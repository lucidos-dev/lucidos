//! INVARIANT: The periodic refusal check: read each hook's own record, judge it, report it.
//!
//! # It has no gates, and that is the load-bearing part
//!
//! The ingress check beside it stops on three of them, one being "no webhook is
//! enabled", and a closed gate retracts whatever stands. In a workspace with
//! one hook, switching that hook off IS that condition. So the ingress prober
//! cannot see a disabled hook still taking deliveries, by construction, and
//! that is the 18-day outage this exists to catch.
//!
//! This cycle therefore judges every hook, enabled or not, and makes no network
//! request at all. It reads the `webhooks` rows and the declarations the
//! timeline already carries.
//!
//! What it emits is edge-triggered, and the declared state is read back from
//! the events table every cycle. A restarted engine cannot announce a fault the
//! timeline already holds.
//!
//! See `docs/adr/0235-a-refused-delivery-is-an-outage.md`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use sqlx::PgPool;

use crate::core::webhook_refusal::{
    decide, judge, recovered_secs, refusing_secs, Decision, Declared, RefusalCause, RefusalVerdict,
    Resolution,
};
use crate::core::{Webhook, WebhookStore};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::types::SharedEngine;

/// INVARIANT: Every 15 minutes, on the ingress check's own cadence.
///
/// The two readings are independent but they answer one question between them,
/// so a user comparing the timeline sees them at the same marks.
pub(crate) const WEBHOOK_REFUSAL_CRON: &str = "0 */15 * * * *";

static CHECK_RUNNING: AtomicBool = AtomicBool::new(false);

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

pub(crate) async fn run_webhook_refusal_check(engine: SharedEngine, pool: PgPool) {
    let Some(_guard) = CheckGuard::try_acquire() else {
        log_legacy!("[WebhookRefusal] Skipping run; the previous cycle is still going");
        return;
    };
    if engine.is_shutting_down() {
        return;
    }
    if let Err(e) = run_cycle(&engine, &pool).await {
        log_legacy!("[WebhookRefusal] The cycle could not run: {e}");
    }
}

/// INVARIANT: The cycle proper.
///
/// A read that fails stops the whole cycle rather than judging what it did
/// get. Half the hooks is not a reading. Retracting a live fault over a failed
/// query is the one mistake that costs the user their outage.
async fn run_cycle(
    engine: &SharedEngine,
    pool: &PgPool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let hooks = WebhookStore::list(pool).await?;
    let declared = declared_refusals(pool).await?;
    for event in plan(&hooks, &declared) {
        log_legacy!("[WebhookRefusal] {}", cycle_line(&event));
        engine
            .event_bus
            .emit_or_log(
                BusEvent::System(event),
                "[WebhookRefusal] delivery refusal state",
            )
            .await;
    }
    Ok(())
}

fn plan(hooks: &[Webhook], declared: &HashMap<String, StandingRefusal>) -> Vec<SystemEvent> {
    let mut out: Vec<SystemEvent> = hooks
        .iter()
        .filter_map(|hook| {
            let id = hook.id.to_string();
            let said = declared.get(&id);
            announce(
                &id,
                &hook.name,
                said,
                decide(judge(hook), said.map(|d| &d.declared), Some(hook)),
                Some(hook),
            )
        })
        .collect();

    // INVARIANT: A declaration whose hook is gone has nothing left to judge, and nothing
    // else will ever retract it. Sweeping here is what keeps a deleted hook's
    // bar from standing for good.
    let live: std::collections::HashSet<String> =
        hooks.iter().map(|hook| hook.id.to_string()).collect();
    let mut orphans: Vec<(&String, &StandingRefusal)> = declared
        .iter()
        .filter(|(id, _)| !live.contains(*id))
        .collect();
    orphans.sort_by(|a, b| a.0.cmp(b.0));
    out.extend(orphans.into_iter().filter_map(|(id, said)| {
        announce(
            id,
            &said.webhook_name,
            Some(said),
            decide(RefusalVerdict::Clear, Some(&said.declared), None),
            None,
        )
    }));
    out
}

