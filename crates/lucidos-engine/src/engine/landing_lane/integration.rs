//! A lane batch's *integration branch*: the lane worktree its lane thread
//! resolves in, the check a clean completion must pass, and the catch-up that
//! lands it.
//!
//! The branch starts at the batch's base. The lane thread merges each member
//! as its own landing merge. Landing then merges whatever reached `main`
//! meanwhile and fast-forwards `main` to the result, under the merge lock.

use super::landing::{landing_merge_of, move_local_main, LandingMerge, MAX_PUSH_ATTEMPTS};
use super::LaneMember;
use crate::engine::claude_code::{WORKTREE_EXCLUDE_PATHS, WORKTREE_WORKSPACE_MARKER};
use crate::engine::git_ops::{
    add_paths_to_worktree_exclude, clear_stranded_worktree_dir, consume_harden_marker,
    consume_plan_marker, git_answer, git_answer_when_ok, git_cmd, landing_base, lane_branch,
    lane_branch_batch, parse_worktree_list, push_to_origin_main, worktree_add,
    worktree_current_branch, worktree_liveness, worktrees_dir, BaseRefusal, GitAnswer,
    LandingOrder, MainLocks, PushOutcome, LANE_BRANCH_PREFIX,
};
use crate::engine::worktree_cleanup::{
    remove_worktree_and_optionally_delete_branch, BranchDisposal,
};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// The subject of the merge that brings `main` into the integration branch.
pub(crate) const CATCH_UP_SUBJECT: &str = "Catch up with main";

/// How many hex characters of the batch id name its worktree directory.
const SHORT_BATCH_ID_LEN: usize = 8;

/// Where a lane batch resolves: its worktree and its integration branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaneWorktree {
    pub(crate) path: PathBuf,
    pub(crate) branch: String,
}

impl LaneWorktree {
    /// `lane-<short batch id>` under the workspace's worktrees, on
    /// `lane/<batch id>`.
    pub(crate) fn of(workspace: &Path, batch_id: Uuid) -> Self {
        let short: String = batch_id
            .as_simple()
            .to_string()
            .chars()
            .take(SHORT_BATCH_ID_LEN)
            .collect();
        Self {
            path: worktrees_dir(workspace).join(format!("lane-{short}")),
            branch: lane_branch(batch_id),
        }
    }
}

