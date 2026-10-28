//! Talking to a repository's `origin` remote: whether it has one, fetching its
//! `main`, and pushing to it. The landing lane builds on these (ADR 0589).

use super::{
    git_answer, git_cmd_await_lock_file, git_cmd_env_timeout, GitAnswer, MainLocks,
    WorkspaceRepoLock,
};
use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

/// The remote-tracking ref a fetch of `origin`'s `main` updates.
pub(crate) const ORIGIN_MAIN: &str = "refs/remotes/origin/main";

/// A fetch of one branch. Longer than [`super::GIT_TIMEOUT`]: it crosses the
/// network.
const FETCH_TIMEOUT: Duration = Duration::from_secs(90);

/// A push of `main`, which can carry a whole batch.
const PUSH_TIMEOUT: Duration = Duration::from_secs(120);

/// A credential prompt would hang the engine until the timeout.
const NO_PROMPT: [(&str, &str); 1] = [("GIT_TERMINAL_PROMPT", "0")];

/// Whether the repository has an `origin` remote. `Unknown` when git could not
/// be asked.
pub(crate) async fn origin_remote(repo_root: &Path) -> GitAnswer {
    git_answer(&["remote", "get-url", "origin"], repo_root).await
}

/// Fetch `origin`'s `main` into [`ORIGIN_MAIN`] and return its tip. `Ok(None)`
/// when `origin` has no `main` yet. `Err` when `origin` could not be reached.
pub(crate) async fn fetch_origin_main(repo_root: &Path) -> Result<Option<String>, String> {
    let refspec = format!("+refs/heads/main:{ORIGIN_MAIN}");
    let out = run_remote(
        &["fetch", "--no-tags", "origin", &refspec],
        repo_root,
        FETCH_TIMEOUT,
    )
    .await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if stderr.contains("couldn't find remote ref") {
            return Ok(None);
        }
        return Err(stderr);
    }
    super::branch_head_sha(repo_root, ORIGIN_MAIN)
        .await
        .map(Some)
        .ok_or_else(|| format!("could not read {ORIGIN_MAIN} after the fetch"))
}

/// How a push to `origin`'s `main` went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PushOutcome {
    Pushed,
    /// `origin` moved since the last fetch, so the push was not a fast-forward.
    Rejected(String),
    /// `origin` could not be reached, refused the credential, or timed out.
    Failed(String),
}

/// Push `sha` to `origin`'s `main`, as a fast-forward only. A successful push
/// also moves [`ORIGIN_MAIN`] and tells the gateway, which nudges the
/// gateways it is paired with (`landing_gateway`).
pub(crate) async fn push_to_origin_main(repo_root: &Path, sha: &str) -> PushOutcome {
    let refspec = format!("{sha}:refs/heads/main");
    match run_remote(
        &["push", "--porcelain", "origin", &refspec],
        repo_root,
        PUSH_TIMEOUT,
    )
    .await
    {
        Ok(o) if o.status.success() => {
            crate::landing_gateway::report_main_landed();
            PushOutcome::Pushed
        }
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            if push_was_rejected(&stdout, &stderr) {
                PushOutcome::Rejected(stderr)
            } else {
                PushOutcome::Failed(stderr)
            }
        }
        Err(e) => PushOutcome::Failed(e),
    }
}

/// Whether a failed push was refused because `origin` moved: git names a
/// non-fast-forward, or a tip it has not fetched. A hook or permission refusal
/// is not this: fetching again would not help.
pub(super) fn push_was_rejected(stdout: &str, stderr: &str) -> bool {
    const MOVED: [&str; 3] = ["non-fast-forward", "fetch first", "stale info"];
    MOVED
        .iter()
        .any(|reason| stdout.contains(reason) || stderr.contains(reason))
}

/// Whether a landing goes to `origin` before local `main` (ADR 0589). Only a
/// gateway that holds a pairing lands origin-first (`landing_gateway`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LandingOrder {
    OriginFirst,
    /// Land locally, then push in the background.
    LocalFirst,
    /// The gateway did not say whether it is paired. A landing on a repository
    /// with an `origin` waits, as origin-first would.
    Unknown(String),
}

/// Where `origin` stood when a landing read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OriginTip {
    /// The repository has no `origin`, so `main` lands locally.
    NoOrigin,
    /// The landing is local-first, so `origin` was not read.
    LocalFirst,
    /// `origin` has no `main` yet. The first push creates it.
    Empty,
    At(String),
}

