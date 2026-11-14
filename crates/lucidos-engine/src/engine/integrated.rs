//! *Integrated sub-threads*: coding-agent sub-threads spawned to fold into
//! their parent's branch. Each runs the fast tier of `/harden`, proposes
//! nothing, and hands its branch up. The parent's agent merges it, and the
//! parent's own `/harden` reviews only what no fast tier reviewed.
//!
//! `thread_summaries.coding_agent_integrated` holds the flag. The child's
//! first `MessageReceived` writes it from the queued spawn request, before its
//! first run can fail. `SessionStarted.integrated` asserts it again. Every read
//! here starts at that column.

use crate::engine::git_ops::HardenTier;
use crate::engine::thread_events::IntegratedBranch;
use uuid::Uuid;

/// The newest session's branch for a thread, as a lateral join on `t`.
const SESSION_BRANCH_JOIN: &str = "JOIN LATERAL ( \
         SELECT payload->>'branch' AS branch FROM events e \
         WHERE e.event_type = 'SessionStarted' AND e.thread_id = t.thread_id \
         ORDER BY e.sequence DESC LIMIT 1 \
     ) s ON TRUE";

/// The thread whose newest session named branch `$1`, joined as `p`.
const BRANCH_THREAD: &str = "SELECT p.* FROM events e \
     JOIN thread_summaries p ON p.thread_id = e.thread_id \
     WHERE e.event_type = 'SessionStarted' AND e.payload->>'branch' = $1 \
     ORDER BY e.sequence DESC LIMIT 1";

/// Whether `thread_id` is an integrated sub-thread. A failed read answers no:
/// the work is then held for hardening or proposed, never handed up unseen.
pub(crate) async fn thread_is_integrated(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT coding_agent_integrated FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!("[Integrated] integrated flag for {}: {}", thread_id, e);
        None
    })
    .unwrap_or(false)
}

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

/// The tier `/harden` runs on `branch_name`: fast while an integrated
/// sub-thread's branch is handed up, full for every other. A branch with a
/// pending or set-aside change of its own is applied directly, so Apply's
/// full marker must be reachable. A branch no session names, or a failed
/// read, is full.
pub(crate) async fn expected_harden_tier(pool: &sqlx::PgPool, branch_name: &str) -> HardenTier {
    let integrated: Option<bool> = sqlx::query_scalar(&format!(
        "SELECT p.coding_agent_integrated AND NOT EXISTS (SELECT 1 FROM changes c \
         WHERE c.branch_name = $1 AND c.status IN ($2, $3)) FROM ({BRANCH_THREAD}) p"
    ))
    .bind(branch_name)
    .bind(crate::core::changes::ChangeStatus::Pending)
    .bind(crate::core::changes::ChangeStatus::SetAside)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!("[Integrated] expected tier for {}: {}", branch_name, e);
        None
    });
    if integrated == Some(true) {
        HardenTier::Fast
    } else {
        HardenTier::Full
    }
}

/// The heads the `/harden` runs of `branch_name`'s integrated children
/// certified. Every commit under one was reviewed by its child's run, so the
/// parent's review skips it (`scripts/harden-scope.sh`). A marker of either
/// tier counts, stale or fresh: the head it names was reviewed either way.
pub(crate) async fn integrated_child_heads(
    pool: &sqlx::PgPool,
    branch_name: &str,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "SELECT DISTINCT h.head_sha FROM ({BRANCH_THREAD}) p \
         JOIN thread_summaries t ON t.parent_thread_id = p.thread_id \
         {SESSION_BRANCH_JOIN} \
         JOIN hardened_branches h ON h.branch_name = s.branch \
         WHERE t.coding_agent_integrated \
         ORDER BY h.head_sha"
    ))
    .bind(branch_name)
    .fetch_all(pool)
    .await
}

/// The branch an integrated child hands up on its report: its own, at the
/// head its `/harden` certified. `None` unless the child's work waits as
/// `integrated_into_parent`, which the proposal hold writes only for a fresh
/// marker.
pub(crate) async fn handed_up_branch(
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
        crate::log!(
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

/// What an integrated spawn request names, for [`integrated_spawn_refusal`].
pub(crate) struct IntegratedSpawn {
    pub(crate) has_parent: bool,
    pub(crate) coding_agent: bool,
    /// The child targets the Lucidos source repo: no app folder, no external
    /// repo.
    pub(crate) lucidos_source: bool,
    /// The parent is a Lucidos-source coding-agent thread.
    pub(crate) parent_is_lucidos_coding_agent: bool,
    pub(crate) thread_exists: bool,
    /// A `mode=human` send. It starts at once rather than through the Thread
    /// Queue, and only the queued request carries the flag.
    pub(crate) human_mode: bool,
}

/// Why an integrated spawn is refused, or `None` when it may go ahead. An
/// integrated child merges into its parent's branch, so both must be
/// Lucidos-source coding-agent threads, and the child must be new.
pub(crate) fn integrated_spawn_refusal(spawn: &IntegratedSpawn) -> Option<&'static str> {
    if spawn.thread_exists {
        return Some("`integrated` starts a new thread. It cannot change an existing one.");
    }
    if spawn.human_mode {
        return Some(
            "`integrated` does not work with `mode=human`: a human-mode spawn starts at once and \
             would start independent. Drop `--mode human`.",
        );
    }
    if !spawn.has_parent {
        return Some(
            "`integrated` needs a parent: spawn with `--relation child`, whose branch the \
             child folds into.",
        );
    }
    if !spawn.coding_agent {
        return Some("`integrated` needs a coding-agent child: pass `--coding-agent`.");
    }
    if !spawn.lucidos_source {
        return Some(
            "`integrated` needs a Lucidos-source child: drop `--folder`, and keep `--repo` \
             on the Lucidos repo.",
        );
    }
    if !spawn.parent_is_lucidos_coding_agent {
        return Some(
            "`integrated` needs a Lucidos-source coding-agent parent: only such a thread has \
             a branch to merge the child into.",
        );
    }
    None
}

#[cfg(test)]
#[path = "integrated_tests.rs"]
mod tests;
