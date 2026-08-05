use super::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Re-merge main into a branch to catch up with any concurrent changes.
/// No-op when main hasn't moved. Aborts and returns Err on conflicts.
pub(crate) async fn catchup_with_main(
    worktree_path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let result = git_cmd(&["merge", "main", "--no-edit"], worktree_path).await;
    match result {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let _ = git_cmd(&["merge", "--abort"], worktree_path).await;
            let stderr = String::from_utf8_lossy(&o.stderr);
            Err(format!(
                "New conflicts from concurrent main changes: {}",
                stderr.trim()
            )
            .into())
        }
        Err(e) => Err(e.into()),
    }
}

/// Serialized merge queue: only one merge-to-main operation at a time.
/// Prevents race conditions when multiple coding-agent sessions try to apply changes
/// simultaneously. Each merge acquires this lock, catches up with main
/// (which may have moved from a previous merge), and fast-forwards.
pub(crate) static MERGE_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Fast-forward refs/heads/main to `branch_sha` using update-ref.
/// Works regardless of HEAD state (detached or on a branch).
/// Returns (pre_sha, post_sha) on success.
pub(crate) async fn ff_main_to(
    repo_root: &Path,
    branch_sha: &str,
    main_sha: &str,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    // An unresolved ref reaches here as an empty string (the callers' own
    // `rev-parse` calls fall back to `unwrap_or_default`). Reject that up front:
    // otherwise a `merge-base` that also failed leaves both sides empty, the
    // ff-ability guard below compares equal, and an `update-ref` fires on refs
    // nobody could resolve. Deliberately ahead of the lock below: it reads no
    // repo state, so an unresolvable ref should fail fast rather than queue
    // behind an unrelated worktree snapshot.
    if branch_sha.is_empty() || main_sha.is_empty() {
        return Err("Cannot fast-forward: could not resolve the branch and main revisions".into());
    }

    // Held across the ref move AND the working-tree sync below, so no
    // `commit_all_dirty` can snapshot the repo root while main is published but
    // the tree has not caught up. See REPO_WORKTREE_MUTEX for the failure it
    // prevents; MERGE_MUTEX (already held by every caller) is always the outer
    // lock, so the pair cannot deadlock.
    let _worktree_guard = REPO_WORKTREE_MUTEX.lock().await;

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
            // This sync MUST land, so it waits out a concurrent `.git/index.lock`
            // holder instead of giving up on the first collision. `main` is already
            // advanced at this point, so a repo-root working tree left at the old
            // commit reads as "every file the merge added has been deleted" -- and
            // the next `commit_all_dirty` (script output, artifact write, any
            // auto-commit that stages all of data/) commits exactly that deletion on
            // top of main, silently reverting the change that was just applied. That
            // is how a concurrent app-thread apply lost a merged file on 2026-08-03.
            // The collision itself is routine: libgit2 (via ArtifactManager) and
            // these shell calls write the same index lock without waiting for each
            // other. `ensure_head_on_main` is NOT a safety net for it -- that only
            // aborts a stale rebase and re-attaches a detached HEAD, and HEAD is
            // already on main here.
            let sync_err =
                match git_cmd_await_index_lock(&["checkout", "-f", "main"], repo_root).await {
                    Ok(o) if o.status.success() => None,
                    Ok(o) => Some(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                    Err(e) => Some(e),
                };
            // A sync that still fails after the retries FAILS the merge rather than
            // reporting a clean apply over a tree it knows is stale. Both retry
            // loops above re-enter here on the Err, and by then the ff itself is a
            // no-op (main and the branch are already equal), so an ordinary
            // collision just costs another attempt. If every attempt loses, the
            // caller surfaces a failed apply, which is recoverable (main carries the
            // commits, so a re-apply takes the already-merged idempotent path)
            // whereas silently reverting the merged files is not. Logged as well as
            // returned, because the Tier-3 fast path discards the Err.
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

/// Catch up with main, then fast-forward main to the branch.
/// Retries up to 3 times to handle the race where main moves between
/// catchup and ff (e.g. from a concurrent background push).
/// On success, deletes the branch.
/// Returns (pre_merge_sha, post_merge_sha) on success for revert tracking.
///
/// Serialized via MERGE_MUTEX -- only one merge-to-main at a time.
pub(crate) async fn catchup_and_ff_to_main(
    repo_root: &Path,
    worktree_path: &Path,
    branch_name: &str,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    let _merge_guard = MERGE_MUTEX.lock().await;

    let mut last_err: Option<Box<dyn std::error::Error + Send + Sync>> = None;
    for attempt in 1..=3 {
        catchup_with_main(worktree_path).await?;

        let main_sha = git_cmd(&["rev-parse", "main"], repo_root)
            .await
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let branch_sha = git_cmd(&["rev-parse", branch_name], repo_root)
            .await
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();

        match ff_main_to(repo_root, &branch_sha, &main_sha).await {
            Ok(shas) => {
                let _ = git_cmd(&["branch", "-D", branch_name], repo_root).await;
                push_main_in_background(repo_root);
                return Ok(shas);
            }
            Err(e) => {
                if attempt < 3 {
                    log!(
                        "[Changes] ff-merge attempt {}/3 failed ({}), retrying after catchup",
                        attempt,
                        e
                    );
                }
                last_err = Some(e);
            }
        }
    }

    Err(format!(
        "Fast-forward merge to main failed after 3 retries: {}",
        last_err
            .as_ref()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown error".to_string())
    )
    .into())
}

/// Catch up in the merge worktree, then fast-forward main to the temp branch.
/// Retries up to 3 times (re-catching up each time) before removing the worktree.
/// On success, removes the worktree and deletes both branches.
/// Returns (pre_merge_sha, post_merge_sha) on success for revert tracking.
///
/// Serialized via MERGE_MUTEX -- only one merge-to-main at a time.
pub(crate) async fn ff_merge_to_main(
    repo_root: &Path,
    wt_path: &str,
    temp_branch: &str,
    feature_branch: &str,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    let _merge_guard = MERGE_MUTEX.lock().await;

    let mut last_err: Option<Box<dyn std::error::Error + Send + Sync>> = None;
    for attempt in 1..=3 {
        catchup_with_main(Path::new(wt_path)).await?;

        let main_sha = git_cmd(&["rev-parse", "main"], repo_root)
            .await
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let branch_sha = git_cmd(&["rev-parse", temp_branch], repo_root)
            .await
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();

        // Worktree stays alive until merge succeeds so retries can re-catchup
        match ff_main_to(repo_root, &branch_sha, &main_sha).await {
            Ok(shas) => {
                let _ = git_cmd(&["worktree", "remove", "--force", wt_path], repo_root).await;
                let _ = git_cmd(&["branch", "-D", temp_branch], repo_root).await;
                let _ = git_cmd(&["branch", "-D", feature_branch], repo_root).await;
                push_main_in_background(repo_root);
                return Ok(shas);
            }
            Err(e) => {
                if attempt < 3 {
                    log!(
                        "[Changes] ff-merge attempt {}/3 failed ({}), retrying after catchup",
                        attempt,
                        e
                    );
                }
                last_err = Some(e);
            }
        }
    }

    // All retries exhausted. Clean up ONLY throwaway temp state: on a Tier-2 (or
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
            "[Changes] ff-merge exhausted for {}: keeping the thread worktree and branch (the merge ran on the real change branch)",
            temp_branch
        );
    }
    Err(format!(
        "Fast-forward merge to main failed after 3 retries: {}",
        last_err
            .as_ref()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown error".to_string())
    )
    .into())
}

