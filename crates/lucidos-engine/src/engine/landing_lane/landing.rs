//! Landing merges, built without a worktree.
//!
//! Each member lands as its own merge commit: `git merge-tree --write-tree`
//! computes the merged tree in memory, and `git commit-tree` records it with
//! the tip as first parent and the snapshot as second. The tip then moves on
//! to that commit, so `git log --first-parent main` reads one landing merge
//! per change. Merge attributes apply here as in any merge.

use crate::engine::git_ops::{
    branch_head_sha, ff_main_to_locked, git_answer, git_cmd, landing_base, push_to_origin_main,
    BaseRefusal, GitAnswer, LandingBase, LandingOrder, MainLocks, PushOutcome, ORIGIN_MAIN,
};
use std::path::Path;
use uuid::Uuid;

/// One member to land, in lane order.
#[derive(Debug, Clone)]
pub(crate) struct LandingInput {
    pub(crate) change_id: Uuid,
    pub(crate) snapshot_sha: String,
    /// The landing merge's subject line.
    pub(crate) subject: String,
    /// The landing merge's body: which change and branch it lands.
    pub(crate) body: String,
}

/// A change's own merge commit on `main`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LandingMerge {
    /// `main` before this change landed.
    pub(crate) first_parent: String,
    pub(crate) merge_sha: String,
}

/// What a clean pass did with one member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LandingStep {
    Landed(LandingMerge),
    /// Its snapshot is already in the tip's history, so there is nothing to
    /// merge. Carries its landing merge when an earlier pass made one: a push
    /// can land on `origin` and still report a failure.
    AlreadyOnMain(Option<LandingMerge>),
    /// It would not merge cleanly onto the tip, or git could not say.
    SetAside,
}

/// The tip a pass would move `main` to, and each member's step in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LandingChain {
    pub(crate) tip: String,
    pub(crate) steps: Vec<(Uuid, LandingStep)>,
}

impl LandingChain {
    pub(crate) fn lands_anything(&self) -> bool {
        self.steps
            .iter()
            .any(|(_, step)| matches!(step, LandingStep::Landed(_)))
    }
}

/// Chain one landing merge per member onto `base`, in order. A member that
/// would conflict is set aside and the chain goes on without it. Nothing
/// here moves a ref.
pub(crate) async fn build_landing_chain(
    repo_root: &Path,
    base: &str,
    inputs: &[LandingInput],
) -> LandingChain {
    let mut tip = base.to_string();
    let mut steps = Vec::with_capacity(inputs.len());
    for input in inputs {
        let step = land_one(repo_root, &tip, input).await;
        if let LandingStep::Landed(merge) = &step {
            tip = merge.merge_sha.clone();
        }
        steps.push((input.change_id, step));
    }
    LandingChain { tip, steps }
}

async fn land_one(repo_root: &Path, tip: &str, input: &LandingInput) -> LandingStep {
    let snapshot = input.snapshot_sha.as_str();
    match git_answer(&["merge-base", "--is-ancestor", snapshot, tip], repo_root).await {
        GitAnswer::Yes => {
            return LandingStep::AlreadyOnMain(earlier_landing(repo_root, input).await)
        }
        GitAnswer::No => {}
        GitAnswer::Unknown => return LandingStep::SetAside,
    }
    let Some(tree) = merged_tree(repo_root, tip, snapshot).await else {
        return LandingStep::SetAside;
    };
    let commit = git_cmd(
        &[
            "commit-tree",
            &tree,
            "-p",
            tip,
            "-p",
            snapshot,
            "-m",
            &input.subject,
            "-m",
            &input.body,
        ],
        repo_root,
    )
    .await;
    match commit {
        Ok(o) if o.status.success() => {
            let merge_sha = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if merge_sha.is_empty() {
                return LandingStep::SetAside;
            }
            LandingStep::Landed(LandingMerge {
                first_parent: tip.to_string(),
                merge_sha,
            })
        }
        Ok(o) => {
            log!(
                "[LandingLane] commit-tree for {} failed: {}",
                input.change_id,
                String::from_utf8_lossy(&o.stderr).trim()
            );
            LandingStep::SetAside
        }
        Err(e) => {
            log!(
                "[LandingLane] commit-tree for {} failed: {}",
                input.change_id,
                e
            );
            LandingStep::SetAside
        }
    }
}

/// The landing merge an earlier pass made for `input`, if any. A git error
/// reads as none: the member is on `main` either way.
async fn earlier_landing(repo_root: &Path, input: &LandingInput) -> Option<LandingMerge> {
    find_landing_merge(repo_root, &input.snapshot_sha)
        .await
        .unwrap_or_else(|e| {
            log!(
                "[LandingLane] could not look up the landing merge of {}: {}",
                input.change_id,
                e
            );
            None
        })
}

/// The tree a clean merge of `theirs` into `ours` would have. `None` on a
/// conflict, and when git could not answer.
async fn merged_tree(repo_root: &Path, ours: &str, theirs: &str) -> Option<String> {
    let out = git_cmd(
        &["merge-tree", "--write-tree", "--no-messages", ours, theirs],
        repo_root,
    )
    .await
    .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(str::trim)
        .filter(|tree| !tree.is_empty())
        .map(str::to_string)
}

/// What a clean pass did to `main`.
#[derive(Debug)]
pub(crate) struct Publication {
    /// What the pass built on: `main`, or `origin/main` when it was ahead.
    pub(crate) base: String,
    pub(crate) chain: LandingChain,
    /// Why `main` stayed at `base` although the chain landed something. Every
    /// `Landed` step then landed nothing.
    pub(crate) failed: Option<String>,
}

