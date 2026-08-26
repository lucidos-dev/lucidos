use super::*;
use rand::Rng;
use std::path::{Path, PathBuf};
use std::time::Duration;

const LUCIDOS_HOOKS_PATH: &str = ".lucidos/git-hooks";
const LUCIDOS_POST_COMMIT_HOOK: &str = "post-commit";
const LUCIDOS_CHAIN_MARKER: &str = "# lucidos-chain-hook: ";

/// How many hex chars of a thread id go into an engine-generated name. Two
/// names carry it and they must agree: the worktree directory
/// `thread-<short>` and the `<short>` segment of the thread's branch
/// (`branch_name::coding_agent_branch_base`). One constant so a branch and its
/// worktree always show the same id, and reading one tells you the other.
///
/// 8 hex is for legibility, not for collision resistance: uniqueness comes
/// from the thread id itself, and the full id is recorded in the
/// `CodingAgentIdled` payload for unambiguous lookup.
pub(crate) const SHORT_THREAD_ID_LEN: usize = 8;

/// The [`SHORT_THREAD_ID_LEN`]-char prefix of a thread id, lowercase hex.
///
/// `chars().take` rather than a byte slice: the rule is that a uuid's
/// ASCII-ness should not be what keeps an index from panicking.
pub(crate) fn short_thread_id(thread_id: uuid::Uuid) -> String {
    thread_id
        .as_simple()
        .to_string()
        .chars()
        .take(SHORT_THREAD_ID_LEN)
        .collect()
}

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

/// Which worktree holds a branch, keeping "could not ask" separate from
/// "nothing holds it". The [`GitAnswer`] shape, for a question whose yes-side
/// carries a path.
///
/// `Unknown` is the variant that has to stay distinguishable. `git worktree
/// list` failed to spawn, exited non-zero, or exceeded [`GIT_TIMEOUT`], all of
/// which are routine on a saturated host. Collapsing that into `NotFound` is
/// the bug class in `docs/plans/2026-08-03-unknown-git-state-must-not-delete-worktrees.md`.
///
/// So there is no collapse to `Option<PathBuf>`, no `Default`, no
/// `unwrap_or`-shaped method. Every caller matches three arms, and the
/// `Unknown` arm must skip whatever it was about to do and log which branch it
/// skipped. It must never delete a worktree, delete or move a branch ref, or
/// resolve a change as an already-applied no-op. The per-site decisions are
/// tabulated in `docs/plans/2026-08-24-find-worktree-for-branch-is-a-tri-state.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorktreeLookup {
    /// git ran and this worktree has the branch checked out.
    Found(PathBuf),
    /// git ran and no worktree has the branch checked out.
    NotFound,
    /// git could not be asked: spawn failure, non-zero exit, or timeout.
    Unknown,
}

