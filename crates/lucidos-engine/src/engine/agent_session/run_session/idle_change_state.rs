//! What an idling coding-agent turn believes about its own branch and its diff.
//!
//! The idle boundary is the only branch-reading site whose answer is
//! **durable**: `has_changes` rides `CodingAgentIdled` into
//! `thread_summaries.coding_agent_has_diff`, which is the Diff button's gate.
//! Spawn (`spawn_context`), the worktree's post-commit hook
//! (`api::internal::coding_agent_diff_refresh`) and the session-end path
//! (`completion`) all re-read the branch from the worktree; the idle used to
//! trust the spawn-time name instead, and because it runs LAST it overwrote
//! their correct answers with a wrong one.
//!
//! Two failures were possible there, and this module is where both are now
//! decided, once per idle:
//!
//! 1. The tracked branch had been renamed in place by a skill inside the repo,
//!    so `git diff <base>...<gone-ref>` exited 128.
//! 2. That failure was swallowed into an empty file list, so "git could not
//!    answer" was written down as "there is no diff".

use std::path::Path;

use uuid::Uuid;

use crate::engine::agent_session::external_edits;
use crate::engine::agent_session::lifecycle::idle_change_flags;
use crate::engine::agent_session::resume::lookup_prior_change_flags;
use crate::engine::git_ops::branch_changed_files_checked;

pub(super) struct IdleChangeStateInput<'a> {
    pub(super) pool: &'a sqlx::PgPool,
    pub(super) thread_id: Uuid,
    pub(super) repo_root: &'a Path,
    /// `None` for a session running without a worktree (recovery's "no branch"
    /// path). Nothing to re-read, so the tracked branch stands.
    pub(super) worktree_path: Option<&'a Path>,
    pub(super) tracked_branch: &'a str,
    /// Adoption is external-repo only, matching the gate in `spawn_context`: a
    /// Lucidos-source thread keeps its engine-named branch because Apply
    /// depends on it.
    pub(super) is_external_repo: bool,
    /// The newest SHA this session knows its work sits on top of. See
    /// [`external_edits::try_adopt_branch_at_idle`] for how it is used and why
    /// a first turn's weaker anchor is still safe.
    pub(super) anchor_sha: Option<&'a str>,
}

/// The idle's whole answer, resolved together so no consumer can pick up a
/// different one.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct IdleChangeState {
    /// The branch the rest of the session should use: the tracked one, or the
    /// worktree's real branch when it was safely adopted.
    pub(super) branch_name: String,
    pub(super) has_changes: bool,
    pub(super) requires_restart: bool,
    /// The one diff probe's result. `None` means git could not answer, which is
    /// UNKNOWN: `has_changes` above then carries the thread's prior state, and
    /// no caller may touch durable change state (propose / reconcile) off it.
    pub(super) changed_files: Option<Vec<String>>,
}

/// Resolve the branch, probe the diff ONCE, and collapse both into the state
/// the idle stamps. Conflict-resolution sessions never reach here: the merge IS
/// the work, so the call site short-circuits them to "has changes".
pub(super) async fn resolve_idle_change_state(input: IdleChangeStateInput<'_>) -> IdleChangeState {
    let branch_name = resolve_idle_branch(&input).await;

    let changed_files = match branch_changed_files_checked(input.repo_root, &branch_name).await {
        Ok(files) => Some(files),
        Err(e) => {
            log!(
                "[AgentSession] Diff probe FAILED at idle for thread {} on branch {}: {}. \
                 Treating the change state as unknown and preserving what the thread already \
                 had (a failed probe is never reported as 'no changes').",
                input.thread_id,
                branch_name,
                e
            );
            None
        }
    };

    let prior = lookup_prior_change_flags(input.pool, input.thread_id).await;
    let (has_changes, requires_restart) = idle_change_flags(changed_files.as_deref(), prior);

    IdleChangeState {
        branch_name,
        has_changes,
        requires_restart,
        changed_files,
    }
}

/// The branch the idle should compute against: the worktree's real one when it
/// can be safely adopted, otherwise the tracked name.
async fn resolve_idle_branch(input: &IdleChangeStateInput<'_>) -> String {
    let tracked = input.tracked_branch.to_string();
    if !input.is_external_repo {
        return tracked;
    }
    let Some(worktree) = input.worktree_path else {
        return tracked;
    };
    match external_edits::try_adopt_branch_at_idle(
        input.repo_root,
        worktree,
        &tracked,
        input.anchor_sha,
    )
    .await
    {
        // No note is built for the agent here: the turn is over, so there is
        // nobody to tell. The next spawn re-derives the mismatch from
        // `SessionStarted.branch` and injects its own adoption note.
        Some((adopted, _note)) => {
            log!(
                "[AgentSession] Adopting renegade branch '{}' at idle (was tracking '{}') for thread {}",
                adopted,
                tracked,
                input.thread_id
            );
            adopted
        }
        None => tracked,
    }
}

#[cfg(test)]
#[path = "idle_change_state_tests.rs"]
mod tests;
