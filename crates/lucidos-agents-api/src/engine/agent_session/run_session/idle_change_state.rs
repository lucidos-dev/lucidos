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
    pub(super) worktree_path: Option<&'a Path>,
    pub(super) tracked_branch: &'a str,
    pub(super) is_external_repo: bool,
    pub(super) anchor_sha: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct IdleChangeState {
    pub(super) branch_name: String,
    pub(super) has_changes: bool,
    pub(super) requires_restart: bool,
    pub(super) changed_files: Option<Vec<String>>,
}

pub(super) async fn resolve_idle_change_state(input: IdleChangeStateInput<'_>) -> IdleChangeState {
    let branch_name = resolve_idle_branch(&input).await;

    let changed_files = match branch_changed_files_checked(input.repo_root, &branch_name).await {
        Ok(files) => Some(files),
        Err(e) => {
            log_legacy!(
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
        Some((adopted, _note)) => {
            log_legacy!(
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
