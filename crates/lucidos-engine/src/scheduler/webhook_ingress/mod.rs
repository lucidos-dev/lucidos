//! The periodic ingress check: probe the public path, judge it, report it.
//!
//! Three gates decide whether a cycle runs. There has to be a hook socket, an
//! enabled webhook, and a funnel carrying that socket's port. A closed gate
//! means no probe. A workspace expecting no deliveries has nothing to warn
//! about, so a gate that closes also retracts any warning standing at the time.
//!
//! A gate that closes is not the same as a question we could not ask. Only a
//! completed cycle can emit. So a wedged daemon leaves a live warning exactly
//! as it is, rather than retracting it.
//!
//! What a cycle emits is edge-triggered. The declared state is read back from
//! the events table every cycle. A restarted engine therefore cannot announce
//! an outage the timeline already carries.
//!
//! The payloads are pinned by
//! `docs/adr/0143-webhook-ingress-probed-per-address-family.md`, because a
//! workspace trigger codes against them.

mod dns;
mod funnel;
mod probe;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use sqlx::PgPool;

use crate::api::SharedEngine;
use crate::core::webhook_ingress::{
    decide, degraded_families, judge, judge_no_public_record, AddressProbe, Decision, Family,
    FamilyVerdict,
};
use crate::core::webhook_probe_token;
use crate::core::WebhookStore;
use crate::engine::event_bus::{BusEvent, SystemEvent};

/// Every 15 minutes. Two consecutive failures declare an outage, so the worst
/// case is half an hour of silence before the workspace is told.
pub(crate) const WEBHOOK_INGRESS_CRON: &str = "0 */15 * * * *";

/// How long one DNS over HTTPS request gets.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

static CHECK_RUNNING: AtomicBool = AtomicBool::new(false);

/// What the previous cycle saw, for the two-strike debounce.
///
/// Memory only, which the statelessness rule allows for a cache. Losing it on
/// restart costs one extra cycle before a real outage is declared. What decides
/// whether anything is emitted is read from the events table, not from here.
static LAST_CYCLE: Mutex<Option<CycleMemory>> = Mutex::new(None);

#[derive(Debug, Default)]
struct CycleMemory {
    degraded: Vec<Family>,
    strikes: u32,
}

struct CheckGuard;

