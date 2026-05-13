use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Return the persistent directory for CC worktrees: `<workspace>/.lucidos/worktrees/`.
/// Creates the directory if it doesn't exist.
pub(crate) fn worktrees_dir(workspace_path: &Path) -> PathBuf {
    let dir = workspace_path.join(".lucidos/worktrees");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log!(
            "[Git] Failed to create worktrees dir {}: {}",
            dir.display(),
            e
        );
    }
    dir
}

/// Parse `git worktree list --porcelain` output into a branch->path map.
pub(crate) fn parse_worktree_list(output: &str) -> std::collections::HashMap<String, PathBuf> {
    let mut map = std::collections::HashMap::new();
    let mut current_path: Option<String> = None;
    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(path.to_string());
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            if let Some(ref p) = current_path {
                map.insert(branch.to_string(), PathBuf::from(p));
            }
        } else if line.is_empty() {
            current_path = None;
        }
    }
    map
}

/// Return the on-disk path of the worktree currently holding `branch_name`,
/// or `None` if no worktree has it checked out. Subprocess failures are logged
/// (callers treat the result as authoritative "no worktree exists" — silently
/// drifting on a `git worktree list` failure could lose CC work in the
/// discard / stale-recovery paths).
pub(crate) async fn find_worktree_for_branch(
    repo_root: &Path,
    branch_name: &str,
) -> Option<PathBuf> {
    let output = match git_cmd(&["worktree", "list", "--porcelain"], repo_root).await {
        Ok(o) => o,
        Err(e) => {
            log!(
                "[Git] git worktree list failed in {}: {}",
                repo_root.display(),
                e
            );
            return None;
        }
    };
    if !output.status.success() {
        log!(
            "[Git] git worktree list returned non-zero in {}: {}",
            repo_root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return None;
    }
    parse_worktree_list(&String::from_utf8_lossy(&output.stdout)).remove(branch_name)
}

/// Resolve the main working tree starting from the runtime repo root.
/// Walks `git worktree list --porcelain` (main worktree is always first).
pub(super) async fn main_worktree() -> PathBuf {
    resolve_main_worktree(&crate::paths::repo_root_or_compile_time_fallback()).await
}

/// Resolve the main working tree from a path that might be a git worktree.
/// `git worktree list --porcelain` always lists the main working tree first.
/// Falls back to the input path if git isn't available or the path isn't a repo.
///
/// If the compile-time path no longer exists (e.g., the engine was built in a
/// CC worktree that was later cleaned up), falls back to `std::env::current_dir()`
/// which is typically set to the repo root by the startup scripts.
pub(super) async fn resolve_main_worktree(path: &Path) -> PathBuf {
    // If the compile-time path no longer exists, try the process working directory
    let effective_path = if path.exists() {
        path.to_path_buf()
    } else {
        log!(
            "[ClaudeCode] Compile-time path {} no longer exists — trying cwd fallback",
            path.display()
        );
        match std::env::current_dir() {
            Ok(cwd) if cwd.join(".git").exists() => {
                log!(
                    "[ClaudeCode] Using cwd as fallback repo root: {}",
                    cwd.display()
                );
                cwd
            }
            _ => {
                log!(
                    "[ClaudeCode] No fallback available for non-existent compile-time path {}",
                    path.display()
                );
                return path.to_path_buf();
            }
        }
    };
    match git_cmd(&["worktree", "list", "--porcelain"], &effective_path).await {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // First "worktree <path>" line is always the main working tree
            if let Some(line) = stdout.lines().find(|l| l.starts_with("worktree ")) {
                let main_wt = PathBuf::from(line.strip_prefix("worktree ").unwrap());
                if main_wt != effective_path {
                    log!(
                        "[ClaudeCode] Resolved compile-time path to main working tree: {} -> {}",
                        effective_path.display(),
                        main_wt.display()
                    );
                }
                main_wt
            } else {
                effective_path
            }
        }
        _ => effective_path,
    }
}

