use crate::engine::git_ops::{
    branch_head_sha, ff_main_to_locked, git_answer, git_cmd, landing_base, push_to_origin_main,
    BaseRefusal, GitAnswer, LandingBase, LandingOrder, MainLocks, PushOutcome, ORIGIN_MAIN,
};
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub(crate) struct LandingInput {
    pub(crate) change_id: Uuid,
    pub(crate) snapshot_sha: String,
    pub(crate) subject: String,
    pub(crate) body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LandingMerge {
    pub(crate) first_parent: String,
    pub(crate) merge_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LandingStep {
    Landed(LandingMerge),
    /// INVARIANT: Its snapshot is already in the tip's history, so there is nothing to
    /// merge. Carries its landing merge when an earlier pass made one: a push
    /// can land on `origin` and still report a failure.
    AlreadyOnMain(Option<LandingMerge>),
    SetAside,
}

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
            log_legacy!(
                "[LandingLane] commit-tree for {} failed: {}",
                input.change_id,
                String::from_utf8_lossy(&o.stderr).trim()
            );
            LandingStep::SetAside
        }
        Err(e) => {
            log_legacy!(
                "[LandingLane] commit-tree for {} failed: {}",
                input.change_id,
                e
            );
            LandingStep::SetAside
        }
    }
}

async fn earlier_landing(repo_root: &Path, input: &LandingInput) -> Option<LandingMerge> {
    find_landing_merge(repo_root, &input.snapshot_sha)
        .await
        .unwrap_or_else(|e| {
            log_legacy!(
                "[LandingLane] could not look up the landing merge of {}: {}",
                input.change_id,
                e
            );
            None
        })
}

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

#[derive(Debug)]
pub(crate) struct Publication {
    pub(crate) base: String,
    pub(crate) chain: LandingChain,
    pub(crate) failed: Option<String>,
}

impl Publication {
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

#[derive(Debug)]
pub(crate) enum CleanPass {
    Published(Publication),
    WaitingForOrigin(String),
}

pub(super) const MAX_PUSH_ATTEMPTS: usize = 3;

pub(crate) async fn publish_clean_pass(
    remote: &[&super::LaneMember],
    locks: &MainLocks,
    repo_root: &Path,
    order: &LandingOrder,
    inputs: &[LandingInput],
) -> Result<CleanPass, String> {
    let _merge = locks.lock_merge().await;
    let mut rejected = String::new();
    for _ in 0..MAX_PUSH_ATTEMPTS {
        let order = if remote.is_empty() {
            order
        } else {
            &LandingOrder::OriginFirst
        };
        let base = match landing_base(repo_root, order).await {
            Ok(base) => base,
            Err(BaseRefusal::MainUnreadable(e)) => return Err(e),
            Err(BaseRefusal::Origin(why)) => return Ok(CleanPass::WaitingForOrigin(why)),
        };
        let chain = build_landing_chain(repo_root, &base.base, inputs).await;
        let landed: Vec<_> = remote
            .iter()
            .copied()
            .filter(|member| {
                chain.steps.iter().any(|(id, step)| {
                    *id == member.change_id
                        && matches!(step, LandingStep::Landed(_) | LandingStep::AlreadyOnMain(_))
                })
            })
            .collect();
        if !landed.is_empty() {
            if let Err(error) = super::remote::land(repo_root, &chain.tip, &landed).await {
                rejected = error;
                continue;
            }
        } else if base.origin.needs_push(&chain.tip) {
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
        Err(e) if branch_head_sha(repo_root, "main").await.as_deref() == Some(tip) => {
            log_legacy!("[LandingLane] main moved to {} but: {}", tip, e);
            None
        }
        // INVARIANT: `origin` holds the landing, so it happened. Local `main` follows
        // on the next pass.
        Err(e) if base.origin.origin_first() => {
            log_legacy!(
                "[LandingLane] origin/main is at {} but local main is not: {}",
                tip,
                e
            );
            None
        }
        Err(e) => Some(e.to_string()),
    }
}

const LANDING_SEARCH_DEPTH: &str = "2000";

/// INVARIANT: The landing merge on `main`'s first-parent line whose second parent is
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