impl CheckGuard {
    fn try_acquire() -> Option<Self> {
        CHECK_RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for CheckGuard {
    fn drop(&mut self) {
        CHECK_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// One ingress check, registered on [`WEBHOOK_INGRESS_CRON`].
pub(crate) async fn run_webhook_ingress_check(engine: SharedEngine, pool: PgPool) {
    let Some(_guard) = CheckGuard::try_acquire() else {
        log!("[WebhookIngress] Skipping run; the previous cycle is still going");
        return;
    };
    if engine.is_shutting_down() {
        return;
    }
    if let Some(reason) = run_cycle(&engine, &pool).await {
        log!("[WebhookIngress] Not probing: {reason}");
    }
}

/// The cycle proper. Returns the reason it stopped early, if it stopped early.
async fn run_cycle(engine: &SharedEngine, pool: &PgPool) -> Option<&'static str> {
    let Some(hook_port) = configured_hook_port() else {
        return out_of_service(engine, pool, "this engine has no hook socket").await;
    };
    let Some(slug) = crate::api::base_path::workspace_id() else {
        let reason = "this engine has no gateway slug, so no public delivery path";
        return out_of_service(engine, pool, reason).await;
    };
    let hooks = match WebhookStore::list(pool).await {
        Ok(hooks) => hooks,
        Err(e) => {
            log!("[WebhookIngress] The webhook list could not be read: {e}");
            return undetermined("the webhook list could not be read");
        }
    };
    // The list is ordered by creation, so every cycle picks the same hook.
    let Some(hook) = hooks.into_iter().find(|hook| hook.enabled) else {
        return out_of_service(engine, pool, "no webhook is enabled").await;
    };
    let ingress = match funnel::public_ingress(hook_port).await {
        funnel::FunnelState::Serving(ingress) => ingress,
        funnel::FunnelState::NotServed => {
            return out_of_service(engine, pool, "no funnel carries the hook port").await;
        }
        funnel::FunnelState::Unknown => {
            return undetermined("the funnel could not be read");
        }
    };

    let declared = match declared_outage(pool).await {
        Ok(declared) => declared,
        Err(e) => {
            // Unknown is neither healthy nor degraded. Emitting on a guess would
            // duplicate a live warning or retract one that still holds.
            log!("[WebhookIngress] The declared state could not be read: {e}");
            return undetermined("the declared state could not be read");
        }
    };

    let resolver = match resolver_client() {
        Ok(resolver) => resolver,
        Err(e) => {
            log!("[WebhookIngress] No resolver client could be built: {e}");
            return undetermined("no resolver client could be built");
        }
    };

    let path = format!("/{slug}/{}", hook.id);
    let (addresses, families) = match dns::public_addresses(&resolver, &ingress.host).await {
        dns::PublicAddresses::Unknown => {
            return undetermined("the funnel hostname could not be resolved");
        }
        // Answered, and named nothing. No delivery can address this host.
        dns::PublicAddresses::NoRecord => (Vec::new(), judge_no_public_record()),
        dns::PublicAddresses::Found(found) => match probe_each(&ingress, &path, &found).await {
            Some(probed) => {
                let families = judge(&probed);
                (probed, families)
            }
            None => return undetermined("no probe token could be minted"),
        },
    };
    let observed = degraded_families(&families);

    match record_cycle(&families, declared.as_ref()) {
        Decision::Nothing => {}
        Decision::Declare => {
            log!(
                "[WebhookIngress] {}:{} is degraded over {}",
                ingress.host,
                ingress.port,
                family_list(&observed)
            );
            engine
                .event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::WebhookIngressDegraded {
                        webhook_id: hook.id.to_string(),
                        webhook_name: hook.name.clone(),
                        host: ingress.host.clone(),
                        port: ingress.port,
                        degraded_families: observed,
                        families,
                        addresses,
                    }),
                    "[WebhookIngress] WebhookIngressDegraded",
                )
                .await;
        }
        Decision::Recover => {
            // `Recover` is only reached with a declaration in hand, and the
            // recovered set is whatever that declaration named.
            let down = declared?;
            log!(
                "[WebhookIngress] {}:{} recovered after {} seconds",
                ingress.host,
                ingress.port,
                down.down_secs
            );
            engine
                .event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::WebhookIngressRecovered {
                        webhook_id: hook.id.to_string(),
                        webhook_name: hook.name.clone(),
                        host: ingress.host.clone(),
                        port: ingress.port,
                        recovered_families: down.families,
                        families,
                        addresses,
                        down_since: down.down_since.to_rfc3339(),
                        down_secs: down.down_secs,
                    }),
                    "[WebhookIngress] WebhookIngressRecovered",
                )
                .await;
        }
    }
    None
}

/// Probe every address, under a bearer minted for this cycle alone.
///
/// The token is minted here rather than at the top of the cycle, so it is live
/// only while requests are in flight. `None` means none could be minted.
async fn probe_each(
    ingress: &funnel::PublicIngress,
    path: &str,
    addresses: &[std::net::IpAddr],
) -> Option<Vec<AddressProbe>> {
    let token = match webhook_probe_token::mint() {
        Ok(token) => token,
        Err(e) => {
            log!("[WebhookIngress] No probe token could be minted: {e}");
            return None;
        }
    };
    let target = probe::ProbeTarget {
        host: ingress.host.clone(),
        port: ingress.port,
        path: path.to_string(),
        token,
    };
    Some(probe::probe_all(&target, addresses, probe::LocalRoutes::detect()).await)
}

