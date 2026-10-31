//! Boot reporting for trigger fires interrupted by an engine restart.

use crate::api::SharedEngine;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, EventMeta, FireRetry, MessageOrigin, RetryOf, RetrySkipReason, ThreadEvent,
    HUMAN_MESSAGE_SQL,
};
use crate::engine::thread_queue::{ThreadQueue, ThreadQueueRequest};
use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};
use crate::scheduler::user_tasks::{emit_trigger_notification, TriggerNotification};
use crate::triggers::{TriggerConfig, TriggerRunStatus};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) const FIRE_RETRY_WINDOW_SECS: i64 = 24 * 60 * 60;
/// Find graceful-stop cuts even after several days offline, without reviving history.
const CUT_FIRE_LOOKBACK_SECS: i64 = 7 * 24 * 60 * 60;
type Error = Box<dyn std::error::Error + Send + Sync>;

const CUT_TRIGGER_FIRES_SQL: &str = concat!(
    "SELECT ts.thread_id, fire.id AS fire_event_id, fire.created AS started_at, \
     fire.payload AS fire_payload, terminal.payload AS terminal_payload, \
     terminal.event_type AS terminal_type, \
     EXISTS(SELECT 1 FROM events WHERE aggregate_id = ts.thread_id::text \
            AND event_type = 'TriggerCompleted' AND sequence > fire.sequence) AS completed \
     FROM thread_summaries ts \
     CROSS JOIN LATERAL (SELECT id, created, payload, sequence FROM events \
         WHERE aggregate_id = ts.thread_id::text AND event_type = 'TriggerStarted' \
         ORDER BY sequence DESC LIMIT 1) fire \
     CROSS JOIN LATERAL (SELECT event_type, payload, created, sequence FROM events \
         WHERE aggregate_id = ts.thread_id::text AND sequence > fire.sequence \
           AND event_type IN (",
    crate::engine::turn_terminal_events!(),
    ") ORDER BY sequence DESC LIMIT 1) terminal \
     WHERE ts.source = 'trigger' AND ts.parent_thread_id IS NULL \
       AND terminal.created > now() - make_interval(secs => $1) \
       AND NOT EXISTS(SELECT 1 FROM events WHERE aggregate_id = ts.thread_id::text \
           AND sequence > fire.sequence AND ({human_message})) \
       AND NOT EXISTS(SELECT 1 FROM events WHERE aggregate_id = ts.thread_id::text \
           AND event_type = 'TriggerRunInterrupted' AND sequence > terminal.sequence) \
       AND NOT {parked} \
     ORDER BY fire.sequence"
);

#[derive(sqlx::FromRow)]
struct Candidate {
    thread_id: Uuid,
    fire_event_id: Uuid,
    started_at: DateTime<Utc>,
    fire_payload: serde_json::Value,
    terminal_payload: serde_json::Value,
    terminal_type: String,
    completed: bool,
}

struct CutFire {
    thread_id: Uuid,
    fire_event_id: Uuid,
    trigger_id: String,
    trigger_name: String,
    started_at: DateTime<Utc>,
    completed: bool,
    already_retried: bool,
}

