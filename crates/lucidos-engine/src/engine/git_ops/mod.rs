pub(crate) use crate::core::repository_lock::MainLocks;
#[cfg(test)]
pub(crate) use crate::core::repository_lock::WORKSPACE_REPO_MUTEX;
pub use crate::core::repository_lock::{lock_workspace_repo, WorkspaceRepoLock, MAIN_LOCKS};

use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::time::Duration;

mod branch_name;
mod checkpoint;
mod commits;
mod harden_marker;
mod merge;
mod origin;
mod plan_approval;
mod plan_marker;
mod restart_detection;
mod worktree;

pub use branch_name::LUCIDOS_BRANCH_PREFIX;
pub(crate) use branch_name::*;
pub(crate) use checkpoint::*;
pub(crate) use commits::*;
pub use harden_marker::record_hardened;
pub(crate) use harden_marker::*;
pub(crate) use merge::*;
pub(crate) use origin::*;
pub use origin::{FollowOutcome, LandingOrder};
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use plan_approval::user_approved_plan;
pub use plan_approval::PlanApprovalError;
pub(crate) use plan_marker::plan_marker_snapshot;
pub use plan_marker::PlanMarkerSnapshot;
pub(crate) use plan_marker::*;
pub(crate) use restart_detection::*;
pub(crate) use worktree::*;
pub use {
    branch_name::allocate_coding_agent_branch, branch_name::branch_name_is_taken,
    branch_name::is_coding_agent_branch, branch_name::is_lane_branch, branch_name::lane_branch,
    branch_name::BranchScope,
};
pub use {
    checkpoint::checkpoint_pair_available, checkpoint::command_checkpoint_ref,
    checkpoint::command_post_image_ref,
};
pub use {
    commits::auto_commit_preserving_marker, commits::auto_commit_safe_files_if_dirty,
    commits::branch_changed_files, commits::branch_changed_files_checked, commits::branch_head_sha,
    commits::commit_worktree_or_err, commits::commits_in_range, commits::current_head_sha,
    commits::describe_branch_changes, commits::git_commit_no_edit, commits::has_branch_commits,
    commits::has_unmerged_authored_commits, commits::local_branch_exists,
    commits::main_history_touches_files, commits::proposal_files_for_branch,
    commits::root_commit_sha, commits::sole_branch_containing, commits::worktree_dirty_files,
};
pub use {
    harden_marker::consume_harden_marker, harden_marker::has_full_harden_marker,
    harden_marker::HardenMarker, harden_marker::HardenMarkerState, harden_marker::HardenTier,
};
pub use {
    merge::catchup_and_ff_to_main, merge::default_diff_base, merge::default_local_branch,
    merge::ff_merge_to_main, merge::push_main_in_background, merge::resolution_merged_main,
    merge::resolve_worktree_base,
};
pub(crate) use {
    plan_approval::approves as plan_approves, plan_approval::proposed_plan_binding,
    plan_approval::user_approved_plan_for, plan_approval::user_picked_plan_variant,
    plan_approval::ApprovalTarget, plan_approval::HumanInput, plan_approval::PlanBinding,
};
pub use {
    plan_marker::carry_child_planned, plan_marker::carry_planned,
    plan_marker::lands_plan_files_only, plan_marker::record_planned_with_kind,
    plan_marker::tracked_branch_elsewhere, plan_marker::validate_plan_files,
    plan_marker::PlanMarkerKind, plan_marker::PlanMarkerState, plan_marker::PlanMarkerWrite,
};
pub use {plan_marker::is_plan_file, plan_marker::record_planned};
pub use {
    restart_detection::files_have_client_update, restart_detection::files_require_restart,
    restart_detection::is_external_repo_path,
};
pub use {
    worktree::add_paths_to_worktree_exclude, worktree::cleanup_failed_spawn,
    worktree::clear_stranded_worktree_dir, worktree::create_sparse_app_worktree,
    worktree::create_sparse_app_worktree_from, worktree::find_worktree_for_branch,
    worktree::install_coding_agent_diff_hook, worktree::is_live_worktree_at,
    worktree::main_worktree, worktree::worktree_add, worktree::worktree_current_branch,
    worktree::worktrees_dir, worktree::WorktreeLookup,
};
pub use {worktree::parse_worktree_list, worktree::short_thread_id};

