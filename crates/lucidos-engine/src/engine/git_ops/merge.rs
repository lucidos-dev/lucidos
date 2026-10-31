use super::*;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What a catch-up reports when `main` gained commits that conflict with the
/// branch. Wrappers add context around it, so match with
/// [`is_concurrent_main_conflict`], never by prefix.
pub(crate) const CONCURRENT_MAIN_CONFLICT: &str = "New conflicts from concurrent main changes";

/// Whether an apply failed because a change that landed meanwhile conflicts
/// with this one. Apply All retries such a change once (ADR 0314).
pub(crate) fn is_concurrent_main_conflict(error: &str) -> bool {
    error.contains(CONCURRENT_MAIN_CONFLICT)
}

/// Whether a conflict resolution merged the `main` it was asked to merge.
///
/// Asked of the sha `main` had when the resolution began, never of `main` now.
/// Another change may land while the agent works, and the catch-up that
/// follows merges it in.
///
/// `or_unknown(false)`, and a start sha nobody could read is a "no" too. This
/// answer authorizes a fast-forward of `main`, so only a positive may pass.
pub(crate) async fn resolution_merged_main(
    repo_root: &Path,
    main_at_start: Option<&str>,
    branch_name: &str,
) -> bool {
    let Some(start) = main_at_start else {
        return false;
    };
    git_answer(
        &["merge-base", "--is-ancestor", start, branch_name],
        repo_root,
    )
    .await
    .or_unknown(false)
}

/// Re-merge main into a branch to catch up with any concurrent changes.
/// No-op when main hasn't moved. Aborts and returns Err on conflicts.
///
/// Private to this module ON PURPOSE, which is what keeps ADR 0241 true rather
/// than merely remembered. Apply is the only engine path that may merge `main`
/// into a thread's branch, and both callers below are Apply's. The agent's own
/// merge at the start of `/harden` (ADR 0345) is not engine code. A
/// session-path caller would have to widen this visibility first, and that is
/// the moment to re-read the ADR. `git_ops/mod.rs` re-exports `merge::*`, so
/// private here also means absent from the crate-wide surface.
async fn catchup_with_main(
    worktree_path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let result = git_cmd(&["merge", "main", "--no-edit"], worktree_path).await;
    match result {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let _ = git_cmd(&["merge", "--abort"], worktree_path).await;
            let stderr = String::from_utf8_lossy(&o.stderr);
            Err(format!("{CONCURRENT_MAIN_CONFLICT}: {}", stderr.trim()).into())
        }
        Err(e) => Err(e.into()),
    }
}

/// One merge to `main` at a time. Taken before [`WORKSPACE_REPO_MUTEX`].
pub(crate) static MERGE_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The two locks a merge to `main` takes, in this order. Production passes
/// [`MAIN_LOCKS`]. A test passes a pair of its own, so no other test queues on it.
pub(crate) struct MainLocks {
    pub(super) merge: &'static tokio::sync::Mutex<()>,
    pub(super) repo: &'static tokio::sync::Mutex<()>,
}

pub(crate) static MAIN_LOCKS: MainLocks = MainLocks {
    merge: &MERGE_MUTEX,
    repo: &WORKSPACE_REPO_MUTEX,
};

impl MainLocks {
    pub(crate) async fn lock_merge(&self) -> tokio::sync::MutexGuard<'static, ()> {
        self.merge.lock().await
    }

    pub(crate) async fn lock_repo(&self) -> WorkspaceRepoLock {
        WorkspaceRepoLock {
            _guard: self.repo.lock().await,
        }
    }

    /// Both locks, in order, for anything that moves `main` outside a merge:
    /// a revert, or a commit on the repo root. A landing pass holds the merge
    /// lock from reading `main` to moving it. So this keeps every other mover
    /// out of that window (ADR 0463).
    pub(crate) async fn lock_main(&self) -> MainMoveLock {
        let merge = self.lock_merge().await;
        MainMoveLock {
            _repo: self.lock_repo().await,
            _merge: merge,
        }
    }

    /// A pair of locks nothing else queues on.
    #[cfg(test)]
    pub(crate) fn private_for_test() -> &'static MainLocks {
        Box::leak(Box::new(MainLocks {
            merge: Box::leak(Box::new(tokio::sync::Mutex::new(()))),
            repo: Box::leak(Box::new(tokio::sync::Mutex::new(()))),
        }))
    }
}

/// Proof that the holder owns both locks of [`MainLocks::lock_main`]. The repo
/// lock is declared first, so it is released first.
pub(crate) struct MainMoveLock {
    _repo: WorkspaceRepoLock,
    _merge: tokio::sync::MutexGuard<'static, ()>,
}

/// Read `main` and the branch, then fast-forward `main` to the branch. The
/// caller's repo lock keeps both reads and the ref move on one `main`.
pub(crate) async fn ff_main_to_branch(
    held: &WorkspaceRepoLock,
    repo_root: &Path,
    branch_name: &str,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    let main_sha = branch_head_sha(repo_root, "main").await.unwrap_or_default();
    let branch_sha = branch_head_sha(repo_root, branch_name)
        .await
        .unwrap_or_default();
    ff_main_to_locked(held, repo_root, &branch_sha, &main_sha).await
}

