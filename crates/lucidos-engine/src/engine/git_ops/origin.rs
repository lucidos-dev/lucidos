use super::{
    git_answer, git_cmd_await_lock_file, git_cmd_env_timeout, GitAnswer, MainLocks,
    WorkspaceRepoLock,
};
use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

pub(crate) const ORIGIN_MAIN: &str = "refs/remotes/origin/main";

const FETCH_TIMEOUT: Duration = Duration::from_secs(90);

const PUSH_TIMEOUT: Duration = Duration::from_secs(120);

const NO_PROMPT: [(&str, &str); 1] = [("GIT_TERMINAL_PROMPT", "0")];

pub(crate) async fn origin_remote(repo_root: &Path) -> GitAnswer {
    git_answer(&["remote", "get-url", "origin"], repo_root).await
}

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

/// INVARIANT: Verify receive-pack can create and delete queue refs. Use an object origin
/// already holds and a reserved, non-change ref that intake will ignore.
pub(crate) async fn probe_origin_landing_queue(repo_root: &Path) -> Result<(), String> {
    let tip = fetch_origin_main(repo_root)
        .await
        .map_err(|e| format!("Could not read origin/main for the landing queue probe: {e}"))?
        .ok_or("The landing queue needs an origin with a main branch.")?;
    let probe = format!("refs/lucidos/queue/probe-{}", uuid::Uuid::new_v4());
    push_origin_refs(repo_root, &[&format!("{tip}:{probe}")])
        .await
        .map_err(|e| format!("Could not verify origin accepts landing queue refs: {e}"))?;
    let visible = list_origin_refs(repo_root, &probe).await;
    push_origin_refs(repo_root, &[&format!(":{probe}")])
        .await
        .map_err(|e| format!("Could not remove the landing queue probe: {e}"))?;
    if !visible?
        .iter()
        .any(|(name, oid)| name == &probe && oid == &tip)
    {
        return Err("The ref written to origin is not visible when reading origin.".into());
    }
    Ok(())
}

pub(crate) async fn push_origin_refs(repo_root: &Path, updates: &[&str]) -> Result<(), String> {
    let mut args = vec![
        "push",
        "--atomic",
        "--no-follow-tags",
        "--porcelain",
        "origin",
    ];
    args.extend(updates);
    let output = run_remote(&args, repo_root, PUSH_TIMEOUT).await?;
    if output.status.success() {
        return Ok(());
    }
    Err(push_refusal(
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    ))
}

pub(crate) async fn list_origin_refs(
    repo_root: &Path,
    pattern: &str,
) -> Result<Vec<(String, String)>, String> {
    let output = run_remote(
        &["ls-remote", "--refs", "origin", pattern],
        repo_root,
        FETCH_TIMEOUT,
    )
    .await?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    std::str::from_utf8(&output.stdout)
        .map_err(|e| e.to_string())?
        .lines()
        .map(|line| {
            let (oid, name) = line
                .split_once('\t')
                .ok_or("Malformed origin ref listing")?;
            Ok((name.to_string(), oid.to_string()))
        })
        .collect()
}

pub(crate) async fn fetch_origin_object(repo_root: &Path, oid: &str) -> Result<(), String> {
    let output = run_remote(
        &["fetch", "--no-tags", "--no-write-fetch-head", "origin", oid],
        repo_root,
        FETCH_TIMEOUT,
    )
    .await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PushOutcome {
    Pushed,
    Rejected(String),
    Failed(String),
}

static ORIGIN_PUSH_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// INVARIANT: Push `sha` to `origin`'s `main`, as a fast-forward only. A successful push
/// also moves [`ORIGIN_MAIN`] and tells the gateway, which nudges the
/// gateways it is paired with (`landing_gateway`).
///
/// A push that queued behind a newer one is refused as a rewind. A fresh
/// fetch then decides: a `sha` that `origin`'s `main` holds counts as pushed.
pub(crate) async fn push_to_origin_main(repo_root: &Path, sha: &str) -> PushOutcome {
    let _push_guard = ORIGIN_PUSH_MUTEX.lock().await;
    match push_sha(repo_root, sha).await {
        PushOutcome::Rejected(reason) if origin_holds(repo_root, sha).await => {
            log_legacy!(
                "[Changes] origin's main already holds {sha}, so nothing to push ({reason})"
            );
            PushOutcome::Pushed
        }
        outcome => outcome,
    }
}

/// INVARIANT: Whether `origin`'s `main`, fetched now, contains `sha`. The tracking ref
/// alone can be stale, and a wrong yes would claim a push that never happened.
async fn origin_holds(repo_root: &Path, sha: &str) -> bool {
    let Ok(Some(tip)) = fetch_origin_main(repo_root).await else {
        return false;
    };
    git_answer(&["merge-base", "--is-ancestor", sha, &tip], repo_root)
        .await
        .or_unknown(false)
}

async fn push_sha(repo_root: &Path, sha: &str) -> PushOutcome {
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
            let stderr = String::from_utf8_lossy(&o.stderr);
            let reason = push_refusal(&stdout, &stderr);
            if push_was_rejected(&stdout, &stderr) {
                PushOutcome::Rejected(reason)
            } else {
                PushOutcome::Failed(reason)
            }
        }
        Err(e) => PushOutcome::Failed(e),
    }
}

fn push_refusal(stdout: &str, stderr: &str) -> String {
    stdout
        .lines()
        .filter(|line| line.starts_with('!'))
        .filter_map(|line| line.rsplit('\t').next())
        .chain(Some(stderr.trim()).filter(|s| !s.is_empty()))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn push_was_rejected(stdout: &str, stderr: &str) -> bool {
    const MOVED: [&str; 3] = ["non-fast-forward", "fetch first", "stale info"];
    MOVED
        .iter()
        .any(|reason| stdout.contains(reason) || stderr.contains(reason))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LandingOrder {
    OriginFirst,
    LocalFirst,
    /// INVARIANT: The gateway did not say whether it is paired. A landing on a repository
    /// with an `origin` waits, as origin-first would.
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OriginTip {
    NoOrigin,
    LocalFirst,
    Empty,
    At(String),
}

impl OriginTip {
    pub(crate) fn needs_push(&self, tip: &str) -> bool {
        match self {
            OriginTip::NoOrigin | OriginTip::LocalFirst => false,
            OriginTip::Empty => true,
            OriginTip::At(sha) => sha != tip,
        }
    }

    pub(crate) fn origin_first(&self) -> bool {
        matches!(self, OriginTip::Empty | OriginTip::At(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LandingBase {
    pub(crate) local: String,
    pub(crate) base: String,
    pub(crate) origin: OriginTip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BaseRefusal {
    MainUnreadable(String),
    Origin(String),
}

/// INVARIANT: Fetch `origin` when the landing is origin-first, and pick the base it builds
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowOutcome {
    NoOrigin,
    Unreachable(String),
    UpToDate,
    Followed { from: String, to: String },
    Refused(String),
}

/// INVARIANT: Bring local `main` up to `origin`'s, as a fast-forward only. A cheap
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

/// INVARIANT: Move `main` from `from` to `to`, a known fast-forward. A checkout on `main`
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
