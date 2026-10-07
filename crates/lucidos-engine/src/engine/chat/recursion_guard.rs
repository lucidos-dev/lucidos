use uuid::Uuid;

use crate::engine::thread_lifecycle::ThreadStatus;
use crate::engine::LucidosEngine;

/// Maximum thread nesting depth. Root threads are depth 0, first children are
/// depth 1, etc. Spawning is rejected when child_depth would exceed this limit.
pub(crate) const MAX_THREAD_DEPTH: i32 = 3;

/// A *live child*: a `thread_summaries` row that has not finished its turn, so
/// its parent is still owed a result. Running, parked on a question, paused
/// for a resume the engine promised, or holding an event wait. An idle or
/// failed child has reported, and frees its slot.
///
/// Unqualified columns, so `thread_summaries` must be the only source of them.
static LIVE_CHILD_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "(status IN ({running}, {asking}, {paused}) OR live_event_wait_count > 0)",
        running = ThreadStatus::Running.sql_literal(),
        asking = ThreadStatus::WaitingForUserAnswer.sql_literal(),
        paused = ThreadStatus::Paused.sql_literal(),
    )
});

/// How many live children `$1` has, each counted once. Three sources:
/// - children on the edge;
/// - children moved to top level, each `ChildThreadDetached` on the parent
///   joined to the child's row, so a move frees no live slot (ADR 0278);
/// - spawns still waiting in the Thread Queue, which have no row yet.
///
/// One branch per source, so each reads an index: an `OR` across the first
/// two would scan every row.
static LIVE_CHILD_COUNT_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "SELECT COUNT(*) FROM ( \
             SELECT thread_id FROM thread_summaries \
             WHERE parent_thread_id = $1 AND {live} \
             UNION \
             SELECT thread_id FROM thread_summaries \
             WHERE thread_id = ANY(ARRAY( \
                 SELECT (payload->>'child_thread_id')::uuid FROM events \
                 WHERE aggregate = 'thread' AND aggregate_id = $1::text \
                   AND event_type = 'ChildThreadDetached')) \
               AND {live} \
             UNION \
             SELECT thread_id FROM thread_queue \
             WHERE status = 'queued' AND thread_id IS NOT NULL \
               AND request->>'parent_thread_id' = $1::text \
         ) live_children",
        live = *LIVE_CHILD_SQL,
    )
});

impl LucidosEngine {
    /// Check recursion guard before spawning a child thread.
    ///
    /// Enforces:
    /// 1. Max depth (MAX_THREAD_DEPTH): prevents unbounded nesting.
    /// 2. Max live children (`max_live_children`, the capacity policy's
    ///    `max_concurrent_children_per_thread`): limits fan-out at one moment.
    ///    A child that finishes frees its slot.
    ///
    /// Returns the child's depth on success, or an error message on violation.
    pub(crate) async fn check_thread_recursion_guard(
        pool: &sqlx::PgPool,
        parent_thread_id: Uuid,
        max_live_children: usize,
    ) -> Result<i32, String> {
        let parent_depth: i32 =
            sqlx::query_scalar("SELECT depth FROM thread_summaries WHERE thread_id = $1")
                .bind(parent_thread_id)
                .fetch_optional(pool)
                .await
                .map_err(|e| format!("Failed to query parent thread depth: {}", e))?
                .unwrap_or(0);

        let child_depth = parent_depth + 1;

        if child_depth > MAX_THREAD_DEPTH {
            return Err(format!(
                "Maximum thread nesting depth ({}) exceeded. \
                 This thread is already at depth {}. \
                 Cannot spawn further child threads — \
                 complete the task in this thread instead.",
                MAX_THREAD_DEPTH, parent_depth
            ));
        }

        let live_children: i64 = sqlx::query_scalar(&LIVE_CHILD_COUNT_SQL)
            .bind(parent_thread_id)
            .fetch_one(pool)
            .await
            .map_err(|e| format!("Failed to query live child count: {}", e))?;

        if usize::try_from(live_children).unwrap_or(usize::MAX) >= max_live_children {
            return Err(live_child_cap_refusal(max_live_children, live_children));
        }

        Ok(child_depth)
    }
}

/// The refusal at the child cap. It names the limit as one on children
/// running at the same time, and offers only ways that stay in this family.
fn live_child_cap_refusal(max_live_children: usize, live_children: i64) -> String {
    format!(
        "This thread already has {live_children} child threads running, and at most \
         {max_live_children} may run at the same time. A slot frees as soon as a child \
         finishes. Do the next piece of work in this thread, or end your turn and spawn \
         it when a child reports back. The user can raise the limit in Settings → \
         System → Thread Queue."
    )
}