/// Ask which worktree currently holds `branch_name`.
///
/// A non-zero exit is [`WorktreeLookup::Unknown`], not a `NotFound`. The answer
/// comes from parsing stdout, so a failed exit leaves no listing to parse
/// rather than an empty one.
pub(crate) async fn find_worktree_for_branch(
    repo_root: &Path,
    branch_name: &str,
) -> WorktreeLookup {
    let output = match git_cmd(&["worktree", "list", "--porcelain"], repo_root).await {
        Ok(o) => o,
        Err(e) => {
            log!(
                "[Git] git worktree list failed in {}: {}",
                repo_root.display(),
                e
            );
            return WorktreeLookup::Unknown;
        }
    };
    if !output.status.success() {
        log!(
            "[Git] git worktree list returned non-zero in {}: {}",
            repo_root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return WorktreeLookup::Unknown;
    }
    match parse_worktree_list(&String::from_utf8_lossy(&output.stdout)).remove(branch_name) {
        Some(path) => WorktreeLookup::Found(path),
        None => WorktreeLookup::NotFound,
    }
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
            "[Git] Compile-time path {} no longer exists — trying cwd fallback",
            path.display()
        );
        match std::env::current_dir() {
            Ok(cwd) if cwd.join(".git").exists() => {
                log!("[Git] Using cwd as fallback repo root: {}", cwd.display());
                cwd
            }
            _ => {
                log!(
                    "[Git] No fallback available for non-existent compile-time path {}",
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
                        "[Git] Resolved compile-time path to main working tree: {} -> {}",
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

/// Create a sparse-checkout worktree narrowed to `data/apps/<app_id>/`. The
/// worktree is anchored at `workspace_root` (the workspace git, not Lucidos
/// source). After this returns successfully, the worktree has only the app
/// folder plus top-level files (`.gitignore`, marker file) materialised on
/// disk.
///
/// If `branch_name` already exists it is reused (checked out into the new
/// worktree, preserving its commits); otherwise it is created off the
/// default branch. Reuse is keyed purely on ref existence, not resume
/// intent: callers pass either a freshly allocated name
/// (`branch_name::allocate_coding_agent_branch`, unique against the repo's
/// refs at allocation time) or a validated
/// resume branch, so the fresh-branch-at-default-tip guarantee is
/// caller-supplied naming discipline, and the second probe (the caller's
/// `resolve_branch_for_resume` already checked once) makes the recreate
/// paths self-healing. Reuse is what makes resume work: a thread's worktree
/// dir can be torn down while its branch survives with the session's
/// committed work — an unconditional `-b` there fails with "branch already
/// exists" and strands the thread. (The generic spawn path expresses the
/// same split via `reusing_branch` in `spawn_context.rs`.)
///
/// Returns `true` if this call created the branch (`-b`), `false` if it
/// reused an existing one — feeds the caller's failure-cleanup decision
/// (only a branch born here may be deleted on a failed spawn) and its log.
///
/// Sequence (cone mode):
/// 1. `git worktree add --no-checkout --no-track <wt_path> -b <branch> <base>`
///    (or `git worktree add --no-checkout <wt_path> <branch>` on reuse).
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
) -> Result<bool, String> {
    let wt_str = wt_path
        .to_str()
        .ok_or_else(|| format!("non-utf8 worktree path: {}", wt_path.display()))?;

    let cleanup = || async {
        // Best-effort: a failed worktree add may have left an empty dir
        // behind that the next attempt would refuse to overwrite.
        let _ = git_cmd(&["worktree", "remove", "--force", wt_str], workspace_root).await;
    };

    // Step 1: create the worktree without checking anything out. Without
    // --no-checkout, git would materialise every tracked file first and
    // then sparse-checkout would prune — a pointless O(repo) copy before
    // the prune. An existing branch (resume) is checked out as-is; a fresh
    // one is created off the workspace git's default branch — via
    // `default_local_branch` (not a hardcoded `main`) so workspaces whose
    // default branch is `master` / `trunk` / a custom name still spawn
    // correctly — with `--no-track` so concurrent spawns can't race on the
    // shared `.git/config` upstream write (same rationale as `worktree_add`).
    let branch_exists = git_ref_exists(workspace_root, &format!("refs/heads/{branch_name}")).await;
    let add = if branch_exists {
        worktree_add_pruning_stale(
            workspace_root,
            &["worktree", "add", "--no-checkout", wt_str, branch_name],
        )
        .await?
    } else {
        let base = default_local_branch(workspace_root).await;
        worktree_add_pruning_stale(
            workspace_root,
            &[
                "worktree",
                "add",
                "--no-checkout",
                "--no-track",
                wt_str,
                "-b",
                branch_name,
                &base,
            ],
        )
        .await?
    };
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

    // Step 4: now materialise the sparse cone on the branch. Without this,
    // the worktree HEAD is set but the working tree is empty.
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

    Ok(!branch_exists)
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
    let mut args: Vec<&str> = vec!["worktree", "add", "--no-checkout"];
    // Engine-created isolation branches (`-b <name>`) must NOT set up upstream
    // tracking. They're throwaway `lucidos-*` coding-agent branches merged into
    // main locally and never pushed on their own, so tracking config is dead
    // weight — AND it is the ONLY thing `git worktree add` writes to the shared
    // `.git/config`. With `branch.autoSetupMerge` in play that write races on
    // `.git/config.lock` whenever several coding-agent spawns start at once,
    // failing the spawn with "could not lock config file .git/config: File
    // exists" / "unable to write upstream branch configuration". `--no-track`
    // suppresses the write entirely (only valid when creating a branch), so
    // concurrent spawns can no longer collide here.
    if extra_args.iter().any(|a| *a == "-b" || *a == "-B") {
        args.push("--no-track");
    }
    args.push(wt_str);
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
///
/// The pair runs under [`WORKTREE_ADMIN_MUTEX`], so one spawn's prune cannot
/// delete another spawn's half-created admin directory. The lock covers the add
/// as well, because the add is what owns that directory. Work after the add
/// runs unlocked: `gitdir` is on disk by then, and a prune leaves the entry
/// alone.
async fn worktree_add_pruning_stale(
    repo_root: &Path,
    args: &[&str],
) -> Result<std::process::Output, String> {
    let _guard = WORKTREE_ADMIN_MUTEX.lock().await;
    prune_worktrees_locked(repo_root).await;
    git_cmd_kill_on_timeout(args, repo_root).await
}

/// Serialises `git worktree prune` against `git worktree add` in this process.
///
/// A prune deletes every `$GIT_DIR/worktrees/<id>` that holds no `gitdir` file,
/// with no grace period. An add creates that directory a few syscalls BEFORE it
/// writes the file. A prune landing in that window deletes a sibling's admin
/// directory, and the sibling dies on `could not open .../gitdir for writing`.
/// Six spawns from one response is the shape that hits it, since each prunes
/// before it adds.
///
/// One global mutex, not one per repo. Spawns against different repos cannot
/// corrupt each other, but an add takes milliseconds, so a keyed map is cost
/// with no payoff. A leaf in the lock order: below it sits only a `git_cmd`
/// subprocess.
///
/// A guard binds only what it can see. So BOTH halves run under
/// [`super::git_cmd_kill_on_timeout`]: a timed-out one outlives the guard and
/// races whichever spawn takes it next. A prune from ANOTHER process still
/// lands in that window: the release and e2e scripts each run one. Git takes
/// no repo lock that would cover them.
pub(super) static WORKTREE_ADMIN_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Clear registrations whose working tree is gone. The caller holds
/// [`WORKTREE_ADMIN_MUTEX`] for the sweep.
///
/// It logs a failed prune and carries on, because the caller's own step reports
/// whatever the leftover garbage then breaks.
async fn prune_worktrees_locked(repo_root: &Path) {
    match git_cmd_kill_on_timeout(&["worktree", "prune"], repo_root).await {
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
    let git_dir = lines
        .next()
        .ok_or("rev-parse: missing --absolute-git-dir")?;
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

/// True when `wt_path` is a live, linked git worktree rooted at `wt_path`
/// itself — not a stranded directory, and not a path git resolves *upward* to an
/// enclosing repository (e.g. the workspace data repo that physically contains
/// `.lucidos/worktrees/`). A resumed Claude Code session must never run in such
/// a directory: git there operates on the enclosing repo, so the session reads
/// the wrong files and could commit into the data repo — the "worktree torn
/// down" failure where a torn-down worktree left the session standing in the
/// workspace's git root.
///
/// The spawn path uses this to decide reuse-vs-recreate instead of the weaker
/// "directory + `.git` entry exists" test, which a stranded tree passes (its
/// `.git` file can survive while the admin dir it points at is gone, or git
/// simply walks up to the enclosing repo).
///
/// An unanswered probe reads as **live**. See [`worktree_liveness`] for why:
/// every caller of this function reacts to "not live" by clearing and
/// recreating the directory, so an unanswered probe that resolved to `false`
/// would authorize deleting a tree that is fine.
pub(crate) async fn is_live_worktree_at(wt_path: &Path) -> bool {
    worktree_liveness(wt_path).await.or_unknown(true)
}

/// Tri-state form of [`is_live_worktree_at`]: `Yes` when git confirms the path
/// is its own work tree, `No` when git confirms it is not (no repo at all, or
/// it resolves upward to an enclosing one), `Unknown` when git could not be
/// asked.
///
/// The `Unknown` arm is load-bearing. On 2026-08-03, with the host saturated by
/// the e2e suite, `git rev-parse --show-toplevel` exceeded the 30s ceiling
/// against a perfectly healthy worktree. The old code mapped that straight to
/// "not a live worktree", the spawn path concluded the tree was stranded, and
/// the session-end cleanup then ran `git worktree remove --force` over the
/// user's work. Treating `Unknown` as live is the recoverable direction: if the
/// tree really is broken, the next git call inside it fails loudly and the
/// session surfaces an error, which costs a retry rather than a working copy.
pub(crate) async fn worktree_liveness(wt_path: &Path) -> GitAnswer {
    if !wt_path.exists() {
        return GitAnswer::No;
    }
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    git_answer_with(&["rev-parse", "--show-toplevel"], wt_path, |o| {
        let toplevel = String::from_utf8_lossy(&o.stdout).trim().to_string();
        !toplevel.is_empty() && canon(Path::new(&toplevel)) == canon(wt_path)
    })
    .await
}

/// Attempts for a shared-`.git/config` write that loses the `.git/config.lock`
/// race against a concurrent git process.
const CONFIG_LOCK_RETRIES: u32 = 8;

/// True when a git invocation's stderr is the transient `.git/config.lock`
/// contention error — git's lockfile fails fast (it does not block) when
/// another process holds the lock, expecting the caller to retry.
fn is_config_lock_contention(stderr: &str) -> bool {
    stderr.contains("could not lock config file")
}

/// Set `key=value` in the repo's SHARED `.git/config`, retrying when a
/// concurrent git process holds `.git/config.lock`. The engine spawns many
/// coding-agent worktrees at once, each touching the shared config; git's
/// lockfile fails fast on contention rather than blocking, so the engine must
/// retry to be a well-behaved concurrent client. Backoff is jittered so
/// simultaneous losers don't resynchronize and collide again. The caller is
/// responsible for ensuring the write is idempotent (safe to repeat).
async fn set_shared_config_with_lock_retry(
    repo_root: &Path,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let mut last_err = String::new();
    for attempt in 0..CONFIG_LOCK_RETRIES {
        let out = git_cmd(&["config", key, value], repo_root).await?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if !is_config_lock_contention(&stderr) {
            return Err(format!("git config {key} {value} failed: {stderr}"));
        }
        last_err = stderr;
        // No point sleeping after the final attempt — we're about to give up.
        if attempt + 1 < CONFIG_LOCK_RETRIES {
            // Grow the base delay with the attempt, plus a random spread so
            // concurrent losers retry at different moments.
            let base = 20u64 * (attempt as u64 + 1);
            let jitter = rand::thread_rng().gen_range(0..40);
            tokio::time::sleep(Duration::from_millis(base + jitter)).await;
        }
    }
    Err(format!(
        "git config {key} {value} failed after {CONFIG_LOCK_RETRIES} attempts (config lock contention): {last_err}"
    ))
}

/// Install the per-worktree post-commit hook that lets a coding-agent commit
/// refresh the visible Diff button immediately, before the turn idles.
///
/// The hook is deliberately worktree-local (`core.hooksPath` in
/// `config.worktree`) so it affects only Lucidos-managed agent worktrees, not
/// the user's main checkout or sibling worktrees. If the repo already had a
/// post-commit hook through the previous `core.hooksPath` (or the common
/// `.git/hooks` directory), the Lucidos hook chains to it after refreshing the
/// diff flag.
pub(crate) async fn install_coding_agent_diff_hook(
    repo_root: &Path,
    wt_path: &Path,
) -> Result<(), String> {
    if !is_live_worktree_at(wt_path).await {
        return Err(format!("{} is not a live git worktree", wt_path.display()));
    }

    let chain_hook = prior_post_commit_hook(wt_path).await?;
    let hook_dir = wt_path.join(LUCIDOS_HOOKS_PATH);
    let hook_path = hook_dir.join(LUCIDOS_POST_COMMIT_HOOK);
    std::fs::create_dir_all(&hook_dir)
        .map_err(|e| format!("create {}: {}", hook_dir.display(), e))?;

    std::fs::write(
        &hook_path,
        render_coding_agent_diff_hook(&chain_hook).as_bytes(),
    )
    .map_err(|e| format!("write {}: {}", hook_path.display(), e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&hook_path)
            .map_err(|e| format!("metadata {}: {}", hook_path.display(), e))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&hook_path, perms)
            .map_err(|e| format!("chmod {}: {}", hook_path.display(), e))?;
    }

    // `extensions.worktreeConfig` lives in the SHARED `.git/config`, so under
    // concurrent coding-agent spawns its write races on `.git/config.lock` the
    // same way the worktree-add tracking write did. It's an idempotent
    // repo-global flag: read it first and skip the write once it's enabled (the
    // steady state — every spawn after the first), and on the cold-start write
    // retry through lock contention. Without this, six near-simultaneous spawns
    // could all lose the lock and silently fail to install the diff hook.
    let already_enabled = matches!(
        git_cmd(&["config", "--get", "extensions.worktreeConfig"], repo_root).await,
        Ok(out) if out.status.success()
            && String::from_utf8_lossy(&out.stdout).trim() == "true"
    );
    if !already_enabled {
        set_shared_config_with_lock_retry(repo_root, "extensions.worktreeConfig", "true").await?;
    }

    let set_hooks = git_cmd(
        &["config", "--worktree", "core.hooksPath", LUCIDOS_HOOKS_PATH],
        wt_path,
    )
    .await?;
    if !set_hooks.status.success() {
        return Err(format!(
            "git config --worktree core.hooksPath failed: {}",
            String::from_utf8_lossy(&set_hooks.stderr).trim()
        ));
    }

    Ok(())
}

async fn prior_post_commit_hook(wt_path: &Path) -> Result<PathBuf, String> {
    let hook_path = wt_path
        .join(LUCIDOS_HOOKS_PATH)
        .join(LUCIDOS_POST_COMMIT_HOOK);
    if let Some(current) = current_hooks_path(wt_path).await {
        if !is_lucidos_hooks_path(&current) {
            return Ok(resolve_hooks_path(wt_path, &current).join(LUCIDOS_POST_COMMIT_HOOK));
        }
        if let Some(chain) = existing_lucidos_chain_hook(&hook_path) {
            return Ok(chain);
        }
    }
    common_post_commit_hook(wt_path).await
}

async fn current_hooks_path(wt_path: &Path) -> Option<String> {
    match git_cmd(&["config", "--get", "core.hooksPath"], wt_path).await {
        Ok(out) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if raw.is_empty() {
                None
            } else {
                Some(raw)
            }
        }
        _ => None,
    }
}

fn is_lucidos_hooks_path(path: &str) -> bool {
    path.trim_end_matches('/') == LUCIDOS_HOOKS_PATH
}

fn resolve_hooks_path(wt_path: &Path, hooks_path: &str) -> PathBuf {
    let path = Path::new(hooks_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        wt_path.join(path)
    }
}

fn existing_lucidos_chain_hook(hook_path: &Path) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(hook_path).ok()?;
    contents.lines().find_map(|line| {
        line.strip_prefix(LUCIDOS_CHAIN_MARKER)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    })
}

async fn common_post_commit_hook(wt_path: &Path) -> Result<PathBuf, String> {
    let out = git_cmd(&["rev-parse", "--git-common-dir"], wt_path).await?;
    if !out.status.success() {
        return Err(format!(
            "git rev-parse --git-common-dir failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return Err("git rev-parse --git-common-dir returned empty path".to_string());
    }
    let common_dir = {
        let p = PathBuf::from(raw);
        if p.is_absolute() {
            p
        } else {
            wt_path.join(p)
        }
    };
    Ok(common_dir.join("hooks").join(LUCIDOS_POST_COMMIT_HOOK))
}

fn render_coding_agent_diff_hook(chain_hook: &Path) -> String {
    let chain_display = chain_hook.to_string_lossy();
    let chain_literal = shell_single_quote(&chain_display);
    format!(
        "#!/bin/sh\n\
         # Installed by Lucidos. Refreshes the coding-agent Diff button as soon as a commit lands.\n\
         {LUCIDOS_CHAIN_MARKER}{chain_display}\n\
         if [ -n \"${{LUCIDOS_THREAD_ID:-}}\" ]; then\n\
           lucidos coding-agent-diff-hook >/dev/null 2>&1 || true\n\
         fi\n\
         \n\
         CHAIN={chain_literal}\n\
         if [ -n \"$CHAIN\" ] && [ -x \"$CHAIN\" ] && [ \"$CHAIN\" != \"$0\" ]; then\n\
           \"$CHAIN\" \"$@\"\n\
           exit $?\n\
         fi\n\
         exit 0\n"
    )
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Remove a *stranded* worktree directory (present on disk but not a live linked
/// worktree — see [`is_live_worktree_at`]) so a fresh [`worktree_add`] can
/// recreate it at the same path.
///
/// BOTH destructive steps — `git worktree remove --force` (which would delete
/// even a live, dirty tree) and `remove_dir_all` — fire ONLY after *positive*
/// stranding evidence: git ran successfully and confirmed the path is not its
/// own work tree (resolves to an enclosing repo, or is no git repo at all). A
/// path that still reads as a live worktree, or that git could not be queried
/// about (transient failure), is left untouched — deleting a possibly-live
/// worktree's uncommitted work is the unsafe direction, and the caller instead
/// surfaces the `git worktree add` "already exists" collision. Confine calls to
/// the workspace worktrees dir.
pub(crate) async fn clear_stranded_worktree_dir(repo_root: &Path, wt_path: &Path) {
    if !wt_path.exists() {
        return;
    }

    // Confirm the path is stranded BEFORE touching it. This gate must precede
    // `git worktree remove --force` too, not just the `remove_dir_all` — that
    // command silently discards a live, dirty worktree, so a transient
    // liveness-probe failure upstream must not be able to reach it here.
    // Only a positive `No` (git ran and said this is not its own work tree)
    // authorizes the removal below.
    match worktree_liveness(wt_path).await {
        GitAnswer::Yes => {
            log!(
                "[Git] refusing to clear {}: it reads as a live worktree",
                wt_path.display()
            );
            return;
        }
        GitAnswer::Unknown => {
            log!(
                "[Git] cannot verify {} is stranded (git gave no answer); leaving dir in place",
                wt_path.display()
            );
            return;
        }
        // git ran and confirmed this is not its own work tree: it resolves to
        // an enclosing repo, or is no repo at all. Leftover residue, safe.
        GitAnswer::No => {}
    }

    // Confirmed stranded. Clear any stale git registration first (so a later
    // `worktree_add` at this path doesn't trip "already registered"), then the
    // directory if it survives.
    if let Some(s) = wt_path.to_str() {
        let _ = git_cmd(&["worktree", "remove", "--force", s], repo_root).await;
    }
    if !wt_path.exists() {
        log!(
            "[Git] cleared stranded worktree {} via git",
            wt_path.display()
        );
        return;
    }
    match tokio::fs::remove_dir_all(wt_path).await {
        Ok(()) => log!("[Git] cleared stranded worktree dir {}", wt_path.display()),
        Err(e) => log!(
            "[Git] failed to clear stranded worktree dir {}: {}",
            wt_path.display(),
            e
        ),
    }
}

/// Does this worktree have uncommitted changes?
///
/// `Unknown` when git could not say, which includes a non-zero `git status`
/// (the path is no longer a work tree, or a filter such as git-crypt failed on
/// a locked repo) as well as spawn errors and timeouts. Every caller of this is
/// deciding whether a tree is safe to throw away, so the only answer that may
/// authorize that is a positive `No`. This is the single definition of the
/// question: the background cleanup worker and the session-end path both
/// collapse it with `or_unknown(true)`, so an unanswerable tree counts as dirty
/// in both.
pub(crate) async fn worktree_dirtiness(worktree: &Path) -> GitAnswer {
    git_answer_when_ok(&["status", "--porcelain"], worktree, |o| {
        !o.stdout.is_empty()
    })
    .await
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
    let already: std::collections::HashSet<&str> = existing.lines().map(|l| l.trim()).collect();

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
                log!("[Git] Failed to write {}: {}", exclude_file.display(), e);
            }
        }
        Err(e) => log!("[Git] Failed to open {}: {}", exclude_file.display(), e),
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

/// Best-effort cleanup after a failed coding-agent spawn (`spawn_or_resume`
/// errored after the worktree context was resolved). Deletes ONLY what the
/// failed attempt itself created:
///
/// - the worktree, when `worktree_created` — a pre-existing worktree
///   (resume, recovery, conflict mode) holds the thread's work and the next
///   spawn attempt reuses it;
/// - the branch, when `branch_created` AND the worktree slot is actually
///   gone — a branch checked out in a surviving worktree can't be deleted
///   by git anyway, and deleting it would strip the stranded-worktree
///   sweeper's recovery anchor.
///
/// Every failure is logged. (The old inline shape — two unconditional
/// `let _ =` git calls — force-removed resumed worktrees and force-deleted
/// branches holding committed work, without a trace.)
pub(crate) async fn cleanup_failed_spawn(
    repo_root: &Path,
    worktree_path: Option<&Path>,
    branch_name: &str,
    worktree_created: bool,
    branch_created: bool,
) {
    // "No worktree in play" counts as gone — the branch (if ours) is free.
    let mut worktree_gone = true;
    if let Some(wt) = worktree_path {
        if !worktree_created {
            // Don't return early: fall through to the branch arm so a caller
            // passing (worktree_created=false, branch_created=true) doesn't
            // silently leak its created branch contractually. With the
            // worktree kept, `worktree_gone=false` suppresses the delete in
            // that combination anyway (the branch is checked out there).
            log!(
                "[Git] Spawn failed — keeping pre-existing worktree {} (and branch {}) for resume",
                wt.display(),
                branch_name
            );
            worktree_gone = false;
        } else {
            match git_cmd(
                &["worktree", "remove", "--force", &wt.to_string_lossy()],
                repo_root,
            )
            .await
            {
                Ok(o) if o.status.success() => {
                    log!(
                        "[Git] Removed worktree {} created by the failed spawn",
                        wt.display()
                    );
                }
                Ok(o) => {
                    log!(
                        "[Git] Failed to remove worktree {} after failed spawn: {} — \
                         keeping branch {}; the worktree cleanup worker will retry",
                        wt.display(),
                        String::from_utf8_lossy(&o.stderr).trim(),
                        branch_name
                    );
                    worktree_gone = false;
                }
                Err(e) => {
                    log!(
                        "[Git] git worktree remove errored for {} after failed spawn: {} — \
                         keeping branch {}; the worktree cleanup worker will retry",
                        wt.display(),
                        e,
                        branch_name
                    );
                    worktree_gone = false;
                }
            }
        }
    }
    if branch_created && worktree_gone {
        match git_cmd(&["branch", "-D", branch_name], repo_root).await {
            Ok(o) if o.status.success() => {
                log!(
                    "[Git] Deleted branch {} created by the failed spawn",
                    branch_name
                );
            }
            Ok(o) => log!(
                "[Git] Failed to delete branch {} after failed spawn: {}",
                branch_name,
                String::from_utf8_lossy(&o.stderr).trim()
            ),
            Err(e) => log!(
                "[Git] git branch -D errored for {} after failed spawn: {}",
                branch_name,
                e
            ),
        }
    }
}

#[cfg(test)]
mod config_lock_tests {
    use super::*;

    #[test]
    fn config_lock_contention_recognized() {
        assert!(is_config_lock_contention(
            "error: could not lock config file .git/config: File exists"
        ));
        assert!(!is_config_lock_contention(
            "error: pathspec 'main' did not match any file(s)"
        ));
        assert!(!is_config_lock_contention(""));
    }

    async fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        let _ = git_cmd(&["init", "-q", "-b", "main"], &repo).await;
        let _ = git_cmd(&["config", "user.email", "t@e.com"], &repo).await;
        let _ = git_cmd(&["config", "user.name", "T"], &repo).await;
        let _ = git_cmd(&["commit", "-q", "--allow-empty", "-m", "init"], &repo).await;
        (tmp, repo)
    }

    #[tokio::test]
    async fn shared_config_write_succeeds_without_contention() {
        let (_tmp, repo) = init_repo().await;
        set_shared_config_with_lock_retry(&repo, "extensions.worktreeConfig", "true")
            .await
            .expect("uncontended write should succeed");
        let out = git_cmd(&["config", "--get", "extensions.worktreeConfig"], &repo)
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "true");
    }

    /// A held `.git/config.lock` makes `git config` fail fast (it does not
    /// block); the retry loop must ride it out and succeed once the lock is
    /// released, rather than surfacing the contention error.
    #[tokio::test]
    async fn shared_config_write_retries_until_lock_released() {
        let (_tmp, repo) = init_repo().await;
        let lock = repo.join(".git/config.lock");
        tokio::fs::write(&lock, b"").await.unwrap();

        // Release shortly after the first attempt fails; the 8-attempt backoff
        // budget (~1s) comfortably covers a 60ms hold.
        let lock_clone = lock.clone();
        let releaser = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            let _ = tokio::fs::remove_file(&lock_clone).await;
        });

        let res =
            set_shared_config_with_lock_retry(&repo, "extensions.worktreeConfig", "true").await;
        let _ = releaser.await;
        let _ = tokio::fs::remove_file(&lock).await;

        assert!(
            res.is_ok(),
            "retry should ride out a transient lock: {:?}",
            res.err()
        );
    }
}
