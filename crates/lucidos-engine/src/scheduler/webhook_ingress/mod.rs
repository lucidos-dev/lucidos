//! INVARIANT: The periodic ingress check: probe the public path, judge it, report it.
//!
//! With a hook socket and an enabled webhook, probe the stated public URL or
//! discover Funnel. Declare when neither route is available. If a setting or
//! Funnel status could not be read, hold the standing declarations.
//!
//! What a cycle emits is edge-triggered. The declared state is read back from
//! the events table every cycle. A restarted engine therefore cannot announce
//! an outage the timeline already carries.
//!
//! A round the computer slept through is not evidence. It cannot declare or
//! add a strike, and it breaks the chain, so the first round after a wake does
//! not count either. It can still recover, because sleep cannot fake a 401. A
//! long sleep is said once, as `WebhookDeliveriesSleptThrough`, and blames
//! nothing (`docs/adr/0367-a-sleeping-host-is-not-a-dead-ingress.md`).
//!
//! The payloads are pinned by
//! `docs/adr/0143-webhook-ingress-probed-per-address-family.md`, because a
//! workspace trigger codes against them.

mod dns;
mod funnel;
mod probe;
mod route;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::Deserialize;
use sqlx::PgPool;

use crate::core::host_sleep::{self, ClockReading, RoundClocks};
use crate::core::preference_catalog::WEBHOOK_PUBLIC_URL;
use crate::core::webhook_ingress::{
    decide, degraded_families, judge, recovers, AddressProbe, Decision, Family, FamilyVerdict,
    Stage, STRIKES_BEFORE_DEGRADED,
};
use crate::core::webhook_probe_token;
use crate::core::WebhookStore;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::types::SharedEngine;

pub(crate) const WEBHOOK_INGRESS_CRON: &str = "0 */15 * * * *";

/// INVARIANT: The period [`WEBHOOK_INGRESS_CRON`] fires on, so a late tick can be told.
const CHECK_PERIOD: Duration = Duration::from_secs(15 * 60);

const SLEEP_WORTH_REPORTING: Duration =
    Duration::from_secs(CHECK_PERIOD.as_secs() * STRIKES_BEFORE_DEGRADED as u64);

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

static CHECK_RUNNING: AtomicBool = AtomicBool::new(false);

static WATCH: Mutex<Watch> = Mutex::new(Watch::new());

/// INVARIANT: What the check carries from one round to the next.
///
/// Memory only, which the statelessness rule allows for a cache. Losing it on
/// restart costs one extra round before a real outage is declared, and starts a
/// new sleep spell. Whether an outage stands is read from the events table.
#[derive(Debug)]
struct Watch {
    last_cycle: Option<CycleMemory>,
    last_target: Option<(route::Route, String)>,
    /// INVARIANT: Where the previous round ended, whatever it did. The sleep sign between
    /// rounds reads it, so breaking the debounce chain leaves it alone.
    last_round_end: Option<ClockReading>,
    spell: Duration,
}

#[derive(Debug, Default)]
struct CycleMemory {
    degraded: Vec<Family>,
    strikes: u32,
}

struct Round {
    previous_end: Option<ClockReading>,
    start: ClockReading,
    /// INVARIANT: Where [`Round::measure`] read the clocks. The next round measures from
    /// here, so a sleep after it, during an emit, still shows there.
    measured_end: Option<ClockReading>,
}

