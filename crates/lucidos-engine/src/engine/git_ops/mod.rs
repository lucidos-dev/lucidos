use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

mod checkpoint;
mod commits;
mod harden_marker;
mod merge;
mod plan_marker;
mod restart_detection;
mod worktree;

pub(crate) use checkpoint::*;
pub(crate) use commits::*;
pub(crate) use harden_marker::*;
pub(crate) use merge::*;
pub(crate) use plan_marker::*;
pub(crate) use restart_detection::*;
pub(crate) use worktree::*;

/// How long a single git invocation may run before the engine gives up on it.
/// Generous, because it is a ceiling on a saturated host, not a latency target:
/// while the e2e suite runs, ordinary `rev-parse` calls have been observed
/// taking tens of seconds.
pub(crate) const GIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Run a git command with the [`GIT_TIMEOUT`] ceiling. Always prepends
/// `-c core.quotepath=false` so non-ASCII paths come back as raw UTF-8 instead
/// of git's default `"...\NNN..."` form — every caller treats output as a path.
pub(crate) async fn git_cmd(args: &[&str], dir: &Path) -> Result<std::process::Output, String> {
    git_cmd_env(args, dir, &[]).await
}

/// Like [`git_cmd`] but with extra environment variables. Used by the command
/// checkpoint helpers, which set `GIT_INDEX_FILE` to a throwaway index so a
/// snapshot/restore never disturbs the repo's real index or working tree.
pub(crate) async fn git_cmd_env(
    args: &[&str],
    dir: &Path,
    envs: &[(&str, &OsStr)],
) -> Result<std::process::Output, String> {
    git_cmd_env_timeout(args, dir, envs, GIT_TIMEOUT).await
}

/// [`git_cmd_env`] with an explicit ceiling. The timeout is a parameter only so
/// tests can force the expiry branch deterministically (a nanosecond ceiling
/// cannot be met by any real process spawn); production callers go through
/// [`git_cmd`] / [`git_cmd_env`] and get [`GIT_TIMEOUT`].
pub(crate) async fn git_cmd_env_timeout(
    args: &[&str],
    dir: &Path,
    envs: &[(&str, &OsStr)],
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let mut full_args: Vec<&str> = Vec::with_capacity(args.len() + 2);
    full_args.push("-c");
    full_args.push("core.quotepath=false");
    full_args.extend_from_slice(args);
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(&full_args).current_dir(dir);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(format!("git {} failed: {}", args.join(" "), e)),
        Err(_) => Err(format!(
            "git {} timed out after {}s",
            args.join(" "),
            timeout.as_secs()
        )),
    }
}

/// Answer to a yes/no question the engine asks git.
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
pub(crate) enum GitAnswer {
    /// git ran and said yes (exit 0, or the caller's predicate held).
    Yes,
    /// git ran and said no (non-zero exit, or the caller's predicate failed).
    No,
    /// git could not be asked: spawn failure or timeout. NOT a `No`.
    Unknown,
}

impl GitAnswer {
    /// Collapse to a bool, naming the value `Unknown` takes.
    pub(crate) fn or_unknown(self, on_unknown: bool) -> bool {
        match self {
            GitAnswer::Yes => true,
            GitAnswer::No => false,
            GitAnswer::Unknown => on_unknown,
        }
    }

    /// True when git could not be asked. Use to log the distinction, or to
    /// refuse an action outright rather than pick a fallback.
    pub(crate) fn is_unknown(self) -> bool {
        matches!(self, GitAnswer::Unknown)
    }
}

/// Ask git a yes/no question, keeping "could not ask" separate from "no":
/// exit 0 is [`GitAnswer::Yes`], a non-zero exit is [`GitAnswer::No`], and a
/// spawn failure or timeout is [`GitAnswer::Unknown`].
pub(crate) async fn git_answer(args: &[&str], dir: &Path) -> GitAnswer {
    git_answer_with(args, dir, |_| true).await
}

/// [`git_answer`] where the yes/no split is decided by `predicate` over git's
/// output rather than by exit status alone. A non-zero exit is still `No`, so
/// this is for commands whose failure genuinely means "no" (`rev-parse` on a
/// path in no repository, say).
pub(crate) async fn git_answer_with(
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

/// [`git_answer_with`] for commands whose non-zero exit is a FAILURE rather
/// than an answer, so it maps to `Unknown` alongside spawn errors and timeouts.
///
/// `git status --porcelain` is the canonical case: it exits non-zero when the
/// path is not a work tree at all (or when a hook or filter blows up, as
/// git-crypt does on a locked repo), which says nothing about whether there is
/// uncommitted work. Reading that as "not dirty" would let a caller conclude
/// there is nothing to lose.
pub(crate) async fn git_answer_when_ok(
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

/// Pure mapping from a [`git_cmd`] result to a [`GitAnswer`]. `on_nonzero` is
/// what a non-zero exit means for this particular question. Split out from the
/// two helpers above so the `Unknown` arm is unit-testable without having to
/// starve a real git process.
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
                log!(
                    "[Git] `git {}` failed in {}: {}. Treating as unknown, never as a no",
                    args.join(" "),
                    dir.display(),
                    String::from_utf8_lossy(&o.stderr).trim()
                );
            }
            on_nonzero
        }
        Err(e) => {
            log!(
                "[Git] cannot answer `git {}` in {}: {}. Treating as unknown, never as a no",
                args.join(" "),
                dir.display(),
                e
            );
            GitAnswer::Unknown
        }
    }
}