/// This cycle could not tell healthy from degraded, so nothing is emitted.
///
/// The debounce memory is cleared, because two failures separated by a cycle
/// nobody could read are not consecutive.
fn undetermined(reason: &'static str) -> Option<&'static str> {
    forget_last_cycle();
    Some(reason)
}

/// The ingress is out of service, so no outage of it can still stand.
///
/// Emission is edge-triggered and only a completed cycle emits. Without this, a
/// workspace that switched its funnel off would carry a red warning for good,
/// with the outage age climbing behind it.
async fn out_of_service(
    engine: &SharedEngine,
    pool: &PgPool,
    reason: &'static str,
) -> Option<&'static str> {
    forget_last_cycle();
    let declared = match declared_outage(pool).await {
        Ok(declared) => declared,
        Err(e) => {
            log!("[WebhookIngress] The declared state could not be read: {e}");
            return Some(reason);
        }
    };
    let Some(down) = declared else {
        return Some(reason);
    };

    log!(
        "[WebhookIngress] Retracting the outage of {}:{}: {reason}",
        down.host,
        down.port
    );
    engine
        .event_bus
        .emit_or_log(
            BusEvent::System(SystemEvent::WebhookIngressRecovered {
                webhook_id: down.webhook_id,
                webhook_name: down.webhook_name,
                host: down.host,
                port: down.port,
                recovered_families: down.families,
                // Nothing was probed, so both families read not-probed.
                families: judge(&[]),
                addresses: Vec::new(),
                down_since: down.down_since.to_rfc3339(),
                down_secs: down.down_secs,
            }),
            "[WebhookIngress] WebhookIngressRecovered",
        )
        .await;
    Some(reason)
}

/// Fold this cycle's reading into the debounce memory, and decide.
///
/// The lock is held for the update alone, never across an await.
fn record_cycle(families: &[FamilyVerdict], declared: Option<&DeclaredOutage>) -> Decision {
    let mut memory = LAST_CYCLE.lock().unwrap_or_else(|e| e.into_inner());
    let previous = memory.take().unwrap_or_default();
    let (decision, strikes) = decide(
        families,
        &previous.degraded,
        previous.strikes,
        declared.map(|declared| declared.families.as_slice()),
    );
    *memory = Some(CycleMemory {
        degraded: degraded_families(families),
        strikes,
    });
    decision
}

/// Break the debounce chain, so "consecutive" keeps meaning consecutive.
fn forget_last_cycle() {
    let mut memory = LAST_CYCLE.lock().unwrap_or_else(|e| e.into_inner());
    *memory = None;
}

/// The current outage, as the timeline records it.
///
/// The scheduler reads the families to debounce against. The read route behind
/// the Webhooks page reads the rest, so both surfaces describe one declaration.
///
/// The webhook it names is the hook the probe used, not the owner of the
/// outage. A retraction repeats it, so a reader sees one pair of events about
/// one hook even when the retraction probed nothing.
#[derive(Debug, Clone)]
pub(crate) struct DeclaredOutage {
    pub webhook_id: String,
    pub webhook_name: String,
    pub host: String,
    pub port: u16,
    pub families: Vec<Family>,
    pub down_since: chrono::DateTime<chrono::Utc>,
    pub down_secs: i64,
}

/// The `WebhookIngressDegraded` fields this engine reads back off the timeline.
#[derive(Debug, Deserialize)]
struct DegradedPayload {
    webhook_id: String,
    webhook_name: String,
    host: String,
    port: u16,
    degraded_families: Vec<Family>,
}

