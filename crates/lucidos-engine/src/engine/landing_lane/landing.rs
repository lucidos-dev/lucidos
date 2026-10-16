//! Landing merges, built without a worktree.
//!
//! Each member lands as its own merge commit: `git merge-tree --write-tree`
//! computes the merged tree in memory, and `git commit-tree` records it with
//! the tip as first parent and the snapshot as second. The tip then moves on
//! to that commit, so `git log --first-parent main` reads one landing merge
//! per change. Merge attributes apply here as in any merge.

use crate::engine::git_ops::{
    branch_head_sha, ff_main_to_locked, git_answer, git_cmd, GitAnswer, MainLocks,
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
    /// merge.
    AlreadyOnMain,
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
        GitAnswer::Yes => return LandingStep::AlreadyOnMain,
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
    /// `main` when the pass read it.
    pub(crate) base: String,
    pub(crate) chain: LandingChain,
    /// Why `main` stayed at `base` although the chain landed something. Every
    /// `Landed` step then landed nothing.
    pub(crate) failed: Option<String>,
}

impl Publication {
    /// `main` once the pass is done: the chain's tip when the pass moved it,
    /// else `main` as the pass read it. A batch the pass opens starts here.
    pub(crate) fn main_now(&self) -> &str {
        if self.failed.is_none() && self.chain.lands_anything() {
            &self.chain.tip
        } else {
            &self.base
        }
    }
}

/// Build the chain onto `main` and move `main` to its tip.
///
/// The merge lock is held for the whole pass. Every mover of `main` takes it
/// first (`MainLocks::lock_main`), so nothing moves `main` between the read
/// and the move. The move keeps `update-ref`'s old-value guard all the same.
/// `Err` only when `main` could not be read, and then nothing happened.
pub(crate) async fn publish_clean_pass(
    locks: &MainLocks,
    repo_root: &Path,
    inputs: &[LandingInput],
) -> Result<Publication, String> {
    let _merge = locks.lock_merge().await;
    let base = branch_head_sha(repo_root, "main")
        .await
        .ok_or_else(|| "could not read main".to_string())?;
    let chain = build_landing_chain(repo_root, &base, inputs).await;
    if !chain.lands_anything() {
        return Ok(Publication {
            base,
            chain,
            failed: None,
        });
    }
    let held = locks.lock_repo().await;
    let moved = ff_main_to_locked(&held, repo_root, &chain.tip, &base).await;
    let failed = match moved {
        Ok(_) => None,
        // The ref moved but the repo-root tree did not follow. The landing
        // happened, so it is reported as one, and the error is logged.
        Err(e) if branch_head_sha(repo_root, "main").await.as_deref() == Some(&chain.tip) => {
            log!("[LandingLane] main moved to {} but: {}", chain.tip, e);
            None
        }
        Err(e) => Some(e.to_string()),
    };
    Ok(Publication {
        base,
        chain,
        failed,
    })
}

/// How far back recovery looks for a landing merge on `main`.
const LANDING_SEARCH_DEPTH: &str = "2000";

/// The landing merge on `main`'s first-parent line whose second parent is
/// `snapshot`, for a crash between moving `main` and announcing the landing.
/// `Err` when git could not answer: recovery then leaves the change queued.
pub(crate) async fn find_landing_merge(
    repo_root: &Path,
    snapshot: &str,
) -> Result<Option<LandingMerge>, String> {
    let out = git_cmd(
        &[
            "rev-list",
            "--first-parent",
            "--merges",
            "--parents",
            "-n",
            LANDING_SEARCH_DEPTH,
            "main",
        ],
        repo_root,
    )
    .await?;
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
#[path = "landing_tests.rs"]
mod tests;
