use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::thread_lifecycle::ThreadStatus;

/// INVARIANT: SQL fragment listing the CC lifecycle events whose presence AFTER a trigger
/// event implies the trigger was already picked up (CC clearly ran in response).
/// Shared by `has_cc_event_after` and the orphaned-continuation scan so the
/// predicates can't drift apart.
///
/// `ContinuationStarted` marks actuation before the spawn consumer invokes
/// `run_direct_agent`. Counting it prevents startup from dispatching a resume
/// already underway.
pub const CC_LIFECYCLE_EVENTS_SQL: &str = "'SessionStarted', 'CodingAgentIdled', \
    'SessionEnded', 'CodingAgentToolCalled', 'CodingAgentTextStreamed', \
    'CodingAgentUserMessageSent', 'ContinuationStarted'";

pub fn continuation_superseded_events_sql() -> String {
    format!(
        "{CC_LIFECYCLE_EVENTS_SQL}, 'ContinuationRequested', 'ResponseAborted', \
         'ResponseCanceled', 'ResponseFailed', 'ResponseGenerated'"
    )
}

/// INVARIANT: True when the thread is still running and its newest continuation has yet to
/// start or settle.
///
/// The switch-resume path checks this before emitting a request. The orphan
/// scan drives the existing request, whose event ID provides idempotency. A
/// second event ID could cause another spawn.
///
/// Use the orphan scan's predicate so every skipped emit has a request the scan
/// can drive. Emit on DB errors so a transient read failure cannot silently
/// drop the resume.
pub async fn thread_has_unactuated_continuation(pool: &PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(&format!(
        "SELECT EXISTS ( \
             SELECT 1 FROM events e \
             JOIN thread_summaries t ON t.thread_id = e.aggregate_id::uuid \
             WHERE e.aggregate_id = $1 \
               AND e.event_type = 'ContinuationRequested' \
               AND e.aggregate = 'thread' \
               AND t.source = 'claude_code' \
               AND t.status = {} \
               AND NOT EXISTS ( \
                   SELECT 1 FROM events e2 \
                   WHERE e2.aggregate_id = e.aggregate_id \
                     AND e2.sequence > e.sequence \
                     AND e2.event_type IN ({}) \
               ) \
         )",
        ThreadStatus::Running.sql_literal(),
        continuation_superseded_events_sql()
    ))
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| {
        log_legacy!(
            "[SpawnDispatcher] unactuated-continuation probe failed for {}: {}; emitting anyway",
            thread_id,
            e
        );
        false
    })
}