/// Read the persisted outcome of all boot sweeps, rather than guessing from status.
async fn find_cut_fires(pool: &PgPool) -> Result<Vec<CutFire>, Error> {
    let sql = CUT_TRIGGER_FIRES_SQL
        .replace("{human_message}", HUMAN_MESSAGE_SQL)
        .replace(
            "{parked}",
            &crate::engine::agent_recovery::unanswered_question_exists_sql("ts.thread_id::text"),
        );
    let rows = sqlx::query_as::<_, Candidate>(&sql)
        .bind(CUT_FIRE_LOOKBACK_SECS as f64)
        .bind(ActorMode::Human.as_str())
        .fetch_all(pool)
        .await?;
    let mut fires = Vec::new();
    for row in rows {
        let terminal = ThreadEvent::from_stored(&row.terminal_type, row.terminal_payload.clone());
        let actor = row
            .terminal_payload
            .get("actor")
            .cloned()
            .map(serde_json::from_value::<MessageOrigin>)
            .transpose()
            .map_err(Error::from);
        let start = ThreadEvent::from_stored("TriggerStarted", row.fire_payload);
        match (terminal, actor, start) {
            (
                Ok(ThreadEvent::ResponseAborted { cause, .. }),
                Ok(actor),
                Ok(ThreadEvent::TriggerStarted {
                    trigger_id,
                    trigger_name,
                    retry_of,
                    ..
                }),
            ) if cause.is_transient() && !cause.promises_auto_resume(actor.as_ref()) => {
                fires.push(CutFire {
                    thread_id: row.thread_id,
                    fire_event_id: row.fire_event_id,
                    trigger_name: trigger_name.unwrap_or_else(|| trigger_id.clone()),
                    trigger_id,
                    started_at: row.started_at,
                    completed: row.completed,
                    already_retried: retry_of.is_some(),
                });
            }
            (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
                log!(
                    "[CutFires] Cannot read fire on thread {}: {}",
                    row.thread_id,
                    error
                );
            }
            _ => log!(
                "[CutFires] Skipping thread {}: no unresumed restart abort",
                row.thread_id
            ),
        }
    }
    Ok(fires)
}

/// Failure stamping precedes catch-up; a query or emit error never prevents boot.
pub(super) async fn settle_cut_trigger_fires(engine: &SharedEngine, pool: &PgPool) {
    let fires = match find_cut_fires(pool).await {
        Ok(fires) => fires,
        Err(error) => {
            log!(
                "[CutFires] Could not find interrupted trigger fires: {}",
                error
            );
            return;
        }
    };
    for fire in fires {
        let result = async {
            let config = engine
                .trigger_configs
                .read()
                .unwrap()
                .get(&fire.trigger_id)
                .cloned();
            let retry = prepare_retry(pool, &engine.thread_queue, config.as_ref(), &fire).await?;
            log!(
                "[CutFires] Retry decision for thread {}: {:?}",
                fire.thread_id,
                retry
            );
            if failure_stamp_needed(pool, &fire).await? {
                engine
                    .record_trigger_executed_at(
                        &fire.trigger_id,
                        TriggerRunStatus::Failed,
                        fire.started_at,
                    )
                    .await?;
            }
            report_cut_fire(pool, &engine.event_bus, &fire, retry, Some(engine)).await?;
            Ok::<_, Error>(())
        }
        .await;
        match result {
            Ok(()) => log!(
                "[CutFires] Reported interrupted fire on thread {}",
                fire.thread_id
            ),
            Err(error) => log!(
                "[CutFires] Could not report thread {}: {}",
                fire.thread_id,
                error
            ),
        }
    }
}