/// `git <args>` in `dir`, its trimmed stdout, or why it failed.
async fn git_stdout(args: &[&str], dir: &Path) -> Result<String, String> {
    let out = git_cmd(args, dir).await?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Create the integration branch at `base_sha`, or keep the one a restart
/// left. Created as the batch opens. Opening the lane thread tries again when
/// that fails.
pub(crate) async fn create_lane_branch(
    repo_root: &Path,
    branch: &str,
    base_sha: &str,
) -> Result<(), String> {
    let refname = format!("refs/heads/{branch}");
    match git_answer(&["rev-parse", "--verify", "--quiet", &refname], repo_root).await {
        GitAnswer::Yes => Ok(()),
        GitAnswer::No => git_stdout(&["branch", branch, base_sha], repo_root)
            .await
            .map(|_| ()),
        GitAnswer::Unknown => Err(format!("could not read whether {branch} exists")),
    }
}

/// Give the batch its lane worktree on the integration branch. The tree is
/// made only when git confirms none lives there. A live or unknown tree is
/// kept with its work, unless git names another branch checked out in it. It
/// is tagged for this workspace, so orphan recovery finds a lane thread a
/// restart cut off. It also gets the main checkout's `node_modules` and the
/// cached generated files, for `/harden`'s frontend suites.
pub(crate) async fn ensure_lane_worktree(
    repo_root: &Path,
    workspace: &Path,
    lane: &LaneWorktree,
    base_sha: &str,
) -> Result<(), String> {
    match worktree_liveness(&lane.path).await {
        GitAnswer::No => {
            create_lane_branch(repo_root, &lane.branch, base_sha).await?;
            clear_stranded_worktree_dir(repo_root, &lane.path).await;
            match worktree_add(repo_root, &lane.path, &[lane.branch.as_str()]).await {
                Ok(o) if o.status.success() => {}
                Ok(o) => return Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                Err(e) => return Err(e),
            }
        }
        // A detached HEAD or an unanswered read is `None`, and keeps the tree.
        GitAnswer::Yes | GitAnswer::Unknown => {
            if let Some(other) = worktree_current_branch(&lane.path)
                .await
                .filter(|branch| *branch != lane.branch)
            {
                return Err(format!(
                    "the lane worktree has {other} checked out, not {}",
                    lane.branch
                ));
            }
        }
    }
    let marker = lane.path.join(WORKTREE_WORKSPACE_MARKER);
    if let Err(e) = tokio::fs::write(&marker, workspace.to_string_lossy().as_bytes()).await {
        log!(
            "[LandingLane] Failed to tag {} for this workspace: {}",
            lane.path.display(),
            e
        );
    }
    add_paths_to_worktree_exclude(&lane.path, WORKTREE_EXCLUDE_PATHS).await;
    crate::engine::agent_session::link_node_modules(repo_root, &lane.path).await;
    // After node_modules: the SDK bundles build with its esbuild.
    crate::engine::agent_session::provision_generated_files(&lane.path).await;
    Ok(())
}

/// Check a clean lane-thread completion before the batch lands. Nothing may
/// be half done: no merge in progress, no unmerged path, no uncommitted
/// tracked edit. Each member has exactly one landing merge on the branch's
/// first-parent line, its snapshot as second parent. That also makes every
/// snapshot an ancestor of the head. Returns each member's landing merge, in
/// member order.
pub(crate) async fn verify_integration(
    worktree: &Path,
    base_sha: &str,
    members: &[LaneMember],
) -> Result<Vec<(Uuid, LandingMerge)>, String> {
    match git_answer(
        &["rev-parse", "--verify", "--quiet", "MERGE_HEAD"],
        worktree,
    )
    .await
    {
        GitAnswer::No => {}
        GitAnswer::Yes => return Err("a merge is still in progress".into()),
        GitAnswer::Unknown => return Err("git could not say whether a merge is open".into()),
    }
    let unmerged = git_answer_when_ok(&["diff", "--name-only", "--diff-filter=U"], worktree, |o| {
        !o.stdout.is_empty()
    })
    .await;
    match unmerged {
        GitAnswer::No => {}
        GitAnswer::Yes => return Err("some files are still unmerged".into()),
        GitAnswer::Unknown => return Err("git could not list unmerged files".into()),
    }
    let dirty = git_answer_when_ok(
        &["status", "--porcelain", "--untracked-files=no"],
        worktree,
        |o| !o.stdout.is_empty(),
    )
    .await;
    match dirty {
        GitAnswer::No => {}
        GitAnswer::Yes => return Err("the integration branch has uncommitted edits".into()),
        GitAnswer::Unknown => return Err("git could not read the worktree's status".into()),
    }
    let range = format!("{base_sha}..HEAD");
    let line = git_stdout(
        &["rev-list", "--first-parent", "--parents", &range],
        worktree,
    )
    .await?;
    let mut merges = Vec::with_capacity(members.len());
    for member in members {
        let found: Vec<LandingMerge> = line
            .lines()
            .filter_map(|l| landing_merge_of(l, &member.snapshot_sha))
            .collect();
        match found.as_slice() {
            [merge] => merges.push((member.change_id, merge.clone())),
            [] => {
                return Err(format!(
                    "change {} has no landing merge of {}",
                    member.change_id, member.snapshot_sha
                ))
            }
            more => {
                return Err(format!(
                    "change {} has {} landing merges, not one",
                    member.change_id,
                    more.len()
                ))
            }
        }
    }
    Ok(merges)
}

/// What landing the integration branch did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IntegrationLanding {
    /// `main` moved from `pre` to `post`.
    Landed { pre: String, post: String },
    /// `main` gained commits that conflict with the resolution. The catch-up
    /// merge was aborted and `main` did not move. `commits` summarize what
    /// `main` gained, one line each.
    CatchUpConflict {
        main_sha: String,
        commits: Vec<String>,
    },
    /// `origin` could not take the batch, so `main` did not move. The batch
    /// lands on a later try (ADR 0589).
    WaitingForOrigin(String),
}