impl Round {
    fn measure(&mut self) -> Option<Duration> {
        let end = ClockReading::now();
        self.measured_end = Some(end);
        host_sleep::slept(&RoundClocks {
            previous_end: self.previous_end,
            start: self.start,
            end,
            period: CHECK_PERIOD,
        })
    }
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

pub(crate) async fn run_webhook_ingress_check(engine: SharedEngine, pool: PgPool) {
    let Some(_guard) = CheckGuard::try_acquire() else {
        log_legacy!("[WebhookIngress] Skipping run; the previous cycle is still going");
        return;
    };
    if engine.is_shutting_down() {
        return;
    }
    let mut round = watch().begin_round();
    if let Some(reason) = run_cycle(&engine, &pool, &mut round).await {
        log_legacy!("[WebhookIngress] Not probing: {reason}");
    }
    watch().last_round_end = Some(round.measured_end.unwrap_or_else(ClockReading::now));
}

async fn run_cycle(
    engine: &SharedEngine,
    pool: &PgPool,
    round: &mut Round,
) -> Option<&'static str> {
    let Some(hook_port) = configured_hook_port() else {
        return stand_down(engine, pool, None, "this engine has no hook socket").await;
    };
    let Some(slug) = crate::core::base_path::workspace_id() else {
        let reason = "this engine has no gateway slug, so no public delivery path";
        return stand_down(engine, pool, None, reason).await;
    };
    let hooks = match WebhookStore::list(pool).await {
        Ok(hooks) => hooks,
        Err(e) => {
            log_legacy!("[WebhookIngress] The webhook list could not be read: {e}");
            return undetermined(round, "the webhook list could not be read");
        }
    };
    // INVARIANT: The list is ordered by creation, so every cycle picks the same hook.
    let Some(hook) = hooks.into_iter().find(|hook| hook.enabled) else {
        return stand_down(engine, pool, None, "no webhook is enabled").await;
    };
    let stated = WEBHOOK_PUBLIC_URL.try_stored(pool).await;
    let has_stated_url = matches!(&stated, Ok(Some(_)));
    let ingress = match route::select_route(stated, || funnel::public_ingress(hook_port)).await {
        Ok(Some(ingress)) => ingress,
        Ok(None) => {
            let reason = "no public webhook URL or funnel carries the hook port";
            return stand_down(engine, pool, Some(&hook), reason).await;
        }
        Err(reason) => return undetermined(round, reason),
    };

    let resolver = match resolver_client() {
        Ok(resolver) => resolver,
        Err(e) => {
            log_legacy!("[WebhookIngress] No resolver client could be built: {e}");
            return undetermined(round, "no resolver client could be built");
        }
    };

    let found = match addresses_to_probe(
        dns::public_addresses(&resolver, &ingress.host).await,
        has_stated_url,
    ) {
        Ok(Some(found)) => found,
        Ok(None) => {
            let reason = "the public webhook URL has no public address";
            return stand_down(engine, pool, Some(&hook), reason).await;
        }
        Err(reason) => return undetermined(round, reason),
    };

    let unverified = match declared_unverified(pool).await {
        Ok(standing) => standing,
        Err(e) => {
            log_legacy!("[WebhookIngress] The unverified state could not be read: {e}");
            return undetermined(round, "the unverified state could not be read");
        }
    };
    let declared = match declared_outage(pool).await {
        Ok(declared) => declared,
        Err(e) => {
            log_legacy!("[WebhookIngress] The declared state could not be read: {e}");
            return undetermined(round, "the declared state could not be read");
        }
    };

    let path = ingress.delivery_path(&slug, hook.id);
    let Some(addresses) = probe_each(&ingress, &path, &found).await else {
        return undetermined(round, "no probe token could be minted");
    };
    let families = judge(&addresses);
    let observed = degraded_families(&families);

    let blocked = local_egress_families(&addresses);
    if !blocked.is_empty() {
        log_legacy!(
            "[WebhookIngress] This host cannot send to {}:{} over {}, so it judged nothing there",
            ingress.host,
            ingress.port,
            family_list(&blocked)
        );
    }

    let slept = round.measure();
    if let Some(slept) = slept {
        log_legacy!(
            "[WebhookIngress] This computer slept {} seconds around this round, so it judged nothing",
            slept.as_secs()
        );
    }
    let (decision, spell) = {
        let mut watch = watch();
        watch.set_target(&ingress, &path);
        let decision = watch.record_cycle(&families, declared.as_ref(), slept);
        (decision, watch.record_sleep(slept))
    };

    report_verification(&engine.event_bus, unverified, None).await;