/// Fast-forward refs/heads/main to `branch_sha` using update-ref, then sync the
/// repo-root working tree to it. Works whether HEAD is detached or on a branch.
/// Returns (pre_sha, post_sha) on success.
///
/// The repo lock spans the ref move AND the tree sync, so no workspace repo
/// writer runs between them. A `main` moved by a git command outside the engine
/// fails the merge-base check or `update-ref`'s old-value guard, loudly.
pub(crate) async fn ff_main_to_locked(
    _held: &WorkspaceRepoLock,
    repo_root: &Path,
    branch_sha: &str,
    main_sha: &str,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    // An unresolved ref reaches here as an empty string: `branch_head_sha`
    // answers `None`, and the caller applies `unwrap_or_default`. Otherwise a
    // `merge-base` that also failed leaves both sides empty, the ff-ability
    // guard below compares equal, and an `update-ref` fires on refs nobody
    // could resolve.
    if branch_sha.is_empty() || main_sha.is_empty() {
        return Err("Cannot fast-forward: could not resolve the branch and main revisions".into());
    }

    // Verify ff-ability: main must be an ancestor of the branch
    let merge_base = git_cmd(&["merge-base", "main", branch_sha], repo_root)
        .await
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if merge_base != main_sha {
        return Err(format!(
            "Cannot fast-forward: main ({}) is not an ancestor of branch ({})",
            &main_sha[..main_sha.floor_char_boundary(8)],
            &branch_sha[..branch_sha.floor_char_boundary(8)],
        )
        .into());
    }

    // Atomically advance refs/heads/main (old-value guard prevents races)
    match git_cmd(
        &["update-ref", "refs/heads/main", branch_sha, main_sha],
        repo_root,
    )
    .await
    {
        Ok(o) if o.status.success() => {
            // update-ref only moves the ref pointer -- it doesn't touch the working
            // tree or index. `checkout -f main` both attaches HEAD to main (in case
            // it was detached, e.g. by a failed `pull --rebase`) and resets the
            // working tree + index to match the new main.
            //
            // This sync MUST land. `main` is already advanced, so a repo-root
            // tree left at the old commit reads as "every merged file deleted".
            // The next `commit_all_dirty` would commit exactly that deletion.
            // The repo lock keeps the engine's own writers out, but a git
            // process outside the engine can still hold the index lock. So the
            // sync waits out a lock-file holder instead of failing on the first
            // collision. `ensure_head_on_main` is NOT a safety net for it: it
            // only aborts a stale rebase and re-attaches a detached HEAD.
            let sync_err =
                match git_cmd_await_lock_file(&["checkout", "-f", "main"], repo_root).await {
                    Ok(o) if o.status.success() => None,
                    Ok(o) => Some(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                    Err(e) => Some(e),
                };
            // A sync that still fails after the lock-file waits FAILS the merge
            // rather than reporting a clean apply over a tree it knows is stale.
            // A failed apply is recoverable: main carries the commits, so a
            // re-apply takes the already-merged idempotent path. Silently
            // reverting the merged files is not. Logged as well as returned,
            // because the Tier-3 fast path discards the Err.
            if let Some(err) = sync_err {
                let short = &branch_sha[..branch_sha.floor_char_boundary(8)];
                log!(
                    "[Changes] ERROR: checkout -f main after update-ref failed: {}. main advanced to {} but the repo-root working tree did not, so this merge is reported as failed rather than left to be silently reverted by the next auto-commit of data/.",
                    err,
                    short
                );
                return Err(format!(
                    "main advanced to {} but the repo-root working tree could not be synced: {}",
                    short, err
                )
                .into());
            }
            Ok((main_sha.to_string(), branch_sha.to_string()))
        }
        Ok(o) => Err(format!(
            "update-ref failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )
        .into()),
        Err(e) => Err(e.into()),
    }
}

/// Catch up with main, then fast-forward main to the branch, all under one hold
/// of the merge and repo locks. On success, deletes the branch.
/// Returns (pre_merge_sha, post_merge_sha) on success for revert tracking.
pub(crate) async fn catchup_and_ff_to_main(
    repo_root: &Path,
    worktree_path: &Path,
    branch_name: &str,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    catchup_and_ff_to_main_under(&MAIN_LOCKS, repo_root, worktree_path, branch_name).await
}

pub(crate) async fn catchup_and_ff_to_main_under(
    locks: &MainLocks,
    repo_root: &Path,
    worktree_path: &Path,
    branch_name: &str,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    let _merge_guard = locks.merge.lock().await;
    let held = locks.lock_repo().await;
    catchup_with_main(worktree_path).await?;
    let shas = ff_main_to_branch(&held, repo_root, branch_name).await?;
    drop(held);
    let _ = git_cmd(&["branch", "-D", branch_name], repo_root).await;
    Ok(shas)
}

/// Catch up in the merge worktree, then fast-forward main to the temp branch,
/// all under one hold of the merge and repo locks.
/// Returns (pre_merge_sha, post_merge_sha) on success for revert tracking.
///
/// Throwaway state is reclaimed on BOTH exits, and only when it really is
/// throwaway. `temp_branch != feature_branch` means the merge ran in a Tier-3
/// temp worktree on a `merge-tmp/` branch this attempt created, so the tree and
/// both branches go. Equal names mean the session ran in the THREAD's own
/// worktree on the REAL change branch (Tier 2, and a pruned-temp re-attach).
/// Neither is this call's to delete: a session teardown never reclaims a
/// worktree (ADR 0035), and dropping the change branch costs the thread its
/// resumability the way `SessionEndAction::KeepEmptyBranch` describes. The
/// Tier-2 fast path in `apply_change` keeps both for exactly that reason.
pub(crate) async fn ff_merge_to_main(
    repo_root: &Path,
    wt_path: &str,
    temp_branch: &str,
    feature_branch: &str,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    let merged = {
        let _merge_guard = MAIN_LOCKS.merge.lock().await;
        let held = MAIN_LOCKS.lock_repo().await;
        catch_up_and_publish(&held, repo_root, wt_path, temp_branch, feature_branch).await
    };
    let err = match merged {
        Ok(shas) => {
            if temp_branch == feature_branch {
                log!(
                    "[Changes] ff-merge applied {}: keeping the thread worktree and branch (the merge ran on the real change branch)",
                    feature_branch
                );
            } else {
                let _ = git_cmd(&["worktree", "remove", "--force", wt_path], repo_root).await;
                let _ = git_cmd(&["branch", "-D", temp_branch], repo_root).await;
                // `-d`, not `-D`: git itself refuses to drop a branch main lacks.
                let _ = git_cmd(&["branch", "-d", feature_branch], repo_root).await;
            }
            return Ok(shas);
        }
        Err(e) => e,
    };

    // The merge failed. Clean up ONLY throwaway temp state: on a Tier-2 (or
    // pruned-temp re-attach) resolution `temp_branch == feature_branch`, i.e. the
    // session ran in the THREAD's own worktree on the REAL change branch, so
    // removing the worktree and force-deleting the branch here would destroy the
    // user's committed work while the change row stays pending and points at a
    // branch that no longer exists. Failure-path cleanup must only undo what this
    // attempt created (.claude/rules/rust.md), which is why the Abort arm has the
    // same guard in `conflict_abort_deletes_temp_state`.
    if temp_branch != feature_branch {
        let _ = git_cmd(&["worktree", "remove", "--force", wt_path], repo_root).await;
        let _ = git_cmd(&["branch", "-D", temp_branch], repo_root).await;
    } else {
        log!(
            "[Changes] ff-merge failed for {}: keeping the thread worktree and branch (the merge ran on the real change branch)",
            temp_branch
        );
    }
    Err(err)
}

/// [`ff_merge_to_main`]'s locked part: catch up, check the resolution kept the
/// change, then move `main`.
async fn catch_up_and_publish(
    held: &WorkspaceRepoLock,
    repo_root: &Path,
    wt_path: &str,
    temp_branch: &str,
    feature_branch: &str,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    catchup_with_main(Path::new(wt_path)).await?;
    if temp_branch != feature_branch
        && !temp_branch_contains_change(repo_root, temp_branch, feature_branch).await
    {
        return Err(format!(
            "The conflict resolution did not merge {feature_branch}, so nothing was applied"
        )
        .into());
    }
    ff_main_to_branch(held, repo_root, temp_branch).await
}

/// Whether a Tier-3 temp branch holds every commit of the change branch.
///
/// The temp branch starts at `main` and only the resolving agent merges the
/// change in, so a turn that skipped the merge leaves it out. `or_unknown(false)`:
/// a yes authorizes publishing and then deleting the change branch.
async fn temp_branch_contains_change(
    repo_root: &Path,
    temp_branch: &str,
    feature_branch: &str,
) -> bool {
    git_answer(
        &["merge-base", "--is-ancestor", feature_branch, temp_branch],
        repo_root,
    )
    .await
    .or_unknown(false)
}

/// Push `main` to `origin` in the background, for an Apply outside the landing
/// lane. A repository without `origin` pushes nothing. A failed push emits
/// `MainPushFailed`, so the user sees it (ADR 0589).
pub(crate) fn push_main_in_background(repo_root: &Path, bus: &EventBus) {
    let repo_root = repo_root.to_path_buf();
    let bus = bus.clone();
    tokio::spawn(async move {
        if let Some(reason) = push_main(&repo_root).await {
            log!("[Changes] Push main to origin failed: {}", reason);
            bus.emit_or_log(
                BusEvent::System(SystemEvent::MainPushFailed {
                    repo: repo_root.display().to_string(),
                    reason,
                }),
                "[Changes] MainPushFailed",
            )
            .await;
        }
    });
}

/// Push `main` to `origin`. `None` when it went, or when there is no `origin`.
/// Otherwise why it failed. An unanswered remote check counts as a failure.
pub(super) async fn push_main(repo_root: &Path) -> Option<String> {
    match origin_remote(repo_root).await {
        GitAnswer::No => return None,
        GitAnswer::Yes => {}
        GitAnswer::Unknown => {
            return Some("git could not say whether the repository has an origin remote".into())
        }
    }
    let Some(main) = branch_head_sha(repo_root, "main").await else {
        return Some("could not read main".into());
    };
    match push_to_origin_main(repo_root, &main).await {
        PushOutcome::Pushed => {
            log!("[Changes] Pushed main to origin");
            None
        }
        PushOutcome::Rejected(e) => Some(format!(
            "origin/main has moved, so main was not pushed: {e}"
        )),
        PushOutcome::Failed(e) => Some(e),
    }
}

/// Characters that can continue a git ref name. Used to tell a mention of
/// `feature` from one of `feature-2`, which merely contains it.
fn is_ref_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/')
}

/// Does `line` mention `needle` as a whole ref token, i.e. not flanked by
/// characters that would make it part of a longer branch name?
///
/// A plain `line.contains(branch)` is wrong here and the consequence is severe.
/// Coding-agent branches are named `lucidos-<agent>-<scope>-<slug>` with `-2` /
/// `-3` appended on a duplicate slug, so `…-fix-auth` is a literal prefix of
/// `…-fix-auth-2`. Under a substring match, merging the `-2` branch would make
/// [`find_branch_merge_in_main`] report the FIRST branch as already merged, and
/// its caller (`apply_change`) would resolve that Apply as an idempotent no-op:
/// the user clicks Apply, is told it already landed, and their commits never
/// reach main. (The same hazard was already latent for `claude-code/foo` versus
/// `claude-code/foobar`.)
fn mentions_ref(line: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(rel) = line[from..].find(needle) {
        let start = from + rel;
        let end = start + needle.len();
        let bounded_left = line[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_ref_char(c));
        let bounded_right = line[end..].chars().next().is_none_or(|c| !is_ref_char(c));
        if bounded_left && bounded_right {
            return true;
        }
        // Overlapping occurrences are possible, so step past this match's FIRST
        // CHARACTER rather than to `end`. By character, not by byte: git allows
        // non-ASCII branch names (an adopted renegade branch in an external repo
        // can carry one), and `start + 1` would land mid-codepoint and panic the
        // next `line[from..]`.
        from = start + line[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// Check if a git log --oneline merge entry represents merging `branch_name` INTO main,
/// not the reverse (merging main into the branch). Git merge commit messages follow these patterns:
/// - "abc1234 Merge branch 'feature-branch'" -- branch merged into current (main)
/// - "abc1234 Merge feature-branch: description" -- branch merged with custom message
/// - "abc1234 Merge branch 'main' into feature-branch" -- WRONG direction (main into branch)
///
/// Both halves match on whole ref tokens (see [`mentions_ref`]), so a sibling
/// branch that merely contains this one's name neither counts as a merge of it
/// nor suppresses one.
pub(crate) fn is_merge_of_branch_into_main(line: &str, branch_name: &str) -> bool {
    if !mentions_ref(line, branch_name) {
        return false;
    }
    // Reject "into <branch>" pattern -- that's merging something INTO the branch, not FROM it
    if mentions_ref(line, &format!("into {}", branch_name)) {
        return false;
    }
    true
}

/// What the leftover rebase directories are, once git has answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StaleRebase {
    /// git aborted the rebase, and the directories went with it.
    Aborted,
    /// git ran and refused, so what is left on disk is residue.
    Residue,
    /// git could not be asked, so nothing is known about the state.
    Unanswered,
}

/// Classify a `git rebase --abort` result. Pure, so the `Unanswered` arm is
/// unit-testable without starving a real git process.
///
/// `Err` is a spawn failure or the [`GIT_TIMEOUT`] ceiling, which a live rebase
/// on a saturated host really does hit. Deleting `.git/rebase-merge` there
/// destroys the user's only route back to the pre-rebase HEAD, so an unanswered
/// probe must never authorize it. A non-zero exit is a different thing: git ran
/// and said there is no rebase to abort, which makes the leftovers residue.
pub(crate) fn classify_rebase_abort(result: &Result<std::process::Output, String>) -> StaleRebase {
    match result {
        Ok(o) if o.status.success() => StaleRebase::Aborted,
        Ok(_) => StaleRebase::Residue,
        Err(_) => StaleRebase::Unanswered,
    }
}

/// Clean up stale git state and re-attach HEAD to main if needed.
///
/// A Claude Code session (or any git operation) can leave behind:
/// 1. A stale rebase -- `.git/rebase-merge/` or `.git/rebase-apply/` exists
///    but no rebase is actively running. This blocks `git status` output.
/// 2. A detached HEAD -- causes `git status` to report false dirty files.
///
/// This is a no-op when git state is clean and HEAD is on main.
///
/// An unanswered abort makes the whole function stand down, re-attach included.
/// The two halves are one decision. HEAD is legitimately detached during a live
/// rebase, so the re-attach below would force-check-out main over the tree the
/// `Unanswered` arm just declined to disturb. Standing down leaves the caller's
/// `git status` to report dirty, which refuses the apply loudly.
pub(crate) async fn ensure_head_on_main(repo_root: &Path) {
    let rebase_merge = repo_root.join(".git/rebase-merge");
    let rebase_apply = repo_root.join(".git/rebase-apply");
    if rebase_merge.exists() || rebase_apply.exists() {
        log!("[Changes] Stale rebase state detected -- aborting");
        let abort = git_cmd(&["rebase", "--abort"], repo_root).await;
        let detail = match &abort {
            Ok(o) => String::from_utf8_lossy(&o.stderr).trim().to_string(),
            Err(e) => e.clone(),
        };
        match classify_rebase_abort(&abort) {
            StaleRebase::Aborted => {}
            StaleRebase::Residue => {
                log!("[Changes] git rebase --abort refused: {}", detail);
                for dir in [&rebase_merge, &rebase_apply] {
                    if dir.exists() {
                        if let Err(e) = std::fs::remove_dir_all(dir) {
                            log!("[Changes] Failed to remove {}: {}", dir.display(), e);
                        }
                    }
                }
                log!("[Changes] Removed stale rebase directories directly");
            }
            StaleRebase::Unanswered => {
                log!(
                    "[Changes] cannot run git rebase --abort: {}. Leaving the rebase state \
                     and HEAD untouched",
                    detail
                );
                return;
            }
        }
    }

    // `or_unknown(true)`: only a positive "HEAD is detached" may reach the
    // `checkout -f` below, which discards whatever is uncommitted in the repo
    // root's working tree. A timed-out `symbolic-ref` used to resolve to
    // `false` here, so a saturated host could force-checkout main over the
    // user's checkout on the strength of a probe that never answered.
    // Skipping a genuinely needed re-attach costs a loud merge failure instead.
    let head_ok = git_answer(&["symbolic-ref", "HEAD"], repo_root)
        .await
        .or_unknown(true);
    if !head_ok {
        log!("[Changes] HEAD is detached -- re-attaching to main");
        // Same lock contention as the sync in ff_main_to_locked: a detached HEAD that
        // stays detached because a concurrent artifact commit held a lock makes
        // every later `git status` report false dirty files.
        match git_cmd_await_lock_file(&["checkout", "-f", "main"], repo_root).await {
            Ok(o) if o.status.success() => {}
            Ok(o) => log!(
                "[Changes] Failed to re-attach HEAD: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ),
            Err(e) => log!("[Changes] Failed to re-attach HEAD: {}", e),
        }
    }
}

/// Detect the default branch from `origin` for an external repo.
/// Returns `Some("origin/main")` (or whatever the default is) when the remote exists,
/// or `None` when there is no `origin` remote (so the worktree will branch from HEAD).
pub(crate) async fn detect_origin_default_branch(repo_root: &Path) -> Option<String> {
    if !origin_remote(repo_root).await.or_unknown(false) {
        log!("[Changes] No 'origin' remote found -- will branch from HEAD");
        return None;
    }

    // Fetch latest from origin so we branch from up-to-date default branch
    match git_cmd(&["fetch", "origin"], repo_root).await {
        Ok(o) if o.status.success() => {}
        Ok(o) => log!(
            "[Changes] git fetch origin warning: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log!("[Changes] Failed to run git fetch: {}", e),
    }

    let remote_ref = match read_origin_head_ref(repo_root).await {
        Some(branch) => format!("origin/{}", branch),
        None => {
            log!("[Changes] Could not detect default branch, falling back to origin/main");
            "origin/main".to_string()
        }
    };

    // Fast-forward the local default branch to match origin so diffs and merges
    // use the latest state.  "origin/main" -> local branch "main".
    let local_branch = remote_ref.strip_prefix("origin/").unwrap_or(&remote_ref);
    let current_branch = git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"], repo_root)
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    match current_branch.as_deref() {
        Some(b) if b == local_branch => {
            // We're on the default branch -- merge ff-only to update it in place
            match git_cmd(&["merge", "--ff-only", &remote_ref], repo_root).await {
                Ok(o) if o.status.success() => log!(
                    "[Changes] Fast-forwarded local {} to {}",
                    local_branch,
                    remote_ref
                ),
                Ok(_) => log!(
                    "[Changes] Could not fast-forward {} (diverged) -- worktree will branch from {}",
                    local_branch,
                    remote_ref
                ),
                Err(e) => log!("[Changes] Failed to fast-forward {}: {}", local_branch, e),
            }
        }
        Some(_) => {
            // Not on the default branch. A local fetch moves the ref only on a
            // fast-forward, so unpushed local commits are never rewound. It
            // also refuses a branch checked out in another worktree.
            let refspec = format!("refs/remotes/{remote_ref}:refs/heads/{local_branch}");
            match git_cmd(&["fetch", ".", &refspec], repo_root).await {
                Ok(o) if o.status.success() => log!(
                    "[Changes] Updated local {} to match {}",
                    local_branch,
                    remote_ref
                ),
                Ok(_) => log!(
                    "[Changes] Could not update local {} -- worktree will branch from {}",
                    local_branch,
                    remote_ref
                ),
                Err(e) => log!("[Changes] Failed to update local {}: {}", local_branch, e),
            }
        }
        None => {
            // git could not say which branch the repo root is on, so neither
            // arm above is known to fit. Leave the local ref alone; worktrees
            // branch from a slightly older base, which both arms already treat
            // as an acceptable outcome.
            log!(
                "[Changes] Could not read the current branch; leaving local {} alone (worktree will branch from {})",
                local_branch,
                remote_ref
            );
        }
    }

    Some(remote_ref)
}

/// Resolve the git ref a freshly-spawned coding-agent worktree must branch from.
///
/// The new worktree's branch must NEVER inherit whatever the shared repo
/// checkout happens to have at `HEAD`. A primary checkout parked on an unrelated
/// in-flight `lucidos-*` branch would otherwise leak that branch's commits
/// into the new thread as phantom "pending changes" — a thread that did nothing
/// surfaces a multi-file diff it never authored. Always resolve an explicit base:
///
/// - **External repo**: the `origin` default branch (`origin/main`, …) via
///   [`detect_origin_default_branch`], or `None` when there is no `origin`
///   remote — external repos legitimately branch from the user's checked-out
///   `HEAD` in that case (there is no canonical local default to prefer).
/// - **Lucidos-source repo**: the workspace's local default branch via
///   [`default_local_branch`] (`main` / `master` / …). Always `Some`, so a
///   Lucidos-source spawn can never silently fall back to `HEAD`.
pub(crate) async fn resolve_worktree_base(
    repo_root: &Path,
    is_external_repo: bool,
) -> Option<String> {
    if is_external_repo {
        detect_origin_default_branch(repo_root).await
    } else {
        Some(default_local_branch(repo_root).await)
    }
}

/// If `branch_name` was already merged into main as a `--no-ff` merge commit,
/// return `(pre_merge_sha, post_merge_sha)` where:
///   - `pre_merge_sha`  = the merge commit's first parent (main before the merge)
///   - `post_merge_sha` = the merge commit itself
///
/// Returns `None` if no such merge commit is found in the recent main history.
/// Used by `apply_change` to make Apply idempotent when the branch was merged
/// out-of-band (e.g. by an agentic loop running `git merge` directly).
pub(crate) async fn find_branch_merge_in_main(
    repo_root: &Path,
    branch_name: &str,
) -> Option<(String, String)> {
    let base = default_local_branch(repo_root).await;
    // `--format=%H %s` keeps full 40-char SHAs (unlike --oneline) so the result
    // is suitable for git plumbing (rev-parse, log -1, diff range, etc.).
    let log_output = git_cmd(
        &["log", "--merges", "--format=%H %s", "-500", &base],
        repo_root,
    )
    .await
    .ok()?;
    if !log_output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&log_output.stdout);
    let merge_sha = text
        .lines()
        .find(|line| is_merge_of_branch_into_main(line, branch_name))
        .and_then(|line| line.split_whitespace().next())?
        .to_string();

    let parents_output = git_cmd(&["log", "-1", "--pretty=%P", &merge_sha], repo_root)
        .await
        .ok()?;
    if !parents_output.status.success() {
        return None;
    }
    let parents_text = String::from_utf8_lossy(&parents_output.stdout);
    let parent1 = parents_text.split_whitespace().next()?.to_string();

    Some((parent1, merge_sha))
}

/// Detect the local default branch name (e.g. `main`, `master`, `develop`, `trunk`).
///
/// External repos may use any default branch name. `origin/HEAD` is the
/// authoritative source; the `main`/`master` heuristic is a fallback for
/// repos without a remote (test fixtures, fresh init).
///
/// Result is cached per `repo_root` for `DEFAULT_BRANCH_CACHE_TTL` (60s) to
/// keep the cleanup worker from forking 2 git subprocesses per worktree per
/// tick; default-branch changes are rare enough that a 60s lag is invisible.
pub(crate) async fn default_local_branch(repo_root: &Path) -> String {
    let key = tokio::fs::canonicalize(repo_root)
        .await
        .unwrap_or_else(|_| repo_root.to_path_buf());
    if let Some(cached) = lookup_default_branch_cache(&key) {
        return cached;
    }
    let resolved = resolve_default_local_branch(repo_root).await;
    insert_default_branch_cache(key, &resolved);
    resolved
}

async fn resolve_default_local_branch(repo_root: &Path) -> String {
    default_local_branch_from(&RefFacts::read(repo_root).await)
}

/// `origin/HEAD`'s branch when it exists locally, else `main`, else `master`.
/// Falls back to the `"main"` guess, which [`default_diff_base`] never trusts
/// without checking the ref exists.
fn default_local_branch_from(facts: &RefFacts) -> String {
    let origin_head = facts
        .origin_head
        .as_deref()
        .filter(|b| facts.local(b).is_some());
    origin_head
        .into_iter()
        .chain(["main", "master"])
        .find(|b| facts.local(b).is_some())
        .unwrap_or("main")
        .to_string()
}

/// The branch facts the default-branch questions need, read in one launch.
///
/// Each launch costs tens of milliseconds on a busy macOS host, and these
/// questions sit on the Diff button's critical path. Keep them to one launch.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct RefFacts {
    /// Commit sha of every `refs/heads/*` and `refs/remotes/origin/*` ref.
    shas: HashMap<String, String>,
    /// The branch `refs/remotes/origin/HEAD` points at, e.g. `develop`. `None`
    /// when that branch is gone from the remote, which then names no default.
    origin_head: Option<String>,
}

impl RefFacts {
    /// A launch that fails or times out reads as a repo with no refs. The
    /// default branch is then the `"main"` guess, and the diff base the
    /// primary worktree's tip. Neither authorizes a destructive step: a merge
    /// into a missing `main` fails, and a diff base only changes what is shown.
    pub(crate) async fn read(repo_root: &Path) -> Self {
        let format = "--format=%(refname)%00%(objectname)%00%(symref)";
        let args = [
            "for-each-ref",
            format,
            "refs/heads/",
            "refs/remotes/origin/",
        ];
        match git_cmd(&args, repo_root).await {
            Ok(o) if o.status.success() => Self::parse(&String::from_utf8_lossy(&o.stdout)),
            _ => Self::default(),
        }
    }

    pub(crate) fn parse(stdout: &str) -> Self {
        let mut facts = Self::default();
        for line in stdout.lines() {
            let mut fields = line.split('\0');
            let (Some(name), Some(sha)) = (fields.next(), fields.next()) else {
                continue;
            };
            let symref = fields.next().unwrap_or("");
            if name == "refs/remotes/origin/HEAD" {
                facts.origin_head = symref
                    .strip_prefix("refs/remotes/origin/")
                    .map(str::to_string);
            } else {
                facts.shas.insert(name.to_string(), sha.to_string());
            }
        }
        facts
    }

    fn local(&self, branch: &str) -> Option<&str> {
        self.shas
            .get(&format!("refs/heads/{branch}"))
            .map(String::as_str)
    }

    fn remote(&self, branch: &str) -> Option<&str> {
        self.shas
            .get(&format!("refs/remotes/origin/{branch}"))
            .map(String::as_str)
    }
}

/// What [`default_diff_base`] can settle from [`RefFacts`] alone, and the two
/// cases that need one more launch.
#[derive(Debug, PartialEq)]
pub(crate) enum DiffBase {
    Settled(String),
    /// Both exist at different commits: `local` unless it has diverged from
    /// `remote_tracking`, which takes a `merge-base --is-ancestor`.
    LocalUnlessDiverged {
        local: String,
        remote_tracking: String,
    },
    PrimaryWorktreeHead,
}

pub(crate) fn diff_base_from(facts: &RefFacts) -> DiffBase {
    let local = default_local_branch_from(facts);
    let remote_tracking = format!("origin/{local}");
    if let Some(local_sha) = facts.local(&local) {
        // Equal shas are trivially "not diverged", so they skip the launch.
        return match facts.remote(&local) {
            Some(remote_sha) if remote_sha != local_sha => DiffBase::LocalUnlessDiverged {
                local,
                remote_tracking,
            },
            _ => DiffBase::Settled(local),
        };
    }
    if facts.remote(&local).is_some() {
        return DiffBase::Settled(remote_tracking);
    }
    match facts.origin_head.as_deref() {
        Some(name) if facts.remote(name).is_some() => DiffBase::Settled(format!("origin/{name}")),
        _ => DiffBase::PrimaryWorktreeHead,
    }
}

/// Resolve the base ref the Diff button should diff a coding-agent thread's
/// branch against — the point its eventual PR/merge diverges from.
///
/// Defaults to the local default branch (e.g. `main`). That is the right base
/// when the engine has applied work locally that hasn't been pushed yet: the
/// local branch is then *linearly ahead* of `origin/<default>`, and diffing
/// against it keeps already-merged commits out of the thread's diff (see
/// `diff_base_is_local_main_when_origin_is_behind`).
///
/// But when the local default branch has *diverged* from the remote-tracking
/// ref — its history was rewritten so `origin/<default>` is no longer an
/// ancestor of it (a repo migration, a force-pull, or a rebase landed on the
/// user's local `main`) — the merge-base with the thread branch collapses to an
/// ancient commit and the diff balloons with unrelated churn. Then fall back to
/// `origin/<default>`, which still holds the branch's true fork point, so the
/// diff matches what the host (GitHub, …) computes for the PR.
///
/// Repos with no `origin` remote (test fixtures, fresh init) keep the local
/// branch name.
///
/// When the local default branch does NOT exist at all, the base must still
/// resolve to a real ref — otherwise the Diff button's `<base>...<branch>` range
/// errors with `fatal: unknown revision`. This is the external-repo case: a repo
/// whose canonical branch was never checked out locally (the coding-agent
/// worktree branched straight off `origin/<default>`), especially one whose
/// default is neither `main` nor `master`, where [`default_local_branch`] falls
/// through to its hardcoded `"main"` guess. We fall back to `origin/<default>`
/// (the branch's true fork point) and ultimately to `HEAD`, never handing back a
/// phantom branch name.
pub(crate) async fn default_diff_base(repo_root: &Path) -> String {
    match diff_base_from(&RefFacts::read(repo_root).await) {
        DiffBase::Settled(base) => base,
        DiffBase::LocalUnlessDiverged {
            local,
            remote_tracking,
        } => {
            if is_ancestor(repo_root, &remote_tracking, &local).await {
                local
            } else {
                remote_tracking
            }
        }
        // Last resort (no origin, non-main/master default, e.g. a local
        // `trunk`): the PRIMARY worktree's tip commit. Deliberately NOT the
        // string `"HEAD"`. Inside a linked coding-agent worktree, `HEAD` is the
        // thread's OWN branch, so `HEAD...HEAD` would render an empty diff.
        DiffBase::PrimaryWorktreeHead => primary_worktree_head(repo_root)
            .await
            .unwrap_or_else(|| "HEAD".to_string()),
    }
}

/// The primary (main) worktree's tip commit SHA. `git worktree list
/// --porcelain` always lists the primary worktree first, so its `HEAD <sha>`
/// line is the repo's checked-out base commit even when this is called from a
/// linked worktree whose own `HEAD` is a feature branch. `None` when the command
/// fails or emits no `HEAD` line.
async fn primary_worktree_head(repo_root: &Path) -> Option<String> {
    let o = git_cmd(&["worktree", "list", "--porcelain"], repo_root)
        .await
        .ok()?;
    if !o.status.success() {
        return None;
    }
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("HEAD ").map(str::to_owned))
}

/// `true` if `git_ref` resolves to a commit in `repo_root`.
///
/// `or_unknown(false)`: both directions fail safely at its call site.
/// `worktree_add` either tries `-b` on an existing branch or checks out a
/// missing one, and git refuses loudly either way. Do NOT reuse this to gate a
/// destructive step: an unknown would read as "the ref is gone". A gate like
/// that wants the tri-state [`git_answer`] directly, so it can refuse.
pub(crate) async fn git_ref_exists(repo_root: &Path, git_ref: &str) -> bool {
    git_answer(&["rev-parse", "--verify", "--quiet", git_ref], repo_root)
        .await
        .or_unknown(false)
}

/// `true` if `ancestor` is an ancestor of `descendant` (i.e. `descendant`'s
/// history contains `ancestor`). Mirrors `git merge-base --is-ancestor`, whose
/// exit status is 0 for yes and 1 for no. A commit is its own ancestor, so
/// equal refs return `true`.
///
/// `or_unknown(false)`: its only caller is `default_diff_base`, where a
/// `false` means "assume the local default diverged" and diff against
/// `origin/<default>` instead. That is a read-only choice of base.
async fn is_ancestor(repo_root: &Path, ancestor: &str, descendant: &str) -> bool {
    git_answer(
        &["merge-base", "--is-ancestor", ancestor, descendant],
        repo_root,
    )
    .await
    .or_unknown(false)
}

const DEFAULT_BRANCH_CACHE_TTL: Duration = Duration::from_secs(60);

fn default_branch_cache() -> &'static Mutex<HashMap<PathBuf, (Instant, String)>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (Instant, String)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lookup_default_branch_cache(key: &Path) -> Option<String> {
    let cache = default_branch_cache().lock().ok()?;
    let (stamped, branch) = cache.get(key)?;
    (stamped.elapsed() < DEFAULT_BRANCH_CACHE_TTL).then(|| branch.clone())
}

fn insert_default_branch_cache(key: PathBuf, branch: &str) {
    if let Ok(mut cache) = default_branch_cache().lock() {
        cache.insert(key, (Instant::now(), branch.to_string()));
    }
}

/// Read `origin/HEAD` and return the local branch name (e.g. `"develop"`).
/// Returns `None` if the remote ref is unset or doesn't have the expected
/// `refs/remotes/origin/` prefix. Does NOT verify the local ref exists —
/// callers that need that should check it themselves.
async fn read_origin_head_ref(repo_root: &Path) -> Option<String> {
    let o = git_cmd(&["symbolic-ref", "refs/remotes/origin/HEAD"], repo_root)
        .await
        .ok()?;
    if !o.status.success() {
        return None;
    }
    let full_ref = String::from_utf8_lossy(&o.stdout).trim().to_string();
    full_ref
        .strip_prefix("refs/remotes/origin/")
        .map(str::to_string)
}