#[derive(Clone, Copy)]
struct RetryFacts {
    already_retried: bool,
    completed: bool,
    spawned_work: bool,
    fresh: bool,
    later_slot_due: bool,
    newer_fire: bool,
    available: bool,
    request_available: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum FireRetryDecision {
    Submit,
    Skip(RetrySkipReason),
}

fn decide_retry(facts: RetryFacts) -> FireRetryDecision {
    for (fails, reason) in [
        (facts.already_retried, RetrySkipReason::AlreadyRetried),
        (facts.completed, RetrySkipReason::FireCompleted),
        (facts.spawned_work, RetrySkipReason::SpawnedWork),
        (!facts.fresh, RetrySkipReason::TooOld),
        (facts.later_slot_due, RetrySkipReason::LaterSlotDue),
        (facts.newer_fire, RetrySkipReason::NewerFire),
        (!facts.available, RetrySkipReason::TriggerUnavailable),
        (
            !facts.request_available,
            RetrySkipReason::RequestUnavailable,
        ),
    ] {
        if fails {
            return FireRetryDecision::Skip(reason);
        }
    }
    FireRetryDecision::Submit
}

fn next_slot(
    config: &TriggerConfig,
    started_at: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, Error> {
    let schedules = config
        .schedule
        .iter()
        .map(|s| crate::engine::tools::scheduler::parse_standard_cron(s).map_err(Error::from))
        .collect::<Result<Vec<_>, _>>()?;
    let timezone = config.timezone.parse::<chrono_tz::Tz>()?;
    Ok(
        crate::engine::tools::scheduler::next_occurrences_after(
            &schedules, timezone, started_at, 1,
        )
        .first()
        .map(|time| time.with_timezone(&Utc)),
    )
}

async fn rebuild_request(
    pool: &PgPool,
    fire: &CutFire,
) -> Result<Option<ThreadQueueRequest>, Error> {
    let payload: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT queued.payload->'data'->'request' FROM events admitted \
         JOIN events queued ON queued.event_type = 'ThreadQueued' \
           AND queued.payload->'data'->>'entry_id' = admitted.payload->'data'->>'entry_id' \
         WHERE admitted.event_type = 'ThreadQueueAdmitted' \
           AND admitted.payload->'data'->>'thread_id' = $1 \
         ORDER BY admitted.sequence DESC, queued.sequence DESC LIMIT 1",
    )
    .bind(fire.thread_id.to_string())
    .fetch_optional(pool)
    .await?;
    let Some(payload) = payload else {
        return Ok(None);
    };
    let request: ThreadQueueRequest = match serde_json::from_value(payload) {
        Ok(request) => request,
        Err(error) => {
            log!(
                "[CutFires] Cannot rebuild request on thread {}: {}",
                fire.thread_id,
                error
            );
            return Ok(None);
        }
    };
    Ok(match &request {
        ThreadQueueRequest::Cron { trigger_id, .. }
        | ThreadQueueRequest::EventTrigger { trigger_id, .. }
            if trigger_id == &fire.trigger_id =>
        {
            Some(request)
        }
        _ => None,
    })
}

async fn prepare_retry(
    pool: &PgPool,
    queue: &Arc<ThreadQueue>,
    config: Option<&TriggerConfig>,
    fire: &CutFire,
) -> Result<FireRetry, Error> {
    let existing: Option<(String, String)> = sqlx::query_as(
        "SELECT payload->'data'->>'entry_id', payload->'data'->'request'->'retry_of'->>'deadline' \
         FROM events WHERE event_type = 'ThreadQueued' \
           AND payload->'data'->'request'->'retry_of'->>'thread_id' = $1 ORDER BY sequence LIMIT 1",
    )
    .bind(fire.thread_id.to_string())
    .fetch_optional(pool)
    .await?;
    if let Some((entry_id, deadline)) = existing {
        return Ok(FireRetry::Queued {
            entry_id: entry_id.parse()?,
            deadline: deadline.parse()?,
        });
    }
    let next = config
        .map(|config| next_slot(config, fire.started_at))
        .transpose()?
        .flatten();
    let (fresh, later_slot_due, spawned_work, newer_fire): (bool, bool, bool, bool) =
        sqlx::query_as(
            "SELECT now() < $1::timestamptz + make_interval(secs => $2), \
         COALESCE($3::timestamptz <= now(), false), \
         EXISTS(SELECT 1 FROM thread_summaries WHERE parent_thread_id = $4), \
         EXISTS(SELECT 1 FROM events WHERE event_type = 'TriggerStarted' \
           AND payload->>'trigger_id' = $5 \
           AND sequence > (SELECT sequence FROM events WHERE id = $6))",
        )
        .bind(fire.started_at)
        .bind(FIRE_RETRY_WINDOW_SECS as f64)
        .bind(next)
        .bind(fire.thread_id)
        .bind(&fire.trigger_id)
        .bind(fire.fire_event_id)
        .fetch_one(pool)
        .await?;
    let request = rebuild_request(pool, fire).await?;
    let facts = RetryFacts {
        already_retried: fire.already_retried,
        completed: fire.completed,
        spawned_work,
        fresh,
        later_slot_due,
        newer_fire,
        available: config.is_some_and(|config| !config.paused),
        request_available: request.is_some(),
    };
    if let FireRetryDecision::Skip(reason) = decide_retry(facts) {
        return Ok(FireRetry::Skipped { reason });
    }
    let deadline = (fire.started_at + chrono::Duration::seconds(FIRE_RETRY_WINDOW_SECS))
        .min(next.unwrap_or(DateTime::<Utc>::MAX_UTC));
    let mut request = request.expect("eligible request");
    match &mut request {
        ThreadQueueRequest::Cron { retry_of, .. }
        | ThreadQueueRequest::EventTrigger { retry_of, .. } => {
            *retry_of = Some(RetryOf {
                thread_id: fire.thread_id,
                deadline,
            });
        }
        _ => unreachable!("rebuild_request accepts only trigger fires"),
    }
    Ok(match queue.submit_boot_retry(request).await? {
        Some(entry_id) => FireRetry::Queued { entry_id, deadline },
        None => FireRetry::Skipped {
            reason: RetrySkipReason::AlreadyQueued,
        },
    })
}

async fn failure_stamp_needed(pool: &PgPool, fire: &CutFire) -> Result<bool, Error> {
    Ok(!sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM events WHERE event_type = 'TriggerExecuted' \
         AND payload->>'trigger_id' = $1 AND (payload->>'last_run')::timestamptz >= $2)",
    )
    .bind(&fire.trigger_id)
    .bind(fire.started_at)
    .fetch_one(pool)
    .await?)
}

