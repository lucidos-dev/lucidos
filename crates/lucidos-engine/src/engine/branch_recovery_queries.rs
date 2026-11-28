use uuid::Uuid;

use crate::engine::continuation_reason::AUTO_RESUME_AFTER_SWITCH_REASON;

fn restore_request_after_sql(id: &str, floor: &str, automatic_reason: &str) -> String {
    format!("EXISTS (SELECT 1 FROM events r WHERE r.aggregate_id = {id} \
        AND r.sequence > {floor} \
        AND (r.event_type IN ('MessageReceived','CodingAgentUserMessageSent','TriggerStarted','ChildThreadCompleted') \
          OR (r.event_type = 'ContinuationStarted' AND r.payload->>'reason' IS DISTINCT FROM {automatic_reason})))")
}

pub async fn restored_recovery_branches(
    pool: &sqlx::PgPool,
) -> Result<Option<std::collections::HashMap<String, Uuid>>, sqlx::Error> {
    let Some(boundary) = crate::core::snapshot::boundary::read(pool).await? else {
        return Ok(None);
    };
    let fresh_request = restore_request_after_sql("s.aggregate_id", "$1", "$2");
    let branches: Vec<(String, Uuid)> = sqlx::query_as(&format!(
        "SELECT DISTINCT ON (s.payload->>'branch') s.payload->>'branch', s.aggregate_id::uuid FROM \
           (SELECT DISTINCT ON (aggregate_id) aggregate_id, payload, sequence FROM events \
            WHERE event_type = 'SessionStarted' ORDER BY aggregate_id, sequence DESC) s \
         WHERE s.payload->>'branch' IS NOT NULL \
         AND {fresh_request} \
         ORDER BY s.payload->>'branch', s.sequence DESC"
    ))
    .bind(boundary.event_sequence)
    .bind(AUTO_RESUME_AFTER_SWITCH_REASON)
    .fetch_all(pool)
    .await?;
    Ok(Some(branches.into_iter().collect()))
}

const TURN_ENDED_EVENT_TYPES_SQL: &str = "'CodingAgentIdled','ResponseGenerated',\
    'ResponseCanceled','ResponseFailed','SessionEnded','ThreadArchived'";

/// INVARIANT: [`crate::engine::thread_events::SessionEndReason`] values that do NOT end the
/// turn. A `SessionEnded` carrying one is dropped from the lifecycle scan, so
/// the preceding `SessionStarted` becomes the newest lifecycle event again and
/// the branch classifies `running`.
///
/// * `stale_resume` is transient: the agent answered a stale `--resume` with an
///   empty Result, and the handler retries against a fresh session.
/// * `shutdown` is the engine going away mid-turn, the `SessionEnded`-shaped
///   twin of the `ResponseAborted { EngineShutdown }` boundary excluded above.
///   No production site emits it today, but the variant is live in the enum.
///   Re-adding that emit must not silently cost *Switch to new version* its
///   auto-resume.
///
/// Matched through `COALESCE(payload->>'reason','')`, and the `COALESCE` is
/// load-bearing. `reason` is absent on the oldest rows, where a bare `IN` yields
/// NULL and drops them from the scan instead of keeping them. They would fall
/// back to an older `SessionStarted` and classify `running`, which is the bogus
/// interrupt panel this classifier exists to prevent.
const SESSION_ENDED_MID_TURN_REASONS_SQL: &str = "'stale_resume','shutdown'";

const TURN_PROGRESSION_EVENT_TYPES_SQL: &str = "'SessionStarted','CodingAgentUserMessageSent',\
    'MessageReceived','CodingAgentPromptSent','CodingAgentToolCalled',\
    'CodingAgentTextStreamed','ContinuationStarted'";

/// INVARIANT: Classify every coding-agent branch as `running` or `idle`, one row per thread
/// that ever emitted a `SessionStarted` with a branch. `running` means a turn was
/// in flight when the engine died, so resume it or offer Continue. `idle` means
/// the turn ended, so leave the worktree to the cleanup worker.
///
/// A branch is `running` iff its newest lifecycle event is a `SessionStarted`,
/// or a turn-progression event landed after its newest turn-ended event.
/// A *cache keep-alive* the stop cut off is `idle`: it had no work to resume.
/// Everything else is `idle`. There is no third state: the caller computes
/// `in_flight = !idle_branches.contains(branch)`, so a branch missing from both
/// sets would be treated as in-flight.
pub static BRANCH_CLASSIFICATION_SQL: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| {
        format!(
            "WITH last_lifecycle AS ( \
            SELECT DISTINCT ON (thread_id) thread_id, event_type, sequence \
            FROM events \
            WHERE event_type IN ('SessionStarted',{turn_ended}) \
              AND NOT (event_type = 'SessionEnded' \
                       AND COALESCE(payload->>'reason','') IN ({mid_turn_ends})) \
              AND thread_id IS NOT NULL \
            ORDER BY thread_id, sequence DESC \
         ), \
         session_branches AS ( \
            SELECT DISTINCT ON (thread_id) thread_id, payload->>'branch' AS branch \
            FROM events \
            WHERE event_type = 'SessionStarted' AND thread_id IS NOT NULL \
              AND payload->>'branch' IS NOT NULL AND payload->>'branch' != '' \
            ORDER BY thread_id, sequence DESC \
         ) \
         SELECT sb.branch, \
                CASE WHEN EXISTS (SELECT 1 FROM snapshot_restore_boundary WHERE singleton) \
                       AND NOT ({fresh_request}) THEN 'idle' \
                     WHEN COALESCE(ts.keep_alive_turn, FALSE) THEN 'idle' \
                     WHEN ll.event_type = 'SessionStarted' \
                       OR EXISTS ( \
                           SELECT 1 FROM events e3 \
                           WHERE e3.thread_id = ll.thread_id \
                             AND e3.sequence > ll.sequence \
                             AND e3.event_type IN ({progression}) \
                       ) \
                     THEN 'running' ELSE 'idle' END AS status \
         FROM last_lifecycle ll \
         JOIN session_branches sb ON sb.thread_id = ll.thread_id \
         LEFT JOIN thread_summaries ts ON ts.thread_id = ll.thread_id",
            turn_ended = TURN_ENDED_EVENT_TYPES_SQL,
            mid_turn_ends = SESSION_ENDED_MID_TURN_REASONS_SQL,
            progression = TURN_PROGRESSION_EVENT_TYPES_SQL,
            fresh_request = restore_request_after_sql(
                "ll.thread_id::text",
                "(SELECT event_sequence FROM snapshot_restore_boundary WHERE singleton)",
                &format!("'{AUTO_RESUME_AFTER_SWITCH_REASON}'")
            ),
        )
    });

pub async fn branch_turn_in_flight(
    pool: &sqlx::PgPool,
    branch: &str,
) -> Result<Option<bool>, sqlx::Error> {
    let statuses: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT c.status FROM ({}) c WHERE c.branch = $1",
        *BRANCH_CLASSIFICATION_SQL
    ))
    .bind(branch)
    .fetch_all(pool)
    .await?;
    Ok((!statuses.is_empty()).then(|| statuses.iter().any(|s| s == "running")))
}
