//! INVARIANT: *Integrated sub-threads*: coding-agent sub-threads spawned to fold into
//! their parent's branch. Each runs the fast tier of `/harden`, proposes
//! nothing, and hands its branch up. The parent's agent merges it, and the
//! parent's own `/harden` reviews only what no fast tier reviewed.
//!
//! `thread_summaries.coding_agent_integrated` holds the flag. The child's
//! first `MessageReceived` writes it from the queued spawn request, before its
//! first run can fail. `SessionStarted.integrated` asserts it again. Every read
//! here starts at that column.

use crate::engine::git_ops::HardenTier;
use uuid::Uuid;

use crate::core::store::integrated::SESSION_BRANCH_JOIN;

const BRANCH_THREAD: &str = "SELECT p.* FROM events e \
     JOIN thread_summaries p ON p.thread_id = e.thread_id \
     WHERE e.event_type = 'SessionStarted' AND e.payload->>'branch' = $1 \
     ORDER BY e.sequence DESC LIMIT 1";

/// INVARIANT: Whether `thread_id` is an integrated sub-thread. A failed read answers no:
/// the work is then held for hardening or proposed, never handed up unseen.
pub async fn thread_is_integrated(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    read_thread_is_integrated(pool, thread_id)
        .await
        .unwrap_or_else(|e| {
            crate::log_legacy!("[Integrated] integrated flag for {}: {}", thread_id, e);
            false
        })
}

pub async fn read_thread_is_integrated(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT coding_agent_integrated FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(false))
}

/// INVARIANT: The tier `/harden` runs on `branch_name`: fast while an integrated
/// sub-thread's branch is handed up, full for every other. A branch with a
/// pending or set-aside change of its own is applied directly, so Apply's
/// full marker must be reachable. A branch no session names, or a failed
/// read, is full.
pub async fn expected_harden_tier(pool: &sqlx::PgPool, branch_name: &str) -> HardenTier {
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
        crate::log_legacy!("[Integrated] expected tier for {}: {}", branch_name, e);
        None
    });
    if integrated == Some(true) {
        HardenTier::Fast
    } else {
        HardenTier::Full
    }
}

pub async fn integrated_child_heads(
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

pub struct IntegratedSpawn {
    pub has_parent: bool,
    pub coding_agent: bool,
    pub lucidos_source: bool,
    pub parent_is_lucidos_coding_agent: bool,
    pub thread_exists: bool,
    /// INVARIANT: A `mode=human` send. It starts at once rather than through the Thread
    /// Queue, and only the queued request carries the flag.
    pub human_mode: bool,
}

pub fn integrated_spawn_refusal(spawn: &IntegratedSpawn) -> Option<&'static str> {
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