impl Publication {
    /// `main` once the pass is done: the chain's tip when the pass moved it,
    /// else what the pass built on. A batch the pass opens starts here.
    pub(crate) fn main_now(&self) -> &str {
        if self.moved_main() {
            &self.chain.tip
        } else {
            &self.base
        }
    }

    pub(crate) fn moved_main(&self) -> bool {
        self.failed.is_none() && self.chain.lands_anything()
    }
}

/// What a clean pass came to.
#[derive(Debug)]
pub(crate) enum CleanPass {
    Published(Publication),
    /// `origin` could not take the pass, so nothing moved and every member
    /// waits. The reason is shown to the user.
    WaitingForOrigin(String),
}

/// How often a pass rebuilds after `origin` refused its push.
pub(super) const MAX_PUSH_ATTEMPTS: usize = 3;

/// Build the chain onto `main` and move `main` to its tip.
///
/// Origin-first, `main` means `origin/main` (ADR 0589). The pass fetches,
/// builds on the newer line, and pushes before it moves local `main`. A
/// rejected push rebuilds on the new tip. Local-first, the caller pushes after.
/// The merge lock is held for the whole pass, and every mover of `main` takes
/// it first (`MainLocks::lock_main`).
/// `Err` only when `main` could not be read, and then nothing happened.
pub(crate) async fn publish_clean_pass(
    locks: &MainLocks,
    repo_root: &Path,
    order: &LandingOrder,
    inputs: &[LandingInput],
) -> Result<CleanPass, String> {
    let _merge = locks.lock_merge().await;
    let mut rejected = String::new();
    for _ in 0..MAX_PUSH_ATTEMPTS {
        let base = match landing_base(repo_root, order).await {
            Ok(base) => base,
            Err(BaseRefusal::MainUnreadable(e)) => return Err(e),
            Err(BaseRefusal::Origin(why)) => return Ok(CleanPass::WaitingForOrigin(why)),
        };
        let chain = build_landing_chain(repo_root, &base.base, inputs).await;
        if base.origin.needs_push(&chain.tip) {
            match push_to_origin_main(repo_root, &chain.tip).await {
                PushOutcome::Pushed => {}
                PushOutcome::Rejected(e) => {
                    rejected = e;
                    continue;
                }
                PushOutcome::Failed(e) => {
                    return Ok(CleanPass::WaitingForOrigin(format!(
                        "could not push to origin: {e}"
                    )))
                }
            }
        }
        let failed = move_local_main(locks, repo_root, &base, &chain.tip).await;
        return Ok(CleanPass::Published(Publication {
            base: base.base,
            chain,
            failed,
        }));
    }
    Ok(CleanPass::WaitingForOrigin(format!(
        "origin/main kept moving, and {MAX_PUSH_ATTEMPTS} pushes in a row were refused: {rejected}"
    )))
}

/// Fast-forward local `main` to `tip` once `origin` holds it. `Some` only when
/// the landing did not happen: `origin` does not hold it, and `main` did not
/// move.
pub(super) async fn move_local_main(
    locks: &MainLocks,
    repo_root: &Path,
    base: &LandingBase,
    tip: &str,
) -> Option<String> {
    if tip == base.local {
        return None;
    }
    let held = locks.lock_repo().await;
    let moved = ff_main_to_locked(&held, repo_root, tip, &base.local).await;
    match moved {
        Ok(_) => None,
        // The ref moved but the repo-root tree did not follow. The landing
        // happened, so it is reported as one, and the error is logged.
        Err(e) if branch_head_sha(repo_root, "main").await.as_deref() == Some(tip) => {
            log!("[LandingLane] main moved to {} but: {}", tip, e);
            None
        }
        // `origin` holds the landing, so it happened. Local `main` follows
        // on the next pass.
        Err(e) if base.origin.origin_first() => {
            log!(
                "[LandingLane] origin/main is at {} but local main is not: {}",
                tip,
                e
            );
            None
        }
        Err(e) => Some(e.to_string()),
    }
}

/// How far back recovery looks for a landing merge on `main`.
const LANDING_SEARCH_DEPTH: &str = "2000";

/// The landing merge on `main`'s first-parent line whose second parent is
/// `snapshot`, for a crash between moving `main` and announcing the landing,
/// and for a push whose answer was lost.
/// `origin/main` is searched too: a crash can fall between the push and the
/// local move. `Err` when git could not answer: recovery then leaves the
/// change queued.
pub(crate) async fn find_landing_merge(
    repo_root: &Path,
    snapshot: &str,
) -> Result<Option<LandingMerge>, String> {
    let mut args = vec![
        "rev-list",
        "--first-parent",
        "--merges",
        "--parents",
        "-n",
        LANDING_SEARCH_DEPTH,
        "main",
    ];
    if branch_head_sha(repo_root, ORIGIN_MAIN).await.is_some() {
        args.push(ORIGIN_MAIN);
    }
    let out = git_cmd(&args, repo_root).await?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| landing_merge_of(line, snapshot)))
}

/// Parse one `rev-list --parents` line: the commit, then its parents. A
/// landing merge of `snapshot` has it as second parent.
pub(super) fn landing_merge_of(line: &str, snapshot: &str) -> Option<LandingMerge> {
    let mut shas = line.split_whitespace();
    let merge_sha = shas.next()?;
    let first_parent = shas.next()?;
    (shas.next()? == snapshot).then(|| LandingMerge {
        first_parent: first_parent.to_string(),
        merge_sha: merge_sha.to_string(),
    })
}

#[cfg(test)]
impl CleanPass {
    pub(crate) fn published(self) -> Publication {
        match self {
            CleanPass::Published(publication) => publication,
            CleanPass::WaitingForOrigin(why) => panic!("the pass waits for origin: {why}"),
        }
    }
}

#[cfg(test)]
#[path = "landing_tests.rs"]
mod tests;