/// Check if an `origin` remote exists in the repository.
///
/// `or_unknown(false)`: the only consequence is skipping a background push,
/// which the next apply retries. Nothing is deleted on either answer.
async fn has_origin_remote(repo_root: &Path) -> bool {
    git_answer(&["remote", "get-url", "origin"], repo_root)
        .await
        .or_unknown(false)
}

/// Push main to origin in the background if the remote exists.
/// If the push is rejected (origin diverged), logs and moves on -- next apply retries.
pub(crate) fn push_main_in_background(repo_root: &Path) {
    let repo_root = repo_root.to_path_buf();
    tokio::spawn(async move {
        if !has_origin_remote(&repo_root).await {
            return;
        }
        // Push directly -- don't use `pull --rebase` because it modifies the working
        // tree and can detach HEAD on conflict, leaving dirty files that block
        // subsequent change applies. If the push is rejected (origin diverged),
        // the next apply's push will retry.
        match git_cmd(&["push", "origin", "main"], &repo_root).await {
            Ok(o) if o.status.success() => {
                log!("[Changes] Pushed main to origin");
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                // Non-fast-forward is expected if origin diverged -- log and move on
                log!(
                    "[Changes] Push main to origin failed (will retry on next apply): {}",
                    stderr.trim()
                );
            }
            Err(e) => {
                log!("[Changes] Push main to origin failed: {}", e);
            }
        }
    });
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

/// Clean up stale git state and re-attach HEAD to main if needed.
///
/// A Claude Code session (or any git operation) can leave behind:
/// 1. A stale rebase -- `.git/rebase-merge/` or `.git/rebase-apply/` exists
///    but no rebase is actively running. This blocks `git status` output.
/// 2. A detached HEAD -- causes `git status` to report false dirty files.
///
/// This is a no-op when git state is clean and HEAD is on main.
pub(crate) async fn ensure_head_on_main(repo_root: &Path) {
    let rebase_merge = repo_root.join(".git/rebase-merge");
    let rebase_apply = repo_root.join(".git/rebase-apply");
    if rebase_merge.exists() || rebase_apply.exists() {
        log!("[Changes] Stale rebase state detected -- aborting");
        let aborted = match git_cmd(&["rebase", "--abort"], repo_root).await {
            Ok(o) if o.status.success() => true,
            Ok(o) => {
                log!(
                    "[Changes] git rebase --abort failed: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                );
                false
            }
            Err(e) => {
                log!("[Changes] git rebase --abort failed: {}", e);
                false
            }
        };
        if !aborted {
            for dir in [&rebase_merge, &rebase_apply] {
                if dir.exists() {
                    if let Err(e) = std::fs::remove_dir_all(dir) {
                        log!("[Changes] Failed to remove {}: {}", dir.display(), e);
                    }
                }
            }
            log!("[Changes] Removed stale rebase directories directly");
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
        // Same index-lock contention as the sync in ff_main_to: a detached HEAD
        // that stays detached because a concurrent artifact commit held the lock
        // makes every later `git status` report false dirty files.
        match git_cmd_await_index_lock(&["checkout", "-f", "main"], repo_root).await {
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
    if !has_origin_remote(repo_root).await {
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
            // Not on the default branch -- update the local ref directly (local-only, no network)
            let local_ref = format!("refs/heads/{}", local_branch);
            match git_cmd(&["update-ref", &local_ref, &remote_ref], repo_root).await {
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
            // git could not say which branch the repo root is on. The two arms
            // above are not interchangeable: the update-ref one moves
            // refs/heads/<default> with no old-value guard, so running it while
            // we are in fact standing on that branch rewrites the ref under a
            // live checkout and the whole tree reads as uncommitted. Leave the
            // local ref alone; worktrees branch from a slightly older base,
            // which both arms already treat as an acceptable outcome.
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
    if let Some(branch) = origin_head_branch(repo_root).await {
        return branch;
    }
    for name in &["main", "master"] {
        if let Ok(o) = git_cmd(&["rev-parse", "--verify", name], repo_root).await {
            if o.status.success() {
                return name.to_string();
            }
        }
    }
    "main".to_string()
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
    let local = default_local_branch(repo_root).await;
    let remote_tracking = format!("origin/{local}");

    if git_ref_exists(repo_root, &local).await {
        // Local default exists — prefer it, unless it has diverged from
        // origin/<default> (migration/force-push/rebase), in which case
        // origin/<default> holds the branch's true fork point.
        if git_ref_exists(repo_root, &remote_tracking).await
            && !is_ancestor(repo_root, &remote_tracking, &local).await
        {
            return remote_tracking;
        }
        return local;
    }

    // No local default branch. Diff against origin/<default> when it exists —
    // it's the ref the external-repo worktree was cut from.
    if git_ref_exists(repo_root, &remote_tracking).await {
        return remote_tracking;
    }

    // `local` is the phantom `"main"` guess (or a genuinely absent branch) and
    // origin/<local> doesn't exist either. Try origin/HEAD's real target (the
    // default may simply not be main/master).
    if let Some(name) = read_origin_head_ref(repo_root).await {
        let tracking = format!("origin/{name}");
        if git_ref_exists(repo_root, &tracking).await {
            return tracking;
        }
    }

    // Last resort (no origin, non-main/master default — e.g. a local `trunk`):
    // the PRIMARY worktree's tip commit. Deliberately NOT the string `"HEAD"`:
    // this helper also runs inside a linked coding-agent worktree (via
    // `diff_via_worktree`), where `HEAD` is the thread's OWN branch, so a
    // `HEAD...HEAD` range would render an empty diff instead of the branch's
    // changes. `git worktree list --porcelain` lists the primary worktree first,
    // so its tip is the repo's base commit regardless of which worktree we're in.
    if let Some(sha) = primary_worktree_head(repo_root).await {
        return sha;
    }
    "HEAD".to_string()
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
/// `or_unknown(false)`: both directions fail safely at every current call site.
/// `worktree_add` either tries `-b` on an existing branch or checks out a
/// missing one, and git refuses loudly either way; `default_diff_base` merely
/// picks a different base to diff against. Do NOT reuse this to gate a
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

/// Resolve `origin/HEAD` to a local branch name (e.g. `develop`), verifying
/// the local ref exists. Returns `None` if `origin/HEAD` is unset or the
/// local ref is missing.
async fn origin_head_branch(repo_root: &Path) -> Option<String> {
    let branch = read_origin_head_ref(repo_root).await?;
    let verify = git_cmd(&["rev-parse", "--verify", &branch], repo_root)
        .await
        .ok()?;
    verify.status.success().then_some(branch)
}