/// Land a verified integration branch, under the merge lock, so nothing moves
/// `main` between the read and the move. When `main` moved since the branch
/// last held it, `main` merges in first. A clean catch-up lands. A
/// conflicting one is aborted and reported, never a failure.
///
/// Origin-first, `main` means `origin/main`. The batch is fetched against,
/// pushed, and only then moves local `main`. A rejected push catches up again.
/// Local-first, the caller pushes after.
pub(crate) async fn land_integration(
    locks: &MainLocks,
    repo_root: &Path,
    order: &LandingOrder,
    worktree: &Path,
) -> Result<IntegrationLanding, String> {
    let _merge = locks.lock_merge().await;
    let mut rejected = String::new();
    for _ in 0..MAX_PUSH_ATTEMPTS {
        let base = match landing_base(repo_root, order).await {
            Ok(base) => base,
            Err(BaseRefusal::MainUnreadable(e)) => return Err(e),
            Err(BaseRefusal::Origin(why)) => return Ok(IntegrationLanding::WaitingForOrigin(why)),
        };
        if let Some(conflict) = catch_up(worktree, &base.base).await? {
            return Ok(conflict);
        }
        let head = git_stdout(&["rev-parse", "HEAD"], worktree).await?;
        if base.origin.needs_push(&head) {
            match push_to_origin_main(repo_root, &head).await {
                PushOutcome::Pushed => {}
                PushOutcome::Rejected(e) => {
                    rejected = e;
                    continue;
                }
                PushOutcome::Failed(e) => {
                    return Ok(IntegrationLanding::WaitingForOrigin(format!(
                        "could not push to origin: {e}"
                    )))
                }
            }
        }
        if let Some(e) = move_local_main(locks, repo_root, &base, &head).await {
            return Err(e);
        }
        return Ok(IntegrationLanding::Landed {
            pre: base.base,
            post: head,
        });
    }
    Ok(IntegrationLanding::WaitingForOrigin(format!(
        "origin/main kept moving, and {MAX_PUSH_ATTEMPTS} pushes in a row were refused: {rejected}"
    )))
}

/// Merge `main` into the integration branch when the branch lacks it. `Some`
/// on a conflict, after aborting the merge.
async fn catch_up(worktree: &Path, main: &str) -> Result<Option<IntegrationLanding>, String> {
    match git_answer(&["merge-base", "--is-ancestor", main, "HEAD"], worktree).await {
        GitAnswer::Yes => Ok(None),
        GitAnswer::No => {
            let merged = git_cmd(
                &[
                    "merge",
                    "--no-ff",
                    "--no-edit",
                    "-m",
                    CATCH_UP_SUBJECT,
                    main,
                ],
                worktree,
            )
            .await?;
            if merged.status.success() {
                return Ok(None);
            }
            let range = format!("HEAD..{main}");
            let commits = git_stdout(&["log", "--format=%h %s", &range], worktree)
                .await
                .map(|log| log.lines().map(str::to_string).collect())
                .unwrap_or_default();
            if let Err(e) = git_stdout(&["merge", "--abort"], worktree).await {
                log!("[LandingLane] catch-up merge --abort: {}", e);
            }
            Ok(Some(IntegrationLanding::CatchUpConflict {
                main_sha: main.to_string(),
                commits,
            }))
        }
        GitAnswer::Unknown => {
            Err("git could not say whether the integration branch holds main".into())
        }
    }
}

/// `main` just before its last move, from its reflog. Recovery reads it to
/// report a landing a crash cut off. `None` when the reflog cannot say.
pub(crate) async fn main_before_last_move(repo_root: &Path) -> Option<String> {
    git_stdout(&["rev-parse", "--verify", "--quiet", "main@{1}"], repo_root)
        .await
        .ok()
        .filter(|sha| !sha.is_empty())
}

