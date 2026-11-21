//! Integrated-spawn and branch reads used by the event pipeline.

use uuid::Uuid;

use crate::engine::thread_events::IntegratedBranch;

/// The newest session's branch for a thread, as a lateral join on `t`.
pub const SESSION_BRANCH_JOIN: &str = "JOIN LATERAL ( \
         SELECT payload->>'branch' AS branch FROM events e \
         WHERE e.event_type = 'SessionStarted' AND e.thread_id = t.thread_id \
         ORDER BY e.sequence DESC LIMIT 1 \
     ) s ON TRUE";

/// Whether the Thread Queue holds an integrated spawn request for
/// `thread_id`. The `ThreadQueued` event wrote that request, and its row lives
/// until the spawn's work ends. So the child's first message, which that work
/// emits, always finds it.
pub(crate) async fn queued_spawn_is_integrated(
    conn: &mut sqlx::PgConnection,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM thread_queue \
         WHERE thread_id = $1 AND (request->>'integrated')::boolean)",
    )
    .bind(thread_id)
    .fetch_one(conn)
    .await
}

/// The branch an integrated child hands up on its report: its own, at the
/// head its `/harden` certified. `None` unless the child's work waits as
/// `integrated_into_parent`, which the proposal hold writes only for a fresh
/// marker.
pub async fn handed_up_branch(
    pool: &sqlx::PgPool,
    child_thread_id: Uuid,
) -> Option<IntegratedBranch> {
    sqlx::query_as::<_, (String, String)>(&format!(
        "SELECT s.branch, h.head_sha FROM thread_summaries t {SESSION_BRANCH_JOIN} \
         JOIN hardened_branches h ON h.branch_name = s.branch \
         WHERE t.thread_id = $1 AND t.coding_agent_integrated \
           AND t.coding_agent_unproposed_reason = 'integrated_into_parent' \
         ORDER BY h.hardened_at DESC LIMIT 1"
    ))
    .bind(child_thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log_legacy!(
            "[Integrated] handed-up branch for {}: {}",
            child_thread_id,
            e
        );
        None
    })
    .map(|(branch_name, head_sha)| IntegratedBranch {
        branch_name,
        head_sha,
    })
}