fn announce(
    webhook_id: &str,
    webhook_name: &str,
    declared: Option<&StandingRefusal>,
    decision: Decision,
    hook: Option<&Webhook>,
) -> Option<SystemEvent> {
    match decision {
        Decision::Nothing => None,
        Decision::Declare(cause) => {
            let run = hook.map(|h| &h.refusal_run);
            Some(SystemEvent::WebhookDeliveriesRefused {
                webhook_id: webhook_id.to_string(),
                webhook_name: webhook_name.to_string(),
                enabled: hook.is_some_and(|h| h.enabled),
                cause,
                refusals: run.map_or(0, |r| r.refusals),
                reasons: run.map(|r| r.reasons.clone()).unwrap_or_default(),
                refusing_since: run
                    .and_then(|r| r.since)
                    .map(|at| at.to_rfc3339())
                    .unwrap_or_default(),
                refusing_secs: run.map_or(0, refusing_secs),
            })
        }
        Decision::Recover(resolution) => {
            declared.map(|said| SystemEvent::WebhookDeliveriesRecovered {
                webhook_id: webhook_id.to_string(),
                webhook_name: webhook_name.to_string(),
                cause: said.declared.cause,
                resolution,
                refusing_since: said.declared.since.to_rfc3339(),
                refusing_secs: recovered_secs(&said.declared, resolution, hook, said.refusing_secs),
            })
        }
    }
}

fn cycle_line(event: &SystemEvent) -> String {
    match event {
        SystemEvent::WebhookDeliveriesRefused {
            webhook_name,
            cause,
            refusals,
            refusing_secs,
            ..
        } => format!(
            "'{webhook_name}' has turned away {refusals} deliveries over \
             {refusing_secs} seconds: {}",
            cause_word(*cause)
        ),
        SystemEvent::WebhookDeliveriesRecovered {
            webhook_name,
            refusing_secs,
            resolution,
            ..
        } => format!(
            "'{webhook_name}' stopped refusing after {refusing_secs} seconds: {}",
            resolution_word(*resolution)
        ),
        other => format!("unexpected event {}", other.event_type()),
    }
}

#[derive(Debug, Clone)]
pub struct StandingRefusal {
    pub webhook_name: String,
    pub declared: Declared,
    /// INVARIANT: How long the run has been going by now, in seconds.
    ///
    /// The declaration's own age plus the age of the row carrying it, which is
    /// the WHOLE run rather than the part since it was announced. Nothing is
    /// declared before the 30-minute floor, so the row's age alone understates
    /// every outage by at least that much. On a hook quiet enough to take a
    /// week reaching the count, it understates by that week.
    pub refusing_secs: i64,
}

#[derive(Debug, Deserialize)]
struct RefusedPayload {
    webhook_id: String,
    webhook_name: String,
    cause: RefusalCause,
    refusing_since: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    refusing_secs: i64,
}

pub async fn declared_refusals(
    pool: &PgPool,
) -> Result<HashMap<String, StandingRefusal>, sqlx::Error> {
    type Row = (String, String, Option<serde_json::Value>, i64);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT DISTINCT ON (aggregate_id) aggregate_id, event_type, payload->'data', \
         EXTRACT(EPOCH FROM now() - created)::bigint \
         FROM events \
         WHERE event_type IN ('WebhookDeliveriesRefused', 'WebhookDeliveriesRecovered') \
         ORDER BY aggregate_id, created DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(_, event_type, data, row_age_secs)| {
            let payload = parse_declaration(&event_type, &data.unwrap_or(serde_json::Value::Null))?;
            Some((
                payload.webhook_id.clone(),
                StandingRefusal {
                    webhook_name: payload.webhook_name,
                    declared: Declared {
                        cause: payload.cause,
                        since: payload.refusing_since,
                    },
                    refusing_secs: payload.refusing_secs.saturating_add(row_age_secs.max(0)),
                },
            ))
        })
        .collect())
}

fn parse_declaration(event_type: &str, data: &serde_json::Value) -> Option<RefusedPayload> {
    if event_type != "WebhookDeliveriesRefused" {
        return None;
    }
    serde_json::from_value(data.clone()).ok()
}

fn cause_word(cause: RefusalCause) -> &'static str {
    match cause {
        RefusalCause::Disabled => "it is switched off",
        RefusalCause::Verification => "they fail verification",
    }
}

fn resolution_word(resolution: Resolution) -> &'static str {
    match resolution {
        Resolution::Accepted => "a delivery verified",
        Resolution::Reconfigured => "the enabled flag moved",
        Resolution::Quiet => "nothing has arrived for a fortnight",
        Resolution::Removed => "the webhook is gone",
    }
}

#[cfg(test)]
#[path = "webhook_refusal_tests.rs"]
mod tests;