/// True if `repo_path` doesn't canonicalize to `dev_root`. Falls back to raw
/// equality when canonicalization fails (test-only paths, repos missing on disk).
pub(crate) fn is_external_repo_path(repo_path: &Path, dev_root: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(repo_path) != canon(dev_root)
}

/// Check whether any of the given file paths would require an engine restart.
/// Rust source files (excluding tests/docs), SQL migrations, the SDK bundle
/// sources, and engine-bundled static assets all trigger a restart.
pub(crate) fn files_require_restart(files: &[String]) -> bool {
    files.iter().any(|f| {
        let is_rust_source = f.ends_with(".rs") || f == "Cargo.toml" || f == "Cargo.lock";
        let is_test_or_doc = f.contains("/tests/") || f.starts_with("tests/") || f.ends_with(".md");
        let is_migration =
            f.ends_with(".sql") && (f.contains("/migrations/") || f.starts_with("migrations/"));
        // packages/lucidos-sdk → /api/v1/sdk.js. The bundle is rebuilt by
        // `web-dev.sh -b` on engine restart; without restart the previously-
        // built dist/sdk.js keeps being served.
        let is_sdk_bundle_source = f.starts_with("packages/lucidos-sdk/") && !is_test_or_doc;
        // include_str!'d into the engine binary and served at /api/v1/sdk-iframe.*
        let is_engine_bundled_asset = f == "crates/lucidos-engine/src/api/sdk_iframe.css"
            || f == "crates/lucidos-engine/src/api/sdk_iframe_audio.js";
        (is_rust_source && !is_test_or_doc)
            || is_migration
            || is_sdk_bundle_source
            || is_engine_bundled_asset
    })
}

/// Check whether any of the given file paths are frontend files that would
/// require a client reload. TypeScript, CSS, HTML, and JavaScript files trigger this.
pub(crate) fn files_have_client_update(files: &[String]) -> bool {
    files.iter().any(|f| {
        f.ends_with(".ts")
            || f.ends_with(".tsx")
            || f.ends_with(".css")
            || f.ends_with(".html")
            || f.ends_with(".js")
            || f.ends_with(".jsx")
    })
}

/// `git rev-parse HEAD` in the worktree. Used by `auto_commit_preserving_marker`
/// to re-stamp the hardening record after an auto-commit moves HEAD.
pub(crate) async fn current_head_sha(worktree_path: &Path) -> Option<String> {
    let output = git_cmd(&["rev-parse", "HEAD"], worktree_path).await.ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// `git rev-parse <branch>` in the main repo — works after the worktree has
/// been removed, which is the case at apply time and during stale-session
/// recovery. Returns `None` if the branch ref doesn't exist.
pub(crate) async fn branch_head_sha(repo_root: &Path, branch_name: &str) -> Option<String> {
    let output = git_cmd(&["rev-parse", branch_name], repo_root).await.ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// `Fresh` means the stored SHA matches the branch's current HEAD; `Stale`
/// means CC has advanced HEAD since `/harden` ran; `Missing` means no DB row
/// exists for this (repo, branch). Apply-time hardening trusts both `Fresh`
/// and `Stale` (CC ran `/harden` at least once for this branch); only
/// `Missing` triggers Apply to run `/harden` itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HardenMarkerState {
    Fresh,
    Stale,
    Missing,
}

/// Canonical absolute repo_root used as the DB key. Resolves symlinks so the
/// hook (which uses `git rev-parse --git-common-dir`) and the engine produce
/// the same row.
fn canonical_repo_root(repo_root: &Path) -> String {
    repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf())
        .to_string_lossy()
        .to_string()
}