/// Read the newest ingress declaration, whichever webhook it named.
///
/// Global on purpose. The ingress is one funnel, and the hook is only the probe
/// target. A declaration therefore survives the probed hook being deleted.
///
/// Postgres computes the age, per ADR 0053. Subtracting a host clock from a
/// database timestamp is how a drifting dev clock reports a negative outage.
pub(crate) async fn declared_outage(pool: &PgPool) -> Result<Option<DeclaredOutage>, sqlx::Error> {
    type Row = (
        String,
        Option<serde_json::Value>,
        chrono::DateTime<chrono::Utc>,
        i64,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT event_type, payload->'data', created, \
         EXTRACT(EPOCH FROM now() - created)::bigint \
         FROM events \
         WHERE event_type IN ('WebhookIngressDegraded', 'WebhookIngressRecovered') \
         ORDER BY created DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;

    let Some((event_type, data, created, age_secs)) = row else {
        return Ok(None);
    };
    let data = data.unwrap_or(serde_json::Value::Null);
    Ok(
        parse_declaration(&event_type, &data).map(|payload| DeclaredOutage {
            webhook_id: payload.webhook_id,
            webhook_name: payload.webhook_name,
            host: payload.host,
            port: payload.port,
            families: payload.degraded_families,
            down_since: created,
            down_secs: age_secs,
        }),
    )
}

/// The outage a stored payload declares.
///
/// A recovery declares nothing. So does a degraded event naming no family,
/// which cannot happen but would otherwise pin an outage nothing can retract.
/// The family order is canonical, because the debounce compares two of these
/// sets for equality.
fn parse_declaration(event_type: &str, data: &serde_json::Value) -> Option<DegradedPayload> {
    if event_type != "WebhookIngressDegraded" {
        return None;
    }
    let mut payload: DegradedPayload = serde_json::from_value(data.clone()).ok()?;
    payload.degraded_families.sort();
    payload.degraded_families.dedup();
    if payload.degraded_families.is_empty() {
        return None;
    }
    Some(payload)
}

/// The client that asks the public resolvers.
///
/// No pinned address, because both endpoints are IP literals already. Proxies
/// are off, so nothing local can answer for them.
fn resolver_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(RESOLVE_TIMEOUT)
        .build()
}

/// The port the gateway's hook socket listens on.
///
/// The gateway hands it over in `LUCIDOS_HOOK_PORT` when it spawns this engine,
/// so the two cannot disagree about it.
fn configured_hook_port() -> Option<u16> {
    hook_port_from(std::env::var("LUCIDOS_HOOK_PORT").ok().as_deref())
}

/// Read the handed-over port. Anything unusable means there is no hook socket.
fn hook_port_from(configured: Option<&str>) -> Option<u16> {
    match configured?.trim().parse::<u16>() {
        Ok(0) | Err(_) => None,
        Ok(port) => Some(port),
    }
}

