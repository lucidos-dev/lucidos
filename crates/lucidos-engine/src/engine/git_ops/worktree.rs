use super::*;
use std::path::{Path, PathBuf};

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
pub(crate) async fn main_worktree() -> PathBuf {
    resolve_main_worktree(&crate::paths::repo_root_or_compile_time_fallback()).await
}

/// Resolve the main working tree from a path that might be a git worktree.
/// `git worktree list --porcelain` always lists the main working tree first.
/// Falls back to the input path if git isn't available or the path isn't a repo.
///
/// If the compile-time path no longer exists (e.g., the engine was built in a
/// CC worktree that was later cleaned up), falls back to `std::env::current_dir()`
/// which is typically set to the repo root by the startup scripts.
pub(crate) async fn resolve_main_worktree(path: &Path) -> PathBuf {
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


/// Generate the app coding-agent thread branch name:
/// `claude-code/app/<app_id>/<ts>-<uuid>`. Embeds the app id in the path so a
/// `git branch -a` on the workspace git is greppable by app.
pub(crate) fn generate_app_branch_name(app_id: &str) -> String {
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let suffix = &uuid::Uuid::new_v4().as_simple().to_string()[..6];
    format!("claude-code/app/{}/{}-{}", app_id, ts, suffix)
}

/// Create a sparse-checkout worktree narrowed to `data/apps/<app_id>/` on a
/// new branch. The worktree is anchored at `workspace_root` (the workspace
/// git, not Lucidos source). After this returns successfully, the worktree
/// has only the app folder plus top-level files (`.gitignore`, marker file)
/// materialised on disk.
///
/// Sequence (cone mode):
/// 1. `git worktree add --no-checkout <wt_path> -b <branch>` off `main`.
/// 2. `git sparse-checkout init --cone`.
/// 3. `git sparse-checkout set data/apps/<app_id>`.
/// 4. `git checkout <branch>`.
///
/// On any step's failure, the worktree dir is removed so the caller can
/// retry without `git worktree add` complaining about a stale dir.
pub(crate) async fn create_sparse_app_worktree(
    workspace_root: &Path,
    app_id: &str,
    branch_name: &str,
    wt_path: &Path,
) -> Result<(), String> {
    let wt_str = wt_path
        .to_str()
        .ok_or_else(|| format!("non-utf8 worktree path: {}", wt_path.display()))?;

    let cleanup = || async {
        // Best-effort: a failed worktree add may have left an empty dir
        // behind that the next attempt would refuse to overwrite.
        let _ = git_cmd(&["worktree", "remove", "--force", wt_str], workspace_root).await;
    };

    // Step 1: create the worktree on a fresh branch off the workspace
    // git's default branch, without checking anything out. Without
    // --no-checkout, git would materialise every tracked file first and
    // then sparse-checkout would prune — a pointless O(repo) copy before
    // the prune. Use `default_local_branch` (not a hardcoded `main`) so
    // workspaces whose default branch is `master` / `trunk` / a custom
    // name still spawn correctly.
    let base = default_local_branch(workspace_root).await;
    let add = worktree_add_pruning_stale(
        workspace_root,
        &["worktree", "add", "--no-checkout", wt_str, "-b", branch_name, &base],
    )
    .await?;
    if !add.status.success() {
        let stderr = String::from_utf8_lossy(&add.stderr).trim().to_string();
        cleanup().await;
        return Err(format!("git worktree add failed: {stderr}"));
    }

    // Step 2: enable cone-mode sparse-checkout in the new worktree. Cone
    // mode is the recommended mode — it works at the directory level (not
    // pathspec level) which is exactly what we need to narrow to one
    // `data/apps/<id>/` folder.
    let init = git_cmd(&["sparse-checkout", "init", "--cone"], wt_path).await?;
    if !init.status.success() {
        let stderr = String::from_utf8_lossy(&init.stderr).trim().to_string();
        cleanup().await;
        return Err(format!("git sparse-checkout init failed: {stderr}"));
    }

    // Step 3: narrow to the app folder. Cone mode also always materialises
    // top-level files (workspace `.gitignore`, our marker) — so the worktree
    // root stays usable for `git status` / engine markers even though most
    // of `data/` is pruned.
    let app_path = format!("data/apps/{}", app_id);
    let set = git_cmd(&["sparse-checkout", "set", &app_path], wt_path).await?;
    if !set.status.success() {
        let stderr = String::from_utf8_lossy(&set.stderr).trim().to_string();
        cleanup().await;
        return Err(format!("git sparse-checkout set failed: {stderr}"));
    }

    // Step 4: now materialise the sparse cone on the new branch. Without
    // this, the worktree HEAD is set but the working tree is empty.
    let checkout = git_cmd(&["checkout", branch_name], wt_path).await?;
    if !checkout.status.success() {
        let stderr = String::from_utf8_lossy(&checkout.stderr).trim().to_string();
        cleanup().await;
        return Err(format!("git checkout {branch_name} failed: {stderr}"));
    }

    if let Err(e) = link_git_crypt_dir(wt_path).await {
        log!(
            "[Git] git-crypt key symlink for {} failed (encrypted files may not decrypt): {}",
            wt_path.display(),
            e
        );
    }

    Ok(())
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
    let add_out = worktree_add_pruning_stale(repo_root, &args).await?;
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

/// Run `git worktree add <args>` from `repo_root`, first clearing any stale
/// registration. A worktree directory deleted without `git worktree remove`
/// leaves an entry in `$GIT_DIR/worktrees`; git then refuses to re-add at the
/// same path with "missing but already registered worktree". Pruning up front
/// (rather than on failure) matters because git creates the `-b` branch *before*
/// it hits that check — so a naive retry would then trip over "a branch already
/// exists". `git worktree prune` removes only registrations whose working tree
/// is gone (live and locked worktrees, and any path that still exists on disk,
/// are untouched), so it is safe and idempotent: it clears only garbage and
/// lets the add create its branch exactly once. A CC spawn reusing the
/// deterministic `thread-<id>` path would otherwise die with an Event stream
/// error.
async fn worktree_add_pruning_stale(
    repo_root: &Path,
    args: &[&str],
) -> Result<std::process::Output, String> {
    match git_cmd(&["worktree", "prune"], repo_root).await {
        Ok(p) if p.status.success() => {}
        Ok(p) => log!(
            "[Git] git worktree prune returned non-zero in {}: {}",
            repo_root.display(),
            String::from_utf8_lossy(&p.stderr).trim()
        ),
        Err(e) => log!(
            "[Git] git worktree prune failed in {}: {}",
            repo_root.display(),
            e
        ),
    }
    git_cmd(args, repo_root).await
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

/// Hide an engine-injected file that has become *tracked* in the repo from
/// `git status` / `git diff`, so a CC session never sees it as a phantom change
/// it didn't author.
///
/// `.git/info/exclude` (see `add_paths_to_worktree_exclude`) only suppresses
/// *untracked* files — git is silent about excludes for already-tracked paths.
/// When an engine-injected path was committed in the past (e.g. an app
/// coding-agent thread auto-committed the lucidos-cli `SKILL.md` before the
/// deep-path exclude existed), every later session overwrites that
/// tracked-but-stale file on disk with the engine's newer embedded copy,
/// producing a phantom `M` that CC then wastes context reasoning about and may
/// revert or carry into its change.
///
/// We mark the path `--skip-worktree` so git ignores the on-disk divergence.
/// Gated on an actual index↔worktree diff: in the Lucidos repo, where
/// `.claude/skills/lucidos-cli/SKILL.md` is intentionally tracked and identical
/// to the embedded copy, there is no diff, so the file is left untouched and
/// stays normally editable (skip-worktree would otherwise silently swallow a
/// legitimate edit to the skill source). Best-effort: logs and returns on any
/// git failure so session start is never blocked.
///
/// Why skip-worktree is safe here even though it was abandoned for VERSION
/// files (see `c7c941b49` "Removed all skip-worktree/reset-version hacks"): that
/// failure needed `main` to keep *mutating* the skip-worktree'd path, so
/// `catchup_with_main`'s `git merge main` hit "local changes would be
/// overwritten" — and `build.rs` re-committed VERSION on every build. Neither
/// holds for the skill: app coding-agent threads skip `catchup_with_main`
/// entirely (the real trigger for this guard), and skip-worktree blocks
/// `git add -A` from ever committing the divergent copy, so no guarded session
/// can push a change to this path onto `main` — main's copy stays frozen and the
/// merge never sees an incoming change. Even a worst-case conflict is non-fatal:
/// `catchup_with_main` self-aborts and its caller only logs.
///
/// `cwd` is the directory the engine wrote the file relative to — CC's cwd: the
/// app folder for app coding-agent threads, the worktree root otherwise.
pub(crate) async fn hide_phantom_tracked_skill(cwd: &Path, rel_path: &str) {
    // Tracked? `ls-files --error-unmatch` exits non-zero for untracked paths;
    // those are already hidden by `.git/info/exclude`, nothing to do here.
    match git_cmd(&["ls-files", "--error-unmatch", "--", rel_path], cwd).await {
        Ok(o) if o.status.success() => {}
        Ok(_) => return,
        Err(e) => {
            log!("[Git] {}", e);
            return;
        }
    }

    // Phantom? `diff --quiet` exits 0 when the working tree matches the index.
    // Only divergence (the engine's embedded copy over a stale tracked one)
    // warrants skip-worktree — a clean, legitimately-tracked copy stays editable.
    match git_cmd(&["diff", "--quiet", "--", rel_path], cwd).await {
        Ok(o) if o.status.success() => return,
        Ok(_) => {}
        Err(e) => {
            log!("[Git] {}", e);
            return;
        }
    }

    match git_cmd(&["update-index", "--skip-worktree", "--", rel_path], cwd).await {
        Ok(o) if o.status.success() => {}
        Ok(o) => log!(
            "[Git] skip-worktree on {} failed: {}",
            rel_path,
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log!("[Git] {}", e),
    }
}
