//! Bootstrap-time projection seeding for Claude Code sessions.
//!
//! The `thread_summaries.coding_agent_has_diff` column is the single signal that
//! drives the WaitingBanner Diff button. Live updates flow through the
//! `ChangeProposed` / `ChangeApplied` / `ChangeDiscarded` / `ThreadArchived`
//! handlers in `event_bus_projection.rs`. Those handlers are the
//! steady-state truth — but they only fire on new events.
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
//! effect that can happen after the bus emit returns. We only fire the UPDATE
//! when commits exist; the column default already covers the common
//! fresh-branch case.
//!
//! Also invoked by the engine-startup sweep in `agent_recovery` to reconcile
//! all live CC threads — that's why this lives at the engine top level rather
//! than under `agent_session/`.
//!
//! The seed runs after the SessionStarted bus emit, never inside the
//! projection tx — `git rev-list` inside a Postgres tx couples projection
//! latency to git health and blocks the event commit on a long-running
//! subprocess.

use sqlx::PgPool;
use std::path::Path;
use uuid::Uuid;

/// Inspect the worktree's git state and, if `branch_name` has commits beyond
/// the default branch, set `thread_summaries.coding_agent_has_diff = TRUE` for
/// `thread_id`. No-op (no DB write) when `has_branch_commits` returns false,
/// which covers fresh branches with no commits beyond main — the column
/// default already reflects that. Failures are logged but never propagated —
/// the Claude Code session must keep starting even if Postgres or git hiccups.
///
/// Trust note: `git_ops::has_branch_commits` returns `true` on git errors
/// (defensive default — see its docstring). A failing `git rev-list`
/// therefore optimistically enables the Diff button. The next post-commit
/// hook fire or the engine-startup recovery sweep reconciles the column
/// once git is healthy.
///
/// DB failures log at `[SessionSeed]`; git failures log upstream at `[Git]`
/// and surface here as an optimistic TRUE.
pub(crate) async fn seed_coding_agent_has_diff(
    pool: &PgPool,
    thread_id: Uuid,
    repo_root: &Path,
    branch_name: &str,
) {
    if !crate::engine::git_ops::has_branch_commits(repo_root, branch_name).await {
        // Fresh branch — column default is FALSE, nothing to do. Saves a
        // round-trip in the common "session just started, no commits yet"
        // case.
        return;
    }

    if let Err(e) = sqlx::query(
        "UPDATE thread_summaries SET coding_agent_has_diff = TRUE WHERE thread_id = $1",
    )
    .bind(thread_id)
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