impl OriginTip {
    /// Whether moving `main` to `tip` needs a push first.
    pub(crate) fn needs_push(&self, tip: &str) -> bool {
        match self {
            OriginTip::NoOrigin | OriginTip::LocalFirst => false,
            OriginTip::Empty => true,
            OriginTip::At(sha) => sha != tip,
        }
    }

    /// Whether `origin` holds a landing once its push went.
    pub(crate) fn origin_first(&self) -> bool {
        matches!(self, OriginTip::Empty | OriginTip::At(_))
    }
}

/// What a landing builds on: the newer of local `main` and `origin/main`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LandingBase {
    /// Local `main` as read.
    pub(crate) local: String,
    /// The commit the landing builds on.
    pub(crate) base: String,
    pub(crate) origin: OriginTip,
}

/// Why a landing could not read its base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BaseRefusal {
    /// Local `main` could not be read.
    MainUnreadable(String),
    /// `origin` could not be reached, or it and local `main` have diverged, or
    /// the gateway did not say whether it is paired. The landing waits.
    Origin(String),
}

/// Fetch `origin` when the landing is origin-first, and pick the base it builds
/// on. The caller holds the merge lock, so local `main` stays put.
///
/// Local `main` ahead of `origin` is a base too: the landing's push then
/// carries those commits. Two diverged lines are refused, never merged.
pub(crate) async fn landing_base(
    repo_root: &Path,
    order: &LandingOrder,
) -> Result<LandingBase, BaseRefusal> {
    let local = super::branch_head_sha(repo_root, "main")
        .await
        .ok_or_else(|| BaseRefusal::MainUnreadable("could not read main".to_string()))?;
    let at = |base: &str, origin: OriginTip| LandingBase {
        local: local.clone(),
        base: base.to_string(),
        origin,
    };
    match origin_remote(repo_root).await {
        GitAnswer::No => return Ok(at(&local, OriginTip::NoOrigin)),
        GitAnswer::Yes => {}
        GitAnswer::Unknown => {
            return Err(BaseRefusal::Origin(
                "git could not say whether the repository has an origin remote".to_string(),
            ))
        }
    }
    match order {
        LandingOrder::OriginFirst => {}
        LandingOrder::LocalFirst => return Ok(at(&local, OriginTip::LocalFirst)),
        LandingOrder::Unknown(why) => return Err(BaseRefusal::Origin(why.clone())),
    }
    let tip = match fetch_origin_main(repo_root).await {
        Ok(Some(tip)) => tip,
        Ok(None) => return Ok(at(&local, OriginTip::Empty)),
        Err(e) => {
            return Err(BaseRefusal::Origin(format!(
                "could not fetch from origin: {e}"
            )))
        }
    };
    let base = if tip == local {
        local.clone()
    } else if is_ancestor(repo_root, &local, &tip).await? {
        tip.clone()
    } else if is_ancestor(repo_root, &tip, &local).await? {
        local.clone()
    } else {
        return Err(BaseRefusal::Origin(format!(
            "local main ({}) and origin/main ({}) have diverged. Bring them back to one line by hand",
            short(&local),
            short(&tip)
        )));
    };
    Ok(at(&base, OriginTip::At(tip)))
}

/// `origin`'s `main` as `git ls-remote` reports it, without fetching.
/// `Ok(None)` when `origin` has no `main`. `Err` when it could not be reached.
pub(crate) async fn ls_remote_main(repo_root: &Path) -> Result<Option<String>, String> {
    let out = run_remote(
        &["ls-remote", "origin", "refs/heads/main"],
        repo_root,
        FETCH_TIMEOUT,
    )
    .await?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string))
}

/// How following `origin`'s `main` went (ADR 0589).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FollowOutcome {
    /// The repository has no `origin`, so there is nothing to follow.
    NoOrigin,
    /// `origin` did not answer.
    Unreachable(String),
    /// `origin` answered, and local `main` already has its tip.
    UpToDate,
    /// Local `main` fast-forwarded from `from` to `to`.
    Followed { from: String, to: String },
    /// `origin` answered, but local `main` could not fast-forward to it.
    Refused(String),
}