pub use crate::core::git::{
    canonical_repo_root, git_cmd, git_cmd_env, git_ran_ok, predict_merge_into_main, probe_merge,
    MergeProbe,
};
pub(crate) use crate::core::git::{
    git_cmd_env_timeout, git_cmd_kill_on_timeout, git_common_dir, git_main_worktree,
    lucidos_identity_env, GIT_TIMEOUT,
};

/// INVARIANT: Answer to a yes/no question the engine asks git.
///
/// `Unknown` is the case that has to stay distinguishable: git could not be
/// asked at all, because it failed to spawn or exceeded [`GIT_TIMEOUT`]. That
/// is routine on a saturated host, and collapsing it into `No` is what emptied
/// a live coding-agent worktree on 2026-08-03. `git rev-parse` timed out, the
/// engine read the timeout as "this branch and this worktree are gone", and the
/// recovery path it took ran `git worktree remove --force` over the user's work.
///
/// So there is no `Into<bool>`, no `unwrap_or_default`, and no default default:
/// a caller collapsing the tri-state must name the side `Unknown` falls to via
/// [`GitAnswer::or_unknown`], in the open where a reviewer sees it. The rule
/// that side is chosen by: **an unanswered probe must never be the answer that
/// authorizes a destructive or discarding action** (see the failure-path
/// cleanup rule in `.claude/rules/rust.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitAnswer {
    Yes,
    No,
    Unknown,
}

impl GitAnswer {
    pub fn or_unknown(self, on_unknown: bool) -> bool {
        match self {
            GitAnswer::Yes => true,
            GitAnswer::No => false,
            GitAnswer::Unknown => on_unknown,
        }
    }

    pub fn is_unknown(self) -> bool {
        matches!(self, GitAnswer::Unknown)
    }
}

pub async fn git_answer(args: &[&str], dir: &Path) -> GitAnswer {
    git_answer_with(args, dir, |_| true).await
}

pub async fn git_answer_with(
    args: &[&str],
    dir: &Path,
    predicate: impl FnOnce(&std::process::Output) -> bool,
) -> GitAnswer {
    classify_git_answer(
        git_cmd(args, dir).await,
        predicate,
        GitAnswer::No,
        args,
        dir,
    )
}

/// INVARIANT: [`git_answer_with`] for commands whose non-zero exit is a FAILURE rather
/// than an answer, so it maps to `Unknown` alongside spawn errors and timeouts.
///
/// `git status --porcelain` is the canonical case: it exits non-zero when the
/// path is not a work tree at all (or when a hook or filter blows up, as
/// git-crypt does on a locked repo), which says nothing about whether there is
/// uncommitted work. Reading that as "not dirty" would let a caller conclude
/// there is nothing to lose.
pub async fn git_answer_when_ok(
    args: &[&str],
    dir: &Path,
    predicate: impl FnOnce(&std::process::Output) -> bool,
) -> GitAnswer {
    classify_git_answer(
        git_cmd(args, dir).await,
        predicate,
        GitAnswer::Unknown,
        args,
        dir,
    )
}

pub async fn has_unmerged_paths(dir: &Path) -> GitAnswer {
    git_answer_when_ok(&UNMERGED_PATHS_ARGS, dir, |o| !o.stdout.is_empty()).await
}

/// INVARIANT: Lists unmerged paths without touching the index: porcelain `git diff`
/// rewrites a stale index under `index.lock` whatever `GIT_OPTIONAL_LOCKS` says.
const UNMERGED_PATHS_ARGS: [&str; 3] = ["diff-files", "--name-only", "--diff-filter=U"];

fn classify_git_answer(
    result: Result<std::process::Output, String>,
    predicate: impl FnOnce(&std::process::Output) -> bool,
    on_nonzero: GitAnswer,
    args: &[&str],
    dir: &Path,
) -> GitAnswer {
    match result {
        Ok(o) if o.status.success() => {
            if predicate(&o) {
                GitAnswer::Yes
            } else {
                GitAnswer::No
            }
        }
        Ok(o) => {
            if on_nonzero.is_unknown() {
                log_legacy!(
                    "[Git] `git {}` failed in {}: {}. Treating as unknown, never as a no",
                    args.join(" "),
                    dir.display(),
                    String::from_utf8_lossy(&o.stderr).trim()
                );
            }
            on_nonzero
        }
        Err(e) => {
            log_legacy!(
                "[Git] cannot answer `git {}` in {}: {}. Treating as unknown, never as a no",
                args.join(" "),
                dir.display(),
                e
            );
            GitAnswer::Unknown
        }
    }
}