fn skip_explanation(reason: RetrySkipReason) -> &'static str {
    match reason {
        RetrySkipReason::AlreadyRetried => "this run was already a retry",
        RetrySkipReason::FireCompleted => {
            "its main work had finished, and Continue resumes the rest"
        }
        RetrySkipReason::SpawnedWork => "it had started sub-threads, which carry on by themselves",
        RetrySkipReason::TooOld => "it started more than 24 hours ago",
        RetrySkipReason::LaterSlotDue => "its next scheduled run is already due",
        RetrySkipReason::NewerFire => "a newer run of this trigger has started",
        RetrySkipReason::TriggerUnavailable => "the trigger is paused, disabled or deleted",
        RetrySkipReason::RequestUnavailable => "Lucidos could not find what started it",
        RetrySkipReason::AlreadyQueued => "another run of this trigger is already queued",
    }
}

async fn notify_cut_fire(
    pool: &PgPool,
    bus: &EventBus,
    fire: &CutFire,
    retry: &FireRetry,
) -> Result<Option<TriggerNotification>, Error> {
    let title = format!("{}: run interrupted", fire.trigger_name);
    let body = match retry {
        FireRetry::Queued { .. } => {
            "An engine restart interrupted this run. Lucidos is running it again once.".into()
        }
        FireRetry::Skipped { reason } => format!(
            "An engine restart interrupted this run. It was not run again: {}.",
            skip_explanation(*reason)
        ),
    };
    let notice = TriggerNotification::new(&title, &body);
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM events WHERE event_type = 'NotificationCreated' \
         AND payload->'data'->>'thread_id' = $1 AND payload->'data'->>'title' = $2 \
         AND sequence > (SELECT sequence FROM events WHERE id = $3))",
    )
    .bind(fire.thread_id.to_string())
    .bind(notice.title())
    .bind(fire.fire_event_id)
    .fetch_one(pool)
    .await?;
    if exists {
        return Ok(None);
    }
    emit_trigger_notification(
        bus,
        &fire.trigger_id,
        &notice,
        Some(fire.thread_id),
        Tap::Navigate {
            to: Box::new(NavigateUi {
                target: NavigateTarget::Thread,
                id: Some(fire.thread_id.to_string()),
                ..Default::default()
            }),
        },
    )
    .await?;
    Ok(Some(notice))
}

/// The last event commits the report and restores the inbox through the lifecycle.
async fn report_cut_fire(
    pool: &PgPool,
    bus: &EventBus,
    fire: &CutFire,
    retry: FireRetry,
    engine: Option<&SharedEngine>,
) -> Result<(), Error> {
    let notice = notify_cut_fire(pool, bus, fire, &retry).await?;
    if let (Some(notice), Some(engine)) = (notice, engine) {
        notice.send_push(engine);
    }
    bus.emit(BusEvent::Thread {
        thread_id: fire.thread_id,
        event: ThreadEvent::TriggerRunInterrupted {
            trigger_id: fire.trigger_id.clone(),
            trigger_name: fire.trigger_name.clone(),
            fire_event_id: fire.fire_event_id,
            retry,
        },
        meta: EventMeta::NONE,
    })
    .await?;
    Ok(())
}

#[cfg(test)]
#[path = "cut_fires_tests.rs"]
pub(crate) mod tests;