/// Bring local `main` up to `origin`'s, as a fast-forward only. A cheap
/// `ls-remote` decides whether to fetch at all.
///
/// The merge lock spans the fetch and the move, as in a landing pass, so no
/// landing reads `main` in between. Diverged lines are refused, never merged.
pub(crate) async fn follow_origin_main(locks: &MainLocks, repo_root: &Path) -> FollowOutcome {
    match origin_remote(repo_root).await {
        GitAnswer::No => return FollowOutcome::NoOrigin,
        GitAnswer::Yes => {}
        GitAnswer::Unknown => {
            return FollowOutcome::Unreachable(
                "git could not say whether the repository has an origin remote".to_string(),
            )
        }
    }
    let remote = match ls_remote_main(repo_root).await {
        Ok(Some(remote)) => remote,
        Ok(None) => return FollowOutcome::UpToDate,
        Err(e) => return FollowOutcome::Unreachable(e),
    };
    if let Some(local) = super::branch_head_sha(repo_root, "main").await {
        let known = git_answer(&["merge-base", "--is-ancestor", &remote, &local], repo_root).await;
        if known == GitAnswer::Yes {
            return FollowOutcome::UpToDate;
        }
    }
    let _merge = locks.lock_merge().await;
    let tip = match fetch_origin_main(repo_root).await {
        Ok(Some(tip)) => tip,
        Ok(None) => return FollowOutcome::UpToDate,
        Err(e) => return FollowOutcome::Unreachable(e),
    };
    let held = locks.lock_repo().await;
    let Some(local) = super::branch_head_sha(repo_root, "main").await else {
        return FollowOutcome::Refused("could not read main".to_string());
    };
    match origin_is_ahead(repo_root, &local, &tip).await {
        Ok(false) => FollowOutcome::UpToDate,
        Ok(true) => match fast_forward_main(&held, repo_root, &local, &tip).await {
            Ok(()) => FollowOutcome::Followed {
                from: local,
                to: tip,
            },
            Err(e) => FollowOutcome::Refused(e),
        },
        Err(why) => FollowOutcome::Refused(why),
    }
}

/// Whether `tip` is strictly ahead of `local` on one line. `false` when local
/// `main` already holds it. `Err` when the two have diverged.
async fn origin_is_ahead(repo_root: &Path, local: &str, tip: &str) -> Result<bool, String> {
    let ancestor = |older, newer| async move {
        match is_ancestor(repo_root, older, newer).await {
            Ok(yes) => Ok(yes),
            Err(BaseRefusal::Origin(why) | BaseRefusal::MainUnreadable(why)) => Err(why),
        }
    };
    if tip == local || ancestor(tip, local).await? {
        return Ok(false);
    }
    if ancestor(local, tip).await? {
        return Ok(true);
    }
    Err(format!(
        "local main ({}) and origin/main ({}) have diverged. Bring them back to one line by hand",
        short(local),
        short(tip)
    ))
}

/// Move `main` from `from` to `to`, a known fast-forward. A checkout on `main`
/// merges `--ff-only`: git keeps an uncommitted edit, or refuses when the new
/// commits touch it. Any other checkout keeps its HEAD and tree, and only the
/// ref moves.
async fn fast_forward_main(
    _held: &WorkspaceRepoLock,
    repo_root: &Path,
    from: &str,
    to: &str,
) -> Result<(), String> {
    let head = super::git_cmd(&["symbolic-ref", "-q", "HEAD"], repo_root).await?;
    let on_main = String::from_utf8_lossy(&head.stdout).trim() == "refs/heads/main";
    let out = if on_main {
        git_cmd_await_lock_file(&["merge", "--ff-only", "--no-edit", to], repo_root).await?
    } else {
        super::git_cmd(&["update-ref", "refs/heads/main", to, from], repo_root).await?
    };
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "could not fast-forward main to origin/main ({}): {}",
            short(to),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

async fn is_ancestor(repo_root: &Path, older: &str, newer: &str) -> Result<bool, BaseRefusal> {
    match git_answer(&["merge-base", "--is-ancestor", older, newer], repo_root).await {
        GitAnswer::Yes => Ok(true),
        GitAnswer::No => Ok(false),
        GitAnswer::Unknown => Err(BaseRefusal::Origin(
            "git could not compare main with origin/main".to_string(),
        )),
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.floor_char_boundary(8)]
}

async fn run_remote(
    args: &[&str],
    repo_root: &Path,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let envs: Vec<(&str, &OsStr)> = NO_PROMPT
        .iter()
        .map(|(k, v)| (*k, OsStr::new(*v)))
        .collect();
    git_cmd_env_timeout(args, repo_root, &envs, timeout).await
}