const LOCK_FILE_RETRIES: u32 = 20;
const LOCK_FILE_BACKOFF: Duration = Duration::from_millis(50);

/// INVARIANT: `true` when a failed git invocation lost the race for one of the repo's lock
/// files rather than failing on its own merits. git names the lock it could not
/// take, and any of them can be the contended one:
///
/// ```text
/// fatal: Unable to create '<repo>/.git/index.lock': File exists.
/// error: cannot lock ref 'refs/heads/main': Unable to create '<repo>/.git/refs/heads/main.lock': File exists.
/// ```
///
/// Both halves are load-bearing. The quoted `.lock'` keeps this off an unrelated
/// failure that merely mentions a file called `Cargo.lock`. `File exists` is the
/// collision itself, since git prints `strerror` after the lock path. A
/// PERSISTENT reason for not creating it says so there (`Permission denied`, a
/// read-only disk). That must fail on the first attempt, rather than burn the
/// retry budget. The git2 side of the same question is
/// `core::is_transient_repo_contention`.
pub(crate) fn is_lock_file_collision(stderr: &str) -> bool {
    stderr.contains(".lock'") && stderr.contains("File exists")
}

/// INVARIANT: Like [`git_cmd`], but waits out a concurrent holder of a repo lock file
/// instead of failing on the collision.
///
/// A workspace repo has TWO independent writers: the shell `git` calls in this
/// module, and libgit2 through `ArtifactManager` (artifact writes, the
/// `commit_all_dirty` auto-commit after a script run). A git lock is not a
/// waiting lock, so whoever loses errors out immediately. A command that MUST
/// land (the working-tree sync after `main` moves) therefore has to retry, or a
/// routine millisecond-long collision leaves the repo in whatever half-state
/// the loss interrupted.
///
/// Only a lock collision is retried. Every other failure, and every transport
/// error, is returned verbatim on the first attempt.
pub(crate) async fn git_cmd_await_lock_file(
    args: &[&str],
    dir: &Path,
) -> Result<std::process::Output, String> {
    let mut attempt = 0;
    loop {
        let output = git_cmd(args, dir).await?;
        if output.status.success() || attempt == LOCK_FILE_RETRIES {
            return Ok(output);
        }
        if !is_lock_file_collision(&String::from_utf8_lossy(&output.stderr)) {
            return Ok(output);
        }
        attempt += 1;
        tokio::time::sleep(LOCK_FILE_BACKOFF).await;
    }
}

#[cfg(test)]
#[path = "../git_ops_tests/common.rs"]
pub(crate) mod common;

#[cfg(test)]
#[path = "../git_ops_tests/answer.rs"]
mod answer_tests;

#[cfg(test)]
#[path = "../git_ops_tests/origin.rs"]
mod origin_tests;

#[cfg(test)]
#[path = "../git_ops_tests/app_worktree.rs"]
mod app_worktree_tests;

#[cfg(test)]
#[path = "../git_ops_tests/merge.rs"]
mod merge_tests;

#[cfg(test)]
#[path = "../git_ops_tests/harden_marker.rs"]
mod harden_marker_tests;

#[cfg(test)]
#[path = "../git_ops_tests/plan_marker.rs"]
mod plan_marker_tests;

#[cfg(test)]
#[path = "../git_ops_tests/plan_approval.rs"]
mod plan_approval_tests;

#[cfg(test)]
#[path = "../git_ops_tests/branch_queries.rs"]
mod branch_queries_tests;

#[cfg(test)]
#[path = "../git_ops_tests/recover_exclude.rs"]
mod recover_exclude_tests;

#[cfg(test)]
#[path = "../git_ops_tests/spawn_cleanup.rs"]
mod spawn_cleanup_tests;

#[cfg(test)]
#[path = "../git_ops_tests/worktree_validity.rs"]
mod worktree_validity_tests;

#[cfg(test)]
#[path = "../git_ops_tests/commits.rs"]
mod commits_tests;

#[cfg(test)]
#[path = "../git_ops_tests/git_index.rs"]
mod git_index_tests;