    if let Some(spell) = spell {
        log_legacy!(
            "[WebhookIngress] This computer slept {} minutes while {} was on, so deliveries sent then never arrived",
            spell.as_secs() / 60,
            hook.name
        );
        engine
            .event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::WebhookDeliveriesSleptThrough {
                    webhook_id: hook.id.to_string(),
                    webhook_name: hook.name.clone(),
                    host: ingress.host.clone(),
                    port: ingress.port,
                    slept_secs: spell.as_secs(),
                }),
                "[WebhookIngress] WebhookDeliveriesSleptThrough",
            )
            .await;
    }

    match decision {
        Decision::Nothing => {}
        Decision::Declare => {
            log_legacy!(
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
            let down = declared?;
            log_legacy!(
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

fn addresses_to_probe(
    answer: dns::PublicAddresses,
    has_stated_url: bool,
) -> Result<Option<Vec<std::net::IpAddr>>, &'static str> {
    match answer {
        dns::PublicAddresses::Found(found) => Ok(Some(found)),
        dns::PublicAddresses::NoRecord if has_stated_url => Ok(None),
        dns::PublicAddresses::NoRecord => {
            Err("the resolvers named no public address for the public webhook host")
        }
        dns::PublicAddresses::Unknown => Err("the public webhook hostname could not be resolved"),
    }
}

async fn probe_each(
    ingress: &route::Route,
    path: &str,
    addresses: &[std::net::IpAddr],
) -> Option<Vec<AddressProbe>> {
    let token = match webhook_probe_token::mint() {
        Ok(token) => token,
        Err(e) => {
            log_legacy!("[WebhookIngress] No probe token could be minted: {e}");
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

/// INVARIANT: This cycle measured nothing, so it pronounces nothing.
///
/// A health check must not call a path unreachable when it never reached for
/// it. Every reason that lands here left the probe with no address to knock on.
/// The ingress therefore went untested, and a standing outage stays as it is.
///
/// The debounce memory is cleared, because two failures separated by a cycle
/// nobody could read are not consecutive. A sleep spell carries on through it.
fn undetermined(round: &mut Round, reason: &'static str) -> Option<&'static str> {
    watch().measured_nothing(round.measure());
    Some(reason)
}

async fn stand_down(
    engine: &SharedEngine,
    pool: &PgPool,
    unverified_hook: Option<&crate::core::webhooks::Webhook>,
    reason: &'static str,
) -> Option<&'static str> {
    watch().stand_down();
    let declared = match declared_outage(pool).await {
        Ok(declared) => declared,
        Err(e) => {
            log_legacy!("[WebhookIngress] The declared state could not be read: {e}");
            return Some(reason);
        }
    };
    let unverified = match declared_unverified(pool).await {
        Ok(standing) => standing,
        Err(e) => {
            log_legacy!("[WebhookIngress] The unverified state could not be read: {e}");
            return Some(reason);
        }
    };
    report_verification(&engine.event_bus, unverified, unverified_hook).await;

    let Some(down) = declared else {
        return Some(reason);
    };

    log_legacy!(
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

/// The hook named by the standing workspace-wide declaration.
#[derive(Debug, Deserialize)]
pub struct UnverifiedIngress {
    webhook_id: String,
    webhook_name: String,
}

pub async fn declared_unverified(pool: &PgPool) -> Result<Option<UnverifiedIngress>, sqlx::Error> {
    let row: Option<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT event_type, payload->'data' FROM events \
         WHERE event_type IN ('WebhookIngressUnverified', 'WebhookIngressVerified') \
         ORDER BY created DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;

    match row {
        Some((kind, data)) if kind == "WebhookIngressUnverified" => serde_json::from_value(data)
            .map(Some)
            .map_err(|e| sqlx::Error::Decode(Box::new(e))),
        _ => Ok(None),
    }
}

async fn report_verification(
    bus: &crate::engine::event_bus::EventBus,
    standing: Option<UnverifiedIngress>,
    missing_route: Option<&crate::core::webhooks::Webhook>,
) {
    let event = match (standing, missing_route) {
        (None, Some(hook)) => SystemEvent::WebhookIngressUnverified {
            webhook_id: hook.id.to_string(),
            webhook_name: hook.name.clone(),
        },
        (Some(declared), None) => SystemEvent::WebhookIngressVerified {
            webhook_id: declared.webhook_id,
            webhook_name: declared.webhook_name,
        },
        _ => return,
    };
    bus.emit_or_log(
        BusEvent::System(event),
        "[WebhookIngress] verification edge",
    )
    .await;
}

/// INVARIANT: The check's memory. Hold the guard for an update alone, never across an
/// await.
fn watch() -> MutexGuard<'static, Watch> {
    WATCH.lock().unwrap_or_else(|e| e.into_inner())
}

impl Watch {
    const fn new() -> Self {
        Self {
            last_cycle: None,
            last_target: None,
            last_round_end: None,
            spell: Duration::ZERO,
        }
    }

    fn begin_round(&self) -> Round {
        Round {
            previous_end: self.last_round_end,
            start: ClockReading::now(),
            measured_end: None,
        }
    }

    fn set_target(&mut self, ingress: &route::Route, path: &str) {
        if self
            .last_target
            .as_ref()
            .is_some_and(|(last, last_path)| last == ingress && last_path == path)
        {
            return;
        }
        self.last_cycle = None;
        self.last_target = Some((ingress.clone(), path.to_string()));
    }

    /// INVARIANT: Fold a probed round's reading into the debounce, and decide.
    ///
    /// A round the computer slept through breaks the chain and can only
    /// recover. Its failures are the sleep's, not the ingress's.
    fn record_cycle(
        &mut self,
        families: &[FamilyVerdict],
        declared: Option<&DeclaredOutage>,
        slept: Option<Duration>,
    ) -> Decision {
        let declared = declared.map(|declared| declared.families.as_slice());
        if slept.is_some() {
            self.last_cycle = None;
            return if recovers(families, declared) {
                Decision::Recover
            } else {
                Decision::Nothing
            };
        }
        let previous = self.last_cycle.take().unwrap_or_default();
        let (decision, strikes) = decide(families, &previous.degraded, previous.strikes, declared);
        self.last_cycle = Some(CycleMemory {
            degraded: degraded_families(families),
            strikes,
        });
        decision
    }

    fn record_sleep(&mut self, slept: Option<Duration>) -> Option<Duration> {
        match slept {
            Some(slept) => {
                self.carry_sleep(Some(slept));
                None
            }
            None => Some(std::mem::take(&mut self.spell))
                .filter(|spell| *spell >= SLEEP_WORTH_REPORTING),
        }
    }

    fn measured_nothing(&mut self, slept: Option<Duration>) {
        self.last_cycle = None;
        self.carry_sleep(slept);
    }

    fn carry_sleep(&mut self, slept: Option<Duration>) {
        self.spell = self.spell.saturating_add(slept.unwrap_or_default());
    }

    fn stand_down(&mut self) {
        self.last_cycle = None;
        self.last_target = None;
        self.spell = Duration::ZERO;
    }
}

/// INVARIANT: The current outage, as the timeline records it.
///
/// The scheduler reads the families to debounce against. The read route behind
/// the Webhooks page reads the rest, so both surfaces describe one declaration.
///
/// The webhook it names is the hook the probe used, not the owner of the
/// outage. A retraction repeats it, so a reader sees one pair of events about
/// one hook even when the retraction probed nothing.
#[derive(Debug, Clone)]
pub struct DeclaredOutage {
    pub webhook_id: String,
    pub webhook_name: String,
    pub host: String,
    pub port: u16,
    pub families: Vec<Family>,
    pub addresses: Vec<AddressProbe>,
    pub down_since: chrono::DateTime<chrono::Utc>,
    pub down_secs: i64,
}

#[derive(Debug, Deserialize)]
struct DegradedPayload {
    webhook_id: String,
    webhook_name: String,
    host: String,
    port: u16,
    degraded_families: Vec<Family>,
    /// INVARIANT: Filled by `parse_declaration`, never by this derive.
    ///
    /// It is the only descriptive field here, and the rest decide whether an
    /// outage stands. A payload this engine cannot fully read must therefore
    /// cost the diagnosis alone. Losing the whole declaration would strand a
    /// live outage that nothing else can retract.
    #[serde(skip)]
    addresses: Vec<AddressProbe>,
}

pub async fn declared_outage(pool: &PgPool) -> Result<Option<DeclaredOutage>, sqlx::Error> {
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
            addresses: payload.addresses,
            down_since: created,
            down_secs: age_secs,
        }),
    )
}

/// INVARIANT: The outage a stored payload declares.
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
    payload.addresses = data
        .get("addresses")
        .and_then(|addresses| serde_json::from_value(addresses.clone()).ok())
        .unwrap_or_default();
    Some(payload)
}

fn resolver_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(RESOLVE_TIMEOUT)
        .build()
}

/// INVARIANT: The port the gateway's hook socket listens on.
///
/// The gateway hands it over in `LUCIDOS_HOOK_PORT` when it spawns this engine,
/// so the two cannot disagree about it.
fn configured_hook_port() -> Option<u16> {
    hook_port_from(std::env::var("LUCIDOS_HOOK_PORT").ok().as_deref())
}

fn hook_port_from(configured: Option<&str>) -> Option<u16> {
    match configured?.trim().parse::<u16>() {
        Ok(0) | Err(_) => None,
        Ok(port) => Some(port),
    }
}

/// INVARIANT: The families this host could not send for, in canonical order.
///
/// They earn a log line because nothing else reports them. Such a family reads
/// `not-probed`, so no event carries it and the page draws nothing. Without the
/// line, an engine that declined to judge would look exactly like a quiet one.
fn local_egress_families(addresses: &[AddressProbe]) -> Vec<Family> {
    let mut out: Vec<Family> = addresses
        .iter()
        .filter(|a| a.stage == Stage::LocalEgressBlocked)
        .map(|a| a.family)
        .collect();
    out.sort();
    out.dedup();
    out
}

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
    use std::time::Instant;

    #[tokio::test]
    async fn unverified_edges_read_back_after_the_watch_is_recreated() {
        use crate::engine::event_bus::EventBus;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db) = setup_test_db().await;
        let (bus, _) = EventBus::new(pool.clone());
        let (hook, _) = WebhookStore::create(
            &pool,
            &bus,
            "Delivery",
            "DeliveryArrived",
            crate::core::webhooks::WebhookConfig::default(),
            None,
        )
        .await
        .unwrap();

        for _ in 0..2 {
            let mut watch = Watch::new();
            watch.stand_down();
            report_verification(&bus, declared_unverified(&pool).await.unwrap(), Some(&hook)).await;
        }
        let standing = declared_unverified(&pool).await.unwrap().unwrap();
        assert_eq!(standing.webhook_id, hook.id.to_string());
        assert_eq!(standing.webhook_name, hook.name);
        report_verification(&bus, Some(standing), None).await;
        assert!(declared_unverified(&pool).await.unwrap().is_none());

        let rows: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT payload FROM events WHERE event_type IN \
             ('WebhookIngressUnverified', 'WebhookIngressVerified') ORDER BY created",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            rows,
            ["WebhookIngressUnverified", "WebhookIngressVerified"].map(|kind| {
                serde_json::json!({
                    "type": kind,
                    "data": { "webhook_id": hook.id.to_string(), "webhook_name": hook.name },
                })
            }),
        );
        teardown_test_db(&db).await;
    }

    #[test]
    fn the_check_runs_every_fifteen_minutes() {
        assert_eq!(WEBHOOK_INGRESS_CRON, "0 */15 * * * *");
        assert_eq!(WEBHOOK_INGRESS_CRON.split_whitespace().count(), 6);
    }

    #[test]
    fn a_port_this_engine_cannot_use_means_there_is_nothing_to_probe() {
        assert_eq!(hook_port_from(Some("5261")), Some(5261));
        assert_eq!(hook_port_from(Some(" 5261 ")), Some(5261));
        assert_eq!(hook_port_from(Some("0")), None);
        assert_eq!(hook_port_from(Some("")), None);
        assert_eq!(hook_port_from(Some("not a port")), None);
        assert_eq!(hook_port_from(Some("70000")), None);
        assert_eq!(hook_port_from(None), None);
    }

    #[test]
    fn a_family_this_host_could_not_send_for_still_reaches_the_log() {
        let blocked = |address: &str, family| AddressProbe {
            address: address.into(),
            family,
            stage: Stage::LocalEgressBlocked,
            status: None,
            detail: Some("this host cannot reach port 8443 on any address".into()),
        };
        let addresses = vec![
            blocked("2001:db8::1", Family::Ipv6),
            blocked("203.0.113.7", Family::Ipv4),
            blocked("203.0.113.8", Family::Ipv4),
        ];

        assert_eq!(
            local_egress_families(&addresses),
            vec![Family::Ipv4, Family::Ipv6]
        );
        assert_eq!(
            family_list(&local_egress_families(&addresses)),
            "IPv4 and IPv6"
        );

        let healthy = vec![AddressProbe {
            address: "203.0.113.7".into(),
            family: Family::Ipv4,
            stage: Stage::Healthy,
            status: Some(401),
            detail: None,
        }];
        assert!(local_egress_families(&healthy).is_empty());
    }

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
        assert!(parsed.addresses.is_empty());
        assert!(parse_declaration("WebhookIngressRecovered", &degraded).is_none());
    }

    #[test]
    fn an_unreadable_address_costs_the_diagnosis_and_not_the_outage() {
        let mut data = stored(serde_json::json!(["ipv4"]));
        data["addresses"] = serde_json::json!([
            { "address": "203.0.113.7", "family": "ipv4", "stage": "from-the-future",
              "status": null, "detail": null }
        ]);

        let parsed = parse_declaration("WebhookIngressDegraded", &data).expect("still an outage");
        assert_eq!(parsed.degraded_families, vec![Family::Ipv4]);
        assert!(
            parsed.addresses.is_empty(),
            "a list that cannot be read whole is dropped whole, never half"
        );
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
        let probed = AddressProbe {
            address: "203.0.113.7".into(),
            family: Family::Ipv4,
            stage: crate::core::webhook_ingress::Stage::IngressUnreachable,
            status: None,
            detail: Some("could not connect: connection refused".into()),
        };
        let event = SystemEvent::WebhookIngressDegraded {
            webhook_id: "6f1c0f3e-0000-4000-8000-000000000001".into(),
            webhook_name: "github-ci".into(),
            host: "node.tailnet.ts.net".into(),
            port: 8443,
            degraded_families: vec![Family::Ipv4, Family::Ipv6],
            families: judge(std::slice::from_ref(&probed)),
            addresses: vec![probed.clone()],
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
        assert_eq!(parsed.webhook_name, "github-ci");
        assert_eq!(parsed.addresses, vec![probed]);
    }

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

    #[test]
    fn a_stated_route_without_a_public_address_is_unverified() {
        use dns::PublicAddresses::{Found, NoRecord, Unknown};

        assert_eq!(addresses_to_probe(NoRecord, true), Ok(None));
        assert_ne!(
            addresses_to_probe(NoRecord, false).unwrap_err(),
            addresses_to_probe(Unknown, false).unwrap_err(),
        );
        assert!(addresses_to_probe(Unknown, true).is_err());

        let found = vec!["203.0.113.7".parse().expect("test address parses")];
        assert_eq!(
            addresses_to_probe(Found(found.clone()), true),
            Ok(Some(found))
        );
    }

    fn standing(families: Vec<Family>) -> DeclaredOutage {
        DeclaredOutage {
            webhook_id: "6f1c0f3e-0000-4000-8000-000000000001".into(),
            webhook_name: "github-ci".into(),
            host: "node.tailnet.ts.net".into(),
            port: 8443,
            families,
            addresses: Vec::new(),
            down_since: chrono::Utc::now(),
            down_secs: 900,
        }
    }

    const AWAKE: Option<Duration> = None;

    fn asleep(minutes: u64) -> Option<Duration> {
        Some(Duration::from_secs(minutes * 60))
    }

    fn clock(origin: Instant, wall: &str, awake_secs: u64) -> ClockReading {
        let wall = chrono::DateTime::parse_from_rfc3339(wall).expect("test time parses");
        ClockReading {
            wall: wall.into(),
            mono: origin + Duration::from_secs(awake_secs),
        }
    }

    fn slept_through(
        previous_end: Option<ClockReading>,
        start: ClockReading,
        end: ClockReading,
    ) -> Option<Duration> {
        host_sleep::slept(&RoundClocks {
            previous_end,
            start,
            end,
            period: CHECK_PERIOD,
        })
    }

    #[test]
    fn a_late_tick_and_a_long_sleep_are_measured_against_this_schedule() {
        assert_eq!(CHECK_PERIOD, Duration::from_secs(15 * 60));
        assert!(WEBHOOK_INGRESS_CRON.contains("*/15"));
        assert_eq!(SLEEP_WORTH_REPORTING, Duration::from_secs(30 * 60));
    }

    #[test]
    fn two_failures_declare_an_outage_and_one_success_retracts_it() {
        let down = reading(Verdict::Degraded, Verdict::Healthy);
        let up = reading(Verdict::Healthy, Verdict::Healthy);

        let mut watch = Watch::new();
        assert_eq!(watch.record_cycle(&down, None, AWAKE), Decision::Nothing);
        assert_eq!(watch.record_cycle(&down, None, AWAKE), Decision::Declare);
        let ipv4_down = standing(vec![Family::Ipv4]);
        assert_eq!(
            watch.record_cycle(&down, Some(&ipv4_down), AWAKE),
            Decision::Nothing
        );
        assert_eq!(
            watch.record_cycle(&up, Some(&ipv4_down), AWAKE),
            Decision::Recover
        );
        assert_eq!(watch.record_cycle(&up, None, AWAKE), Decision::Nothing);

        let mut watch = Watch::new();
        assert_eq!(watch.record_cycle(&down, None, AWAKE), Decision::Nothing);
        watch.measured_nothing(AWAKE);
        assert_eq!(watch.record_cycle(&down, None, AWAKE), Decision::Nothing);

        let mut watch = Watch::new();
        assert_eq!(watch.record_cycle(&down, None, AWAKE), Decision::Nothing);
        assert_eq!(watch.record_cycle(&down, None, AWAKE), Decision::Declare);
        watch.measured_nothing(AWAKE);
        assert_eq!(
            watch.record_cycle(&down, Some(&ipv4_down), AWAKE),
            Decision::Nothing
        );
    }

    #[test]
    fn a_round_that_slept_mid_probe_declares_nothing() {
        let down = reading(Verdict::Degraded, Verdict::Healthy);
        let mut watch = Watch::new();
        assert_eq!(watch.record_cycle(&down, None, AWAKE), Decision::Nothing);
        assert_eq!(
            watch.record_cycle(&down, None, asleep(14)),
            Decision::Nothing
        );
        assert_eq!(watch.record_cycle(&down, None, AWAKE), Decision::Nothing);
        assert_eq!(watch.record_cycle(&down, None, AWAKE), Decision::Declare);
    }

    #[test]
    fn the_two_false_outages_of_that_night_are_not_declared() {
        let origin = Instant::now();
        let down = reading(Verdict::Degraded, Verdict::Healthy);

        let mut watch = Watch::new();
        let first_end = clock(origin, "2026-10-05T02:30:40Z", 35);
        let first = slept_through(None, clock(origin, "2026-10-05T02:30:05Z", 0), first_end);
        assert_eq!(first, AWAKE);
        assert_eq!(watch.record_cycle(&down, None, first), Decision::Nothing);
        let second = slept_through(
            Some(first_end),
            clock(origin, "2026-10-05T02:48:50Z", 75),
            clock(origin, "2026-10-05T02:49:26Z", 111),
        );
        assert!(second.is_some(), "the two rounds sit in different wakes");
        assert_eq!(watch.record_cycle(&down, None, second), Decision::Nothing);

        let mut watch = Watch::new();
        let first_end = clock(origin, "2026-10-05T05:00:40Z", 35);
        let first = slept_through(None, clock(origin, "2026-10-05T05:00:05Z", 0), first_end);
        assert_eq!(watch.record_cycle(&down, None, first), Decision::Nothing);
        let late = slept_through(
            Some(first_end),
            clock(origin, "2026-10-05T05:25:04Z", 80),
            clock(origin, "2026-10-05T05:25:49Z", 125),
        );
        assert!(late.is_some(), "a tick ten minutes late is a sleep sign");
        assert_eq!(watch.record_cycle(&down, None, late), Decision::Nothing);
    }

    #[test]
    fn a_sleep_between_round_one_and_round_two_resets_the_count() {
        let origin = Instant::now();
        let down = reading(Verdict::Degraded, Verdict::Healthy);
        let mut watch = Watch::new();

        let one_end = clock(origin, "2026-10-05T10:00:20Z", 20);
        let one = slept_through(None, clock(origin, "2026-10-05T10:00:00Z", 0), one_end);
        assert_eq!(watch.record_cycle(&down, None, one), Decision::Nothing);

        let two_end = clock(origin, "2026-10-05T10:15:20Z", 80);
        let two = slept_through(
            Some(one_end),
            clock(origin, "2026-10-05T10:15:00Z", 60),
            two_end,
        );
        assert!(two.is_some());
        assert_eq!(watch.record_cycle(&down, None, two), Decision::Nothing);

        let three_end = clock(origin, "2026-10-05T10:30:20Z", 980);
        let three = slept_through(
            Some(two_end),
            clock(origin, "2026-10-05T10:30:00Z", 960),
            three_end,
        );
        assert_eq!(three, AWAKE);
        assert_eq!(watch.record_cycle(&down, None, three), Decision::Nothing);
        let four = slept_through(
            Some(three_end),
            clock(origin, "2026-10-05T10:45:00Z", 1860),
            clock(origin, "2026-10-05T10:45:20Z", 1880),
        );
        assert_eq!(four, AWAKE);
        assert_eq!(watch.record_cycle(&down, None, four), Decision::Declare);
    }

    #[test]
    fn a_round_that_slept_still_recovers_on_an_answer() {
        let ipv4_down = standing(vec![Family::Ipv4]);
        let mut watch = Watch::new();
        assert_eq!(
            watch.record_cycle(
                &reading(Verdict::Healthy, Verdict::Healthy),
                Some(&ipv4_down),
                asleep(14)
            ),
            Decision::Recover
        );
        assert_eq!(
            watch.record_cycle(
                &reading(Verdict::Degraded, Verdict::Healthy),
                Some(&ipv4_down),
                asleep(14)
            ),
            Decision::Nothing
        );
    }

    #[test]
    fn a_night_of_dark_wakes_is_reported_once_in_the_morning() {
        let mut watch = Watch::new();
        for _ in 0..30 {
            assert_eq!(watch.record_sleep(asleep(14)), None);
        }
        watch.measured_nothing(asleep(14));

        assert_eq!(
            watch.record_sleep(AWAKE),
            Some(Duration::from_secs(31 * 14 * 60))
        );
        assert_eq!(watch.record_sleep(AWAKE), None, "said once");
    }

    #[test]
    fn a_short_sleep_is_not_reported_and_does_not_carry_over() {
        let mut watch = Watch::new();
        assert_eq!(watch.record_sleep(asleep(10)), None);
        assert_eq!(watch.record_sleep(AWAKE), None);
        assert_eq!(watch.record_sleep(asleep(25)), None);
        assert_eq!(watch.record_sleep(AWAKE), None);
    }

    #[test]
    fn nothing_to_protect_forgets_the_spell() {
        let mut watch = Watch::new();
        assert_eq!(watch.record_sleep(asleep(120)), None);
        watch.stand_down();
        assert_eq!(watch.record_sleep(AWAKE), None);
    }
}