/// The degraded families, for a log line.
fn family_list(families: &[Family]) -> String {
    families
        .iter()
        .map(|family| match family {
            Family::Ipv4 => "IPv4",
            Family::Ipv6 => "IPv6",
        })
        .collect::<Vec<_>>()
        .join(" and ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::webhook_ingress::Verdict;

    #[test]
    fn the_check_runs_every_fifteen_minutes() {
        // Six fields, seconds first.
        assert_eq!(WEBHOOK_INGRESS_CRON, "0 */15 * * * *");
        assert_eq!(WEBHOOK_INGRESS_CRON.split_whitespace().count(), 6);
    }

    #[test]
    fn a_port_this_engine_cannot_use_means_there_is_nothing_to_probe() {
        // Zero is what the gateway hands over when it runs no hook socket.
        assert_eq!(hook_port_from(Some("5261")), Some(5261));
        assert_eq!(hook_port_from(Some(" 5261 ")), Some(5261));
        assert_eq!(hook_port_from(Some("0")), None);
        assert_eq!(hook_port_from(Some("")), None);
        assert_eq!(hook_port_from(Some("not a port")), None);
        assert_eq!(hook_port_from(Some("70000")), None);
        assert_eq!(hook_port_from(None), None);
    }

    /// A stored payload, with the families under test.
    fn stored(families: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "webhook_id": "6f1c0f3e-0000-4000-8000-000000000001",
            "webhook_name": "github-ci",
            "host": "node.tailnet.ts.net",
            "port": 8443,
            "degraded_families": families,
        })
    }

    #[test]
    fn only_a_degraded_declaration_is_an_outage() {
        let degraded = stored(serde_json::json!(["ipv4"]));
        let parsed = parse_declaration("WebhookIngressDegraded", &degraded).expect("an outage");
        assert_eq!(parsed.degraded_families, vec![Family::Ipv4]);
        // A recovery retracts, so it declares nothing.
        assert!(parse_declaration("WebhookIngressRecovered", &degraded).is_none());
    }

    #[test]
    fn a_declaration_that_names_no_family_pins_nothing() {
        for data in [
            stored(serde_json::json!([])),
            stored(serde_json::json!("ipv4")),
            stored(serde_json::json!(["martian"])),
            serde_json::json!({}),
            serde_json::Value::Null,
        ] {
            assert!(parse_declaration("WebhookIngressDegraded", &data).is_none());
        }
    }

    #[test]
    fn a_declaration_reads_back_off_its_own_wire_shape() {
        // The debounce compares the two family sets for equality, so a payload
        // that did not round trip would re-declare the same outage every cycle.
        let event = SystemEvent::WebhookIngressDegraded {
            webhook_id: "6f1c0f3e-0000-4000-8000-000000000001".into(),
            webhook_name: "github-ci".into(),
            host: "node.tailnet.ts.net".into(),
            port: 8443,
            degraded_families: vec![Family::Ipv4, Family::Ipv6],
            families: judge(&[]),
            addresses: Vec::new(),
        };
        let wire = serde_json::to_value(&event).expect("the event serializes");
        let data = wire.get("data").expect("a tagged payload carries data");

        let parsed = parse_declaration("WebhookIngressDegraded", data).expect("it reads back");
        assert_eq!(
            parsed.degraded_families,
            vec![Family::Ipv4, Family::Ipv6],
            "the stored event is what the next cycle compares against"
        );
        assert_eq!(parsed.host, "node.tailnet.ts.net");
        assert_eq!(parsed.port, 8443);
    }

    /// What one cycle read, in the order every payload lists the families.
    fn reading(ipv4: Verdict, ipv6: Verdict) -> Vec<FamilyVerdict> {
        Family::BOTH
            .iter()
            .zip([ipv4, ipv6])
            .map(|(family, verdict)| FamilyVerdict {
                family: *family,
                verdict,
                healthy: usize::from(verdict == Verdict::Healthy),
                total: usize::from(verdict != Verdict::NotProbed),
            })
            .collect()
    }

    /// A standing declaration over the given families.
    fn standing(families: Vec<Family>) -> DeclaredOutage {
        DeclaredOutage {
            webhook_id: "6f1c0f3e-0000-4000-8000-000000000001".into(),
            webhook_name: "github-ci".into(),
            host: "node.tailnet.ts.net".into(),
            port: 8443,
            families,
            down_since: chrono::Utc::now(),
            down_secs: 900,
        }
    }

    /// The debounce, driven through the memory the cycle actually uses.
    ///
    /// One test rather than several, because that memory is process-wide and
    /// separate tests would race each other through it. What each reading means
    /// is settled in `core::webhook_ingress`; this covers the threading.
    #[test]
    fn two_failures_declare_an_outage_and_one_success_retracts_it() {
        reset_memory();
        let down = reading(Verdict::Degraded, Verdict::Healthy);
        let up = reading(Verdict::Healthy, Verdict::Healthy);

        assert_eq!(record_cycle(&down, None), Decision::Nothing);
        assert_eq!(record_cycle(&down, None), Decision::Declare);
        let ipv4_down = standing(vec![Family::Ipv4]);
        // Still down, and already said so.
        assert_eq!(record_cycle(&down, Some(&ipv4_down)), Decision::Nothing);
        assert_eq!(record_cycle(&up, Some(&ipv4_down)), Decision::Recover);
        // Nothing is retracted twice.
        assert_eq!(record_cycle(&up, None), Decision::Nothing);

        // A cycle nobody could read breaks the chain, so two failures either
        // side of it are not consecutive.
        reset_memory();
        assert_eq!(record_cycle(&down, None), Decision::Nothing);
        forget_last_cycle();
        assert_eq!(record_cycle(&down, None), Decision::Nothing);

        reset_memory();
    }

    fn reset_memory() {
        let mut memory = LAST_CYCLE.lock().unwrap_or_else(|e| e.into_inner());
        *memory = None;
    }
}