/// Serializes the two workspace-repo operations that are only correct as a
/// unit: advancing `refs/heads/main` and syncing the repo-root working tree to
/// it ([`merge::ff_main_to`]), versus snapshotting that same working tree into
/// a commit (`Engine::commit_dirty_logged` -> `ArtifactManager::commit_all_dirty`).
///
/// These two do not merely race on git's index lock -- they race on a SEMANTIC
/// window that no retry can close, because neither side ever fails. `update-ref`
/// publishes the merge instantly; the `checkout -f main` that brings the working
/// tree along is a separate process. A `commit_all_dirty` landing in between
/// resets its index to the NEW head and then stages the OLD working tree over
/// it, so every file the merge just added is recorded as *deleted* and committed
/// straight onto main. That is the 2026-08-03 nightly's lost `style-b.css`,
/// which reproduced twice: once via a lost index-lock race (retried above) and
/// once via this window.
///
/// **Exactly two owners**, and each holds it for the REAL lifetime of its
/// operation: `ff_main_to` across the ref move and the tree sync, and
/// `commit_all_dirty` across the whole libgit2 snapshot. The snapshot's half is
/// the subtle one, and is why [`lock_repo_worktree_owned`] exists: that snapshot
/// runs in `spawn_blocking`, a blocking task cannot be cancelled, and a guard
/// scoped to the CALLER's future is dropped the instant that future times out,
/// while the closure is still inside `add_all` / `commit_index`. A guard that
/// outlives only the caller does not exclude anything.
///
/// So no caller of `commit_all_dirty` may hold this lock: the callee takes it
/// itself, and a caller holding it would queue behind its own snapshot.
///
/// Lock ordering: [`merge::MERGE_MUTEX`] is always acquired FIRST where both are
/// held (every `ff_main_to` caller wraps it), and nothing holding this lock ever
/// takes `MERGE_MUTEX`, so the two cannot deadlock. Below it sits only the
/// `ArtifactManager` repo handle, taken inside the snapshot's closure and never
/// in the other direction.
pub(crate) static REPO_WORKTREE_MUTEX: std::sync::LazyLock<std::sync::Arc<tokio::sync::Mutex<()>>> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Mutex::new(())));

/// [`REPO_WORKTREE_MUTEX`] as an OWNED guard, so it can be moved into a
/// `spawn_blocking` closure and released when the blocking work actually
/// finishes. A borrowed `lock()` guard has to be held by the async caller, whose
/// future can be dropped (by a timeout, or any other cancellation) while the
/// uncancellable closure runs on with the lock already free.
pub(crate) async fn lock_repo_worktree_owned() -> tokio::sync::OwnedMutexGuard<()> {
    REPO_WORKTREE_MUTEX.clone().lock_owned().await
}

/// Retry budget for a `.git/index.lock` collision. The lock is held only for
/// the duration of one index write (milliseconds), so a handful of short
/// backoffs drains any real contention; the cap keeps a genuinely wedged lock
/// from stalling a caller far past the 30s timeout on the command itself.
const INDEX_LOCK_RETRIES: u32 = 20;
const INDEX_LOCK_BACKOFF: Duration = Duration::from_millis(50);

/// `true` when a failed git invocation lost the race for `.git/index.lock`
/// rather than failing on its own merits. git reports it as
/// `fatal: Unable to create '<repo>/.git/index.lock': File exists.`
pub(crate) fn is_index_lock_collision(stderr: &str) -> bool {
    stderr.contains("index.lock")
        && (stderr.contains("File exists") || stderr.contains("Unable to create"))
}

/// Like [`git_cmd`], but waits out a concurrent holder of `.git/index.lock`
/// instead of failing on the collision.
///
/// A workspace repo has TWO independent writers: the shell `git` calls in this
/// module, and libgit2 through `ArtifactManager` (artifact writes, the
/// `commit_all_dirty` auto-commit after a script run). git's index lock is not
/// a waiting lock, so whoever loses errors out immediately. A command that MUST
/// land (the working-tree sync after `main` moves) therefore has to retry, or a
/// routine millisecond-long collision leaves the repo in whatever half-state
/// the loss interrupted.
///
/// Only a lock collision is retried. Every other failure, and every transport
/// error, is returned verbatim on the first attempt.
pub(crate) async fn git_cmd_await_index_lock(
    args: &[&str],
    dir: &Path,
) -> Result<std::process::Output, String> {
    let mut attempt = 0;
    loop {
        let output = git_cmd(args, dir).await?;
        if output.status.success() || attempt == INDEX_LOCK_RETRIES {
            return Ok(output);
        }
        if !is_index_lock_collision(&String::from_utf8_lossy(&output.stderr)) {
            return Ok(output);
        }
        attempt += 1;
        tokio::time::sleep(INDEX_LOCK_BACKOFF).await;
    }
}

#[cfg(test)]
#[path = "../git_ops_tests/common.rs"]
mod common;

#[cfg(test)]
#[path = "../git_ops_tests/answer.rs"]
mod answer_tests;

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