pub(crate) async fn harden_marker_state(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> HardenMarkerState {
    let stored: Option<String> = match sqlx::query_scalar(
        "SELECT head_sha FROM hardened_branches WHERE repo_root = $1 AND branch_name = $2",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .fetch_optional(pool)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            log!("[ClaudeCode] Failed to read hardened_branches: {}", e);
            return HardenMarkerState::Missing;
        }
    };
    let stored = match stored {
        Some(s) => s,
        None => return HardenMarkerState::Missing,
    };
    if let Some(current) = branch_head_sha(repo_root, branch_name).await {
        if stored == current {
            log!("[ClaudeCode] Harden marker is fresh (HEAD SHA matches)");
            return HardenMarkerState::Fresh;
        }
    }
    log!(
        "[ClaudeCode] Harden marker is STALE (stored={}…)",
        &stored[..stored.floor_char_boundary(12)]
    );
    HardenMarkerState::Stale
}

/// Convenience wrapper: true iff the marker is `Fresh`.
pub(crate) async fn is_harden_marker_fresh(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> bool {
    harden_marker_state(pool, repo_root, branch_name).await == HardenMarkerState::Fresh
}

/// True iff a marker exists at all (Fresh or Stale). Used by callers that
/// trust marker existence as a "CC ran /harden at least once" signal.
pub(crate) async fn is_harden_marker_present(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> bool {
    harden_marker_state(pool, repo_root, branch_name).await != HardenMarkerState::Missing
}

/// Record that `(repo_root, branch_name)` has been hardened at `head_sha`.
/// Idempotent — a second `/harden` upserts the new HEAD SHA. Called by the
/// HTTP endpoint that the `lucidos hardened mark` CLI POSTs to from the
/// `mark-harden.sh` hook.
pub(crate) async fn record_hardened(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
    head_sha: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO hardened_branches (repo_root, branch_name, head_sha, hardened_at) \
         VALUES ($1, $2, $3, NOW()) \
         ON CONFLICT (repo_root, branch_name) DO UPDATE SET head_sha = EXCLUDED.head_sha, hardened_at = NOW()"
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .bind(head_sha)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete the hardening record. Call after successful merge.
pub(crate) async fn consume_harden_marker(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) {
    if let Err(e) = sqlx::query(
        "DELETE FROM hardened_branches WHERE repo_root = $1 AND branch_name = $2",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .execute(pool)
    .await
    {
        log!(
            "[ClaudeCode] Failed to delete hardened_branches row for {}: {}",
            branch_name,
            e
        );
    }
}

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
/// Prevents race conditions when multiple CC sessions try to apply changes
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
            // If this fails, ensure_head_on_main (called before the next apply) is the safety net.
            match git_cmd(&["checkout", "-f", "main"], repo_root).await {
                Ok(o) if o.status.success() => {}
                Ok(o) => log!(
                    "[Changes] checkout -f main after update-ref failed: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
                Err(e) => log!("[Changes] checkout -f main after update-ref failed: {}", e),
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

    // All retries exhausted -- clean up
    let _ = git_cmd(&["worktree", "remove", "--force", wt_path], repo_root).await;
    let _ = git_cmd(&["branch", "-D", temp_branch], repo_root).await;
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
async fn has_origin_remote(repo_root: &Path) -> bool {
    git_cmd(&["remote", "get-url", "origin"], repo_root)
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
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

/// Check if a git log --oneline merge entry represents merging `branch_name` INTO main,
/// not the reverse (merging main into the branch). Git merge commit messages follow these patterns:
/// - "abc1234 Merge branch 'feature-branch'" -- branch merged into current (main)
/// - "abc1234 Merge feature-branch: description" -- branch merged with custom message
/// - "abc1234 Merge branch 'main' into feature-branch" -- WRONG direction (main into branch)
pub(crate) fn is_merge_of_branch_into_main(line: &str, branch_name: &str) -> bool {
    if !line.contains(branch_name) {
        return false;
    }
    // Reject "into <branch>" pattern -- that's merging something INTO the branch, not FROM it
    let into_pattern = format!("into {}", branch_name);
    if line.contains(&into_pattern) {
        return false;
    }
    true
}

/// Auto-commit harmless dirty files (docs/plans), then return whether the repo
/// is still dirty. This prevents apply/revert from blocking on safe changes.
/// Also re-attaches HEAD to main if detached (a precondition for accurate
/// dirty-file detection -- detached HEAD reports false diffs).
///
/// Returns `true` if the repo has uncommitted changes (after any auto-commit).
pub(super) async fn auto_commit_safe_files_if_dirty(repo_root: &Path) -> bool {
    ensure_head_on_main(repo_root).await;
    let output = match git_cmd(&["status", "--porcelain"], repo_root).await {
        Ok(o) => o,
        Err(_) => return false,
    };
    let status = String::from_utf8_lossy(&output.stdout);
    // Porcelain format: "XY filename" -- status is first 2 chars, then a space, then path
    // Skip untracked files (??) -- they don't block git merge
    let dirty_files: Vec<&str> = status
        .lines()
        .filter(|l| l.len() >= 4 && !l.starts_with("??"))
        .map(|l| l[3..].trim())
        .collect();
    if dirty_files.is_empty() {
        return false;
    }
    // Auto-commit harmless dirty files that shouldn't block merging Lucidos changes
    let auto_committable = dirty_files.iter().all(|f| f.starts_with("docs/plans/"));
    if auto_committable {
        let msg = "chore: commit docs changes";
        let mut add_args: Vec<&str> = vec!["add", "--"];
        add_args.extend(dirty_files.iter());
        let _ = git_cmd(&add_args, repo_root).await;
        let _ = git_cmd(&["commit", "-m", msg], repo_root).await;
        log!("[Git] Auto-committed dirty files: {:?}", dirty_files);
        return false;
    }
    true
}

/// Add a git worktree, bridging git-crypt's per-worktree key lookup to the
/// parent's unlocked key (AGWA/git-crypt#97). Sequence: `worktree add
/// --no-checkout`, symlink the key dir, `checkout HEAD`. Repos without
/// git-crypt skip the symlink step.
///
/// `extra_args` follow the worktree path: `[branch]` to reuse, `["-b", name]`
/// to create, optionally with a trailing base ref.
pub(crate) async fn worktree_add(
    repo_root: &Path,
    wt_path: &Path,
    extra_args: &[&str],
) -> Result<std::process::Output, String> {
    let wt_str = wt_path
        .to_str()
        .ok_or_else(|| format!("non-utf8 worktree path: {}", wt_path.display()))?;
    let mut args: Vec<&str> = vec!["worktree", "add", "--no-checkout", wt_str];
    args.extend_from_slice(extra_args);
    let add_out = git_cmd(&args, repo_root).await?;
    if !add_out.status.success() {
        return Ok(add_out);
    }
    if let Err(e) = link_git_crypt_dir(wt_path).await {
        log!(
            "[Git] git-crypt key symlink for {} failed (encrypted files may not decrypt): {}",
            wt_path.display(),
            e
        );
    }
    git_cmd(&["checkout", "HEAD", "--"], wt_path).await
}

async fn link_git_crypt_dir(wt_path: &Path) -> Result<(), String> {
    let out = git_cmd(
        &["rev-parse", "--absolute-git-dir", "--git-common-dir"],
        wt_path,
    )
    .await?;
    if !out.status.success() {
        return Err(format!(
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    let git_dir = lines.next().ok_or("rev-parse: missing --absolute-git-dir")?;
    let common_raw = lines.next().ok_or("rev-parse: missing --git-common-dir")?;

    let git_dir = PathBuf::from(git_dir.trim());
    let common_raw = common_raw.trim();
    let common_dir = if Path::new(common_raw).is_absolute() {
        PathBuf::from(common_raw)
    } else {
        wt_path.join(common_raw)
    };

    if git_dir == common_dir {
        return Ok(());
    }

    let source = common_dir.join("git-crypt");
    let target = git_dir.join("git-crypt");

    if !source.exists() {
        return Ok(());
    }
    if tokio::fs::symlink_metadata(&target).await.is_ok() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        tokio::fs::symlink(&source, &target).await.map_err(|e| {
            format!(
                "symlink {} -> {}: {}",
                target.display(),
                source.display(),
                e
            )
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (source, target);
        Err("git-crypt symlink only supported on unix".to_string())
    }
}

/// Run a git command with a 30-second timeout.
/// Returns the command output on success, or an error string on timeout/failure.
pub(crate) async fn git_cmd(args: &[&str], dir: &Path) -> Result<std::process::Output, String> {
    match tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output(),
    )
    .await
    {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(format!("git {} failed: {}", args.join(" "), e)),
        Err(_) => Err(format!("git {} timed out after 30s", args.join(" "))),
    }
}

/// Clean up stale git state and re-attach HEAD to main if needed.
///
/// A CC session (or any git operation) can leave behind:
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

    let head_ok = git_cmd(&["symbolic-ref", "HEAD"], repo_root)
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !head_ok {
        log!("[Changes] HEAD is detached -- re-attaching to main");
        match git_cmd(&["checkout", "-f", "main"], repo_root).await {
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
pub(super) async fn detect_origin_default_branch(repo_root: &Path) -> Option<String> {
    if !has_origin_remote(repo_root).await {
        log!("[ClaudeCode] No 'origin' remote found -- will branch from HEAD");
        return None;
    }

    // Fetch latest from origin so we branch from up-to-date default branch
    match git_cmd(&["fetch", "origin"], repo_root).await {
        Ok(o) if o.status.success() => {}
        Ok(o) => log!(
            "[ClaudeCode] git fetch origin warning: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log!("[ClaudeCode] Failed to run git fetch: {}", e),
    }

    let remote_ref = match read_origin_head_ref(repo_root).await {
        Some(branch) => format!("origin/{}", branch),
        None => {
            log!("[ClaudeCode] Could not detect default branch, falling back to origin/main");
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

    if current_branch.as_deref() == Some(local_branch) {
        // We're on the default branch -- merge ff-only to update it in place
        match git_cmd(&["merge", "--ff-only", &remote_ref], repo_root).await {
            Ok(o) if o.status.success() => log!(
                "[ClaudeCode] Fast-forwarded local {} to {}",
                local_branch,
                remote_ref
            ),
            Ok(_) => log!(
                "[ClaudeCode] Could not fast-forward {} (diverged) -- worktree will branch from {}",
                local_branch,
                remote_ref
            ),
            Err(e) => log!(
                "[ClaudeCode] Failed to fast-forward {}: {}",
                local_branch,
                e
            ),
        }
    } else {
        // Not on the default branch -- update the local ref directly (local-only, no network)
        let local_ref = format!("refs/heads/{}", local_branch);
        match git_cmd(&["update-ref", &local_ref, &remote_ref], repo_root).await {
            Ok(o) if o.status.success() => log!(
                "[ClaudeCode] Updated local {} to match {}",
                local_branch,
                remote_ref
            ),
            Ok(_) => log!(
                "[ClaudeCode] Could not update local {} -- worktree will branch from {}",
                local_branch,
                remote_ref
            ),
            Err(e) => log!(
                "[ClaudeCode] Failed to update local {}: {}",
                local_branch,
                e
            ),
        }
    }

    Some(remote_ref)
}

/// Subjects we never surface to the user — internal auto-commits.
fn is_internal_auto_commit(subject: &str) -> bool {
    matches!(
        subject,
        "Claude Code changes (auto-committed)"
            | "Claude Code changes (recovered after restart)"
            | "Claude Code changes (pre-merge auto-commit)"
            | "Claude Code changes (post-merge auto-commit)"
    )
}

/// Run `git log --format=%s <args>` and return user-meaningful commit subjects.
/// Internal auto-commits and blank lines are filtered out.
async fn commit_subjects(repo_root: &Path, log_args: &[&str]) -> Vec<String> {
    let mut args = vec!["log", "--format=%s"];
    args.extend_from_slice(log_args);
    match git_cmd(&args, repo_root).await {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| !l.is_empty() && !is_internal_auto_commit(l))
            .map(|l| l.to_string())
            .collect(),
        _ => Vec::new(),
    }
}

/// Read meaningful commit subjects in the range `pre_sha..post_sha`, oldest first.
pub(crate) async fn commits_in_range(
    repo_root: &Path,
    pre_sha: &str,
    post_sha: &str,
) -> Vec<String> {
    if pre_sha == post_sha {
        return Vec::new();
    }
    let range = format!("{}..{}", pre_sha, post_sha);
    commit_subjects(repo_root, &["--reverse", &range]).await
}

/// Build a description for a pending change from the commit subjects on a branch.
/// Reads `git log --format=%s <base>..branch` and summarizes the subjects.
/// If no meaningful commits found, uses `fallback` as the description.
/// If `suffix` is provided, it's appended in parentheses (e.g. "recovered").
pub(super) async fn describe_branch_changes(
    repo_root: &Path,
    range_arg: &str,
    fallback: &str,
    suffix: Option<&str>,
) -> String {
    let subjects = commit_subjects(repo_root, &[range_arg]).await;

    let base = if subjects.is_empty() {
        fallback.to_string()
    } else {
        subjects.join("\n")
    };

    match suffix {
        Some(s) => format!("{} ({})", base, s),
        None => base,
    }
}

/// Get the current branch of a worktree (before it is removed).
/// Returns `None` if detached HEAD or on error.
pub(crate) async fn worktree_current_branch(worktree_path: &Path) -> Option<String> {
    match git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"], worktree_path).await {
        Ok(o) if o.status.success() => {
            let branch = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if branch == "HEAD" {
                None
            } else {
                Some(branch)
            }
        }
        _ => None,
    }
}

/// Check if a branch has commits vs main (i.e., the branch has diverged from the default branch).
/// Returns `true` if there are commits on the branch not on main, or on error (safe default).
///
/// Uses `main` as the base ref rather than `HEAD` because the repo's checked-out
/// branch may differ from `main` (especially for external repos), which would give
/// wrong results.
pub(crate) async fn has_branch_commits(repo_root: &Path, branch_name: &str) -> bool {
    let base = default_local_branch(repo_root).await;
    let range = format!("{}..{}", base, branch_name);
    match git_cmd(&["log", "--oneline", &range], repo_root).await {
        Ok(o) if o.status.success() => !o.stdout.is_empty(),
        Ok(o) => {
            log!(
                "[Git] git log failed for branch {}: {}",
                branch_name,
                String::from_utf8_lossy(&o.stderr).trim()
            );
            true
        }
        Err(e) => {
            log!("[Git] git log failed for branch {}: {}", branch_name, e);
            true
        }
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

/// Get the list of changed files between main and a branch (three-dot merge-base diff).
/// Strips engine-injected paths — see `is_engine_injected_path` for rationale.
pub(crate) async fn branch_changed_files(repo_root: &Path, branch_name: &str) -> Vec<String> {
    let base = default_local_branch(repo_root).await;
    let range = format!("{}...{}", base, branch_name);
    git_cmd(&["diff", "--name-only", &range], repo_root)
        .await
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !super::claude_code::is_engine_injected_path(l))
                .map(|l| l.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Files a Change proposal should reference for this branch, or `None` when the
/// branch isn't proposal-worthy. `None` covers two cases: no commits ahead of
/// main, or commits whose changes cancel out (commit + revert, e.g. CC's
/// `npm install` lockfile rename + restore — zero net diff). Returning the file
/// list here lets callers skip a second `branch_changed_files` call inside the
/// proposal flow.
pub(crate) async fn proposal_files_for_branch(
    repo_root: &Path,
    branch_name: &str,
) -> Option<Vec<String>> {
    if !has_branch_commits(repo_root, branch_name).await {
        return None;
    }
    let files = branch_changed_files(repo_root, branch_name).await;
    if files.is_empty() {
        None
    } else {
        Some(files)
    }
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

/// Auto-commit uncommitted changes in a worktree with a generic message.
pub(crate) async fn auto_commit_worktree(worktree_path: &Path, message: &str) {
    let has_changes = git_cmd(&["status", "--porcelain"], worktree_path)
        .await
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    if has_changes {
        let _ = git_cmd(&["add", "-A"], worktree_path).await;
        let _ = git_cmd(&["commit", "-m", message], worktree_path).await;
    }
}

/// Auto-commit changes in a worktree, preserving the harden marker if fresh.
///
/// The auto-commit can create a new commit (e.g. .claude/ artifacts CC didn't
/// commit), advancing HEAD and invalidating the harden marker even though
/// /harden already reviewed the working tree. This function checks the marker
/// BEFORE committing and re-stamps it afterward with the new HEAD SHA.
///
/// Short-circuits: if the worktree has no uncommitted files, no marker check
/// is needed (HEAD won't move).
pub(crate) async fn auto_commit_preserving_marker(
    pool: &sqlx::PgPool,
    worktree_path: &Path,
    repo_root: &Path,
    branch_name: &str,
    message: &str,
) {
    let has_changes = git_cmd(&["status", "--porcelain"], worktree_path)
        .await
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    if !has_changes {
        return;
    }
    let marker_fresh = is_harden_marker_fresh(pool, repo_root, branch_name).await;
    let _ = git_cmd(&["add", "-A"], worktree_path).await;
    let _ = git_cmd(&["commit", "-m", message], worktree_path).await;
    if marker_fresh {
        if let Some(sha) = current_head_sha(worktree_path).await {
            if let Err(e) = record_hardened(pool, repo_root, branch_name, &sha).await {
                log!(
                    "[ClaudeCode] Failed to re-stamp hardened_branches for {}: {}",
                    branch_name,
                    e
                );
            } else {
                log!("[ClaudeCode] Re-stamped harden marker after auto-commit");
            }
        }
    }
}

/// Outcome of trying to recover from a "branch has no commits" state in `apply_change`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NoCommitsRecovery {
    /// The branch's worktree had uncommitted work that was just auto-committed,
    /// so the branch now has commits. The caller should fall through to the merge path.
    AutoCommitted,
    /// The branch is genuinely empty AND the change has no declared files.
    /// Safe to mark the change applied as a no-op.
    LegitimateNoOp,
    /// The branch had work, but main already contains it (sibling apply, fast-forward,
    /// out-of-band merge, etc.). `git log main..branch` is empty AND main's history
    /// contains commits touching the change's files. Safe to mark applied as a no-op.
    AlreadyApplied,
}

/// Does main's history contain any commit touching at least one of `change_files`?
///
/// Used to distinguish "branch's work was already merged into main" (no-op) from
/// "branch never produced any commits for the referenced files" (corruption).
/// Files referenced by an applied change should always have a corresponding commit
/// somewhere on main, even if the file was later deleted.
async fn main_history_touches_files(repo_root: &Path, change_files: &[String]) -> bool {
    if change_files.is_empty() {
        return false;
    }
    let base = default_local_branch(repo_root).await;
    let mut args: Vec<String> = vec![
        "log".to_string(),
        "--oneline".to_string(),
        "-1".to_string(),
        base,
        "--".to_string(),
    ];
    args.extend(change_files.iter().cloned());
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    git_cmd(&arg_refs, repo_root)
        .await
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// When `has_branch_commits` returned false, decide what to do about it.
///
/// Behaviour:
///   - If a worktree exists for the branch with uncommitted work, auto-commit it.
///     This rescues the silent-data-loss case where CC left staged-but-uncommitted
///     work on a branch ref that still points at the merge base.
///   - Re-check whether the branch now has commits.
///   - If yes → `AutoCommitted` (caller proceeds with the normal merge).
///   - If still no commits AND `change_files` is empty → `LegitimateNoOp` (safe no-op).
///   - If still no commits AND main's history touches any of `change_files` →
///     `AlreadyApplied` (the work landed on main via a sibling apply, fast-forward,
///     or out-of-band merge — nothing to do).
///   - Otherwise → `Err(...)` — branch is empty AND main has no commits touching the
///     declared files. This is the genuinely-empty case (likely a never-committed
///     draft); discarding the change is safe.
///
/// The branch ref is NOT deleted by this function; the caller decides.
pub(crate) async fn recover_no_commits_branch(
    repo_root: &Path,
    branch_name: &str,
    change_files: &[String],
) -> Result<NoCommitsRecovery, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref wt) = find_worktree_for_branch(repo_root, branch_name).await {
        auto_commit_worktree(wt, "Claude Code changes (pre-apply auto-commit)").await;
    }

    if has_branch_commits(repo_root, branch_name).await {
        return Ok(NoCommitsRecovery::AutoCommitted);
    }

    if change_files.is_empty() {
        return Ok(NoCommitsRecovery::LegitimateNoOp);
    }

    if main_history_touches_files(repo_root, change_files).await {
        return Ok(NoCommitsRecovery::AlreadyApplied);
    }

    let preview = if change_files.len() <= 3 {
        change_files.join(", ")
    } else {
        format!("{}, ...", change_files[..3].join(", "))
    };
    Err(format!(
        "Branch '{}' has no commits and main has no history for the {} file(s) referenced \
         by this change ({}). The work was likely never committed — discard the change to clear it.",
        branch_name,
        change_files.len(),
        preview,
    )
    .into())
}

/// Append each path to the git exclude file that `wt_path` actually reads,
/// idempotently. Existing entries (custom or previously-added paths) are
/// preserved; lines already present are skipped. Best-effort: each step logs
/// and continues on failure so partial success never blocks session start.
///
/// Uses `git rev-parse --git-path info/exclude` to locate the file. For a
/// worktree, git resolves this to the COMMON `.git/info/exclude` (shared
/// across all worktrees) — git silently ignores per-worktree info/exclude
/// files, so writing there has no effect on `git status` / `check-ignore`.
/// Verified empirically against git 2.x.
pub(crate) async fn add_paths_to_worktree_exclude(wt_path: &Path, paths: &[&str]) {
    use tokio::io::AsyncWriteExt;

    if paths.is_empty() {
        return;
    }

    let exclude_file = match git_cmd(&["rev-parse", "--git-path", "info/exclude"], wt_path).await {
        Ok(out) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if raw.is_empty() {
                log!(
                    "[Git] git rev-parse returned empty info/exclude path in {}",
                    wt_path.display()
                );
                return;
            }
            let p = PathBuf::from(&raw);
            if p.is_absolute() {
                p
            } else {
                wt_path.join(p)
            }
        }
        Ok(out) => {
            log!(
                "[Git] git rev-parse --git-path info/exclude failed in {}: {}",
                wt_path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
            return;
        }
        Err(e) => {
            log!("[Git] {}", e);
            return;
        }
    };

    if let Some(parent) = exclude_file.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            log!("[Git] Failed to create {}: {}", parent.display(), e);
            return;
        }
    }

    let existing = tokio::fs::read_to_string(&exclude_file)
        .await
        .unwrap_or_default();
    let already: std::collections::HashSet<&str> =
        existing.lines().map(|l| l.trim()).collect();

    let to_add: Vec<&str> = paths
        .iter()
        .copied()
        .filter(|p| !already.contains(p))
        .collect();
    if to_add.is_empty() {
        return;
    }

    let mut payload = String::new();
    if !existing.is_empty() && !existing.ends_with('\n') {
        payload.push('\n');
    }
    for p in to_add {
        payload.push_str(p);
        payload.push('\n');
    }

    match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude_file)
        .await
    {
        Ok(mut f) => {
            if let Err(e) = f.write_all(payload.as_bytes()).await {
                log!(
                    "[Git] Failed to write {}: {}",
                    exclude_file.display(),
                    e
                );
            }
        }
        Err(e) => log!(
            "[Git] Failed to open {}: {}",
            exclude_file.display(),
            e
        ),
    }
}

#[cfg(test)]
#[path = "git_ops_tests.rs"]
mod tests;
