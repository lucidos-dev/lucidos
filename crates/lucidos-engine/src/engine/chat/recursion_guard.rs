use uuid::Uuid;

use crate::engine::LucidosEngine;

/// Maximum thread nesting depth. Root threads are depth 0, first children are
/// depth 1, etc. Spawning is rejected when child_depth would exceed this limit.
pub(crate) const MAX_THREAD_DEPTH: i32 = 3;

/// Maximum number of child threads a single parent can spawn.
pub(crate) const MAX_CHILDREN_PER_THREAD: i64 = 10;

impl LucidosEngine {
    /// Check recursion guard before spawning a child thread.
    ///
    /// Enforces:
    /// 1. Max depth (MAX_THREAD_DEPTH) — prevents unbounded nesting
    /// 2. Max children per parent (MAX_CHILDREN_PER_THREAD) — prevents fan-out explosion
    ///
    /// Returns the child's depth on success, or an error message on violation.
    pub(crate) async fn check_thread_recursion_guard(
        pool: &sqlx::PgPool,
        parent_thread_id: Uuid,
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

        // Check fan-out: how many children does this parent already have?
        let child_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM thread_summaries WHERE parent_thread_id = $1")
                .bind(parent_thread_id)
                .fetch_one(pool)
                .await
                .map_err(|e| format!("Failed to query child thread count: {}", e))?;

        if child_count >= MAX_CHILDREN_PER_THREAD {
            return Err(format!(
                "Maximum child threads per parent ({}) reached. \
                 This thread already has {} children. \
                 Do the remaining work in this thread directly \
                 or wait for existing children to complete.",
                MAX_CHILDREN_PER_THREAD, child_count
            ));
        }

        Ok(child_depth)
    }
}
