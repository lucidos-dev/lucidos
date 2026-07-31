//! Bootstrap-time projection seeding for coding-agent sessions.
//!
//! The `thread_summaries.coding_agent_has_diff` column is the single signal that
//! drives the WaitingBanner Diff button. Live updates flow through the
//! coding-agent post-commit hook's direct refresh plus the `ChangeProposed` /
//! `ChangeApplied` / `ChangeDiscarded` / `ThreadArchived` handlers in
//! `event_bus_projection.rs`. Those paths are the steady-state truth — but
//! they only fire when a new hook or event arrives.
//!
//! When the engine restarts, no new event fires for an idle CC thread that
//! already has commits on its branch. The projection row would stay at the
//! column default (`FALSE`) until the next commit, hiding the Diff button for
//! a thread that absolutely has a diff to show.
//!
//! `seed_coding_agent_has_diff` closes that gap. The CC bootstrap path emits
//! `SessionStarted` (initial start AND resume both go through
//! `run_direct_agent` → `run_session`), and immediately afterwards calls this
//! helper. The git lookup runs OUTSIDE the projection tx — projections must
//! stay synchronous with the event commit, but seeding is a one-shot side
//! effect that can happen after the bus emit returns.
//!
//! Also invoked by the engine-startup sweep in `agent_recovery` to reconcile
//! all live CC threads — that's why this lives at the engine top level rather
//! than under `agent_session/`.
//!
//! The seed runs after the SessionStarted bus emit, never inside the
//! projection tx — `git diff` inside a Postgres tx couples projection
//! latency to git health and blocks the event commit on a long-running
//! subprocess.

use sqlx::PgPool;
use std::path::Path;
use uuid::Uuid;

/// Reconcile `thread_summaries.coding_agent_has_diff` for `thread_id` against
/// the branch's on-disk git state, using the EXACT proposal-worthiness test the
/// recovery sweep uses: `proposal_files_for_branch` (authored commits ahead of
/// main AND a non-empty `git diff <default_diff_base>...<branch>`). Writes the
/// explicit result — TRUE when the branch warrants a proposal, FALSE otherwise —
/// so a stale TRUE left by a prior `ChangeProposed` is corrected on the next
/// session start / restart sweep.
///
/// Sharing `proposal_files_for_branch` (and through it `branch_changed_files` /
/// `default_diff_base`) with both the Diff button and the propose path is the
/// point: the gate that decides whether to SHOW the button, the gate that
/// re-proposes held-back changes, and the algorithm that COMPUTES the diff are
/// the same, so the button can never enable on an empty diff (the
/// `example-repo` migration bug) nor light on a branch the sweep already
/// declined. Two-dot commit existence (`has_branch_commits`) was the wrong gate
/// — a branch can carry commits that produce zero net diff against its true fork
/// point (commit+revert, a force-rewritten local default), or carry *only*
/// engine back-merge commits of already-applied work whose three-dot diff
/// re-surfaces the applied files (real thread `bb9e68d6`); both must read FALSE.
///
/// Failures are logged but never propagated — the Claude Code session must keep
/// starting even if Postgres or git hiccups. `branch_changed_files` swallows
/// git errors to an empty list, so a transient git failure writes FALSE; the
/// next commit hook or restart sweep reconciles once git is healthy. DB
/// failures log at `[SessionSeed]`.
pub(crate) async fn seed_coding_agent_has_diff(
    pool: &PgPool,
    thread_id: Uuid,
    repo_root: &Path,
    branch_name: &str,
) {
    let has_diff = crate::engine::git_ops::proposal_files_for_branch(repo_root, branch_name)
        .await
        .is_some();

    if let Err(e) =
        sqlx::query("UPDATE thread_summaries SET coding_agent_has_diff = $2 WHERE thread_id = $1")
            .bind(thread_id)
            .bind(has_diff)
            .execute(pool)
            .await
    {
        log!(
            "[SessionSeed] Failed to seed coding_agent_has_diff for thread {} branch {}: {}",
            thread_id,
            branch_name,
            e
        );
    }
}

#[cfg(test)]
#[path = "session_seed_tests.rs"]
mod tests;