/// Remove the lane worktree and its branch. Both are the lane's own, made for
/// this batch, so a closed batch keeps neither. The tree moves aside before
/// it is deleted, like every worktree removal, and the cleanup worker
/// finishes a deletion that fails. Returns whether the branch is gone. Every
/// outcome is logged.
pub(crate) async fn remove_lane_worktree(repo_root: &Path, lane: &LaneWorktree) -> bool {
    if lane.path.exists() {
        // The branch goes with the tree only while the tree holds it. A lane
        // thread can check out another branch, which is not the lane's.
        let disposal =
            if worktree_current_branch(&lane.path).await.as_deref() == Some(lane.branch.as_str()) {
                BranchDisposal::Always
            } else {
                BranchDisposal::WhenMerged
            };
        match remove_worktree_and_optionally_delete_branch(&lane.path, None, disposal).await {
            Some(_) => log!(
                "[LandingLane] removed lane worktree {}",
                lane.path.display()
            ),
            None => log!(
                "[LandingLane] kept lane worktree {}: it could not be moved aside",
                lane.path.display()
            ),
        }
    }
    let refname = format!("refs/heads/{}", lane.branch);
    match git_answer(&["rev-parse", "--verify", "--quiet", &refname], repo_root).await {
        GitAnswer::No => return true,
        GitAnswer::Yes => {}
        GitAnswer::Unknown => {
            log!(
                "[LandingLane] could not read whether {} is gone",
                lane.branch
            );
            return false;
        }
    }
    match git_stdout(&["branch", "-D", &lane.branch], repo_root).await {
        Ok(_) => {
            log!("[LandingLane] deleted {}", lane.branch);
            true
        }
        Err(e) => {
            log!("[LandingLane] kept {}: {}", lane.branch, e);
            false
        }
    }
}

/// Retire a closed batch's lane worktree and branch, with the gate markers
/// its lane thread left on that branch. Returns whether the branch is gone.
pub(crate) async fn retire_lane_worktree(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    lane: &LaneWorktree,
) -> bool {
    let gone = remove_lane_worktree(repo_root, lane).await;
    consume_harden_marker(pool, repo_root, &lane.branch).await;
    consume_plan_marker(pool, repo_root, &lane.branch).await;
    gone
}

/// The listed lane worktrees a restart retires: those of a closed batch this
/// workspace opened. Several workspaces can share one checkout, so a lane
/// branch this workspace never opened is another's, and is never touched.
pub(crate) fn closed_lanes_opened_here<'a>(
    listed: &'a [(Uuid, LaneWorktree)],
    opened_here: &'a std::collections::HashSet<Uuid>,
    open: Option<Uuid>,
) -> impl Iterator<Item = &'a LaneWorktree> {
    listed
        .iter()
        .filter(move |(batch_id, _)| opened_here.contains(batch_id) && Some(*batch_id) != open)
        .map(|(_, lane)| lane)
}

/// Every lane branch in the repo, each with the worktree that has it checked
/// out, else where its worktree would be. `Err` when git could not list them:
/// the caller then removes nothing.
pub(crate) async fn lane_worktrees(
    repo_root: &Path,
    workspace: &Path,
) -> Result<Vec<(Uuid, LaneWorktree)>, String> {
    let lane_refs = format!("refs/heads/{LANE_BRANCH_PREFIX}");
    let refs = git_stdout(
        &["for-each-ref", "--format=%(refname:short)", &lane_refs],
        repo_root,
    )
    .await?;
    let checked_out =
        parse_worktree_list(&git_stdout(&["worktree", "list", "--porcelain"], repo_root).await?);
    Ok(refs
        .lines()
        .filter_map(|branch| {
            let batch_id = lane_branch_batch(branch)?;
            let mut lane = LaneWorktree::of(workspace, batch_id);
            if let Some(path) = checked_out.get(branch) {
                lane.path = path.clone();
            }
            Some((batch_id, lane))
        })
        .collect())
}

#[cfg(test)]
#[path = "integration_tests.rs"]
mod tests;
