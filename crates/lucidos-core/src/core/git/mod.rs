//! Git command primitives shared by the store and repository orchestration.

mod conflict_probe;
pub use conflict_probe::{predict_merge_into_main, probe_merge, MergeProbe};

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

/// Canonical absolute repo root, as the DB key of a per-branch marker.
///
/// Resolves symlinks, so the git hook and the engine produce the same row. The
/// hook derives its path from `git rev-parse --git-common-dir`, which can
/// answer relatively or through a symlink. An unresolved path would key a
/// second row nothing reads.
///
/// One definition for all three readers of those tables: the harden marker, the
/// plan marker, and the sweep a thread delete runs over both. It lived twice,
/// once per marker module, and a third copy in `api::threads::delete` would
/// have keyed its DELETEs on a path the writers never wrote.
pub fn canonical_repo_root(repo_root: &Path) -> String {
    repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf())
        .to_string_lossy()
        .to_string()
}

/// How long a single git invocation may run before the engine gives up on it.
/// Generous, because it is a ceiling on a saturated host, not a latency target:
/// while the e2e suite runs, ordinary `rev-parse` calls have been observed
/// taking tens of seconds.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(30);

/// The git binary the engine launches, resolved once per process.
///
/// On macOS `/usr/bin/git` is the `xcrun` shim, which launches the real git as
/// a second process. Every launch costs tens of milliseconds there, so the shim
/// doubles the price of each call. The shim's git names its exec path, whose
/// `git` is the binary behind it.
///
/// Lazy, so it resolves after `core::user_path::augment_process_path` has set
/// the PATH it reads.
pub(crate) async fn git_program() -> &'static Path {
    static PROGRAM: tokio::sync::OnceCell<PathBuf> = tokio::sync::OnceCell::const_new();
    PROGRAM
        .get_or_init(|| async {
            let path_git = std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join("git"))
                    .find(|git| git.is_file())
            });
            if !is_xcrun_git_shim(path_git.as_deref(), cfg!(target_os = "macos")) {
                return PathBuf::from("git");
            }
            let probe = tokio::process::Command::new("git")
                .arg("--exec-path")
                .output();
            let exec_path = match tokio::time::timeout(GIT_TIMEOUT, probe).await {
                Ok(Ok(o)) if o.status.success() => Some(o.stdout),
                _ => None,
            };
            git_program_from_exec_path(exec_path.as_deref())
        })
        .await
}

/// Only the `xcrun` shim is bypassed. Another `git` on PATH may be a wrapper
/// that injects config, credentials or auditing. Launching the binary behind
/// it would skip all of that.
fn is_xcrun_git_shim(path_git: Option<&Path>, macos: bool) -> bool {
    macos && path_git == Some(Path::new("/usr/bin/git"))
}

/// `<exec-path>/git` when `git --exec-path` answered with a directory holding
/// one, else bare `git`. A failed probe costs the shortcut, never a git call.
fn git_program_from_exec_path(stdout: Option<&[u8]>) -> PathBuf {
    let dir = stdout.map(|s| String::from_utf8_lossy(s).trim().to_string());
    match dir.filter(|d| !d.is_empty()) {
        Some(dir) if Path::new(&dir).join("git").is_file() => Path::new(&dir).join("git"),
        _ => PathBuf::from("git"),
    }
}

/// Run a git command with the [`GIT_TIMEOUT`] ceiling. Always prepends
/// `-c core.quotepath=false` so non-ASCII paths come back as raw UTF-8 instead
/// of git's default `"...\NNN..."` form — every caller treats output as a path.
pub async fn git_cmd(args: &[&str], dir: &Path) -> Result<std::process::Output, String> {
    git_cmd_env(args, dir, &[]).await
}

/// Run a git command that is expected to DO something, folding a non-zero exit
/// into the same `Err` a spawn failure or the [`GIT_TIMEOUT`] produces.
///
/// [`git_cmd`] returning `Ok` says only that git ran, not that it did the
/// thing. A caller who just needs "did it work, and if not why" otherwise
/// writes that three-arm match out at every site. Use this for a mutating
/// call. For a yes/no *question* reach for `git_answer` instead, which keeps
/// "no" and "could not ask" apart.
pub async fn git_ran_ok(args: &[&str], dir: &Path) -> Result<(), String> {
    match git_cmd(args, dir).await {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Err(e),
    }
}

/// The repo's shared `.git` directory, as an absolute path. Asked from a linked
/// worktree, it names the main checkout's `.git`. A failed spawn, the timeout,
/// a non-zero exit and an empty answer are all `Err`, never a guessed path.
pub async fn git_common_dir(dir: &Path) -> Result<PathBuf, String> {
    let out = git_cmd(&["rev-parse", "--git-common-dir"], dir).await?;
    if !out.status.success() {
        return Err(format!(
            "git rev-parse --git-common-dir failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return Err("git rev-parse --git-common-dir returned nothing".to_string());
    }
    // Git may answer relative to `dir`. Joining an absolute answer returns it.
    Ok(dir.join(raw))
}

/// The repo's main working tree, asked from any worktree of it. Git lists it
/// first in `git worktree list`, which holds for a submodule too. A bare repo
/// has none. Nor does a `--separate-git-dir` clone asked from a linked
/// worktree: git records no path for it and names the git dir instead. Both
/// are `Err`, like every failure to ask.
pub async fn git_main_worktree(dir: &Path) -> Result<PathBuf, String> {
    let out = git_cmd(&["worktree", "list", "--porcelain"], dir).await?;
    if !out.status.success() {
        return Err(format!(
            "git worktree list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut first = stdout.lines().take_while(|line| !line.is_empty());
    let path = first
        .next()
        .and_then(|line| line.strip_prefix("worktree "))
        .ok_or("git worktree list named no worktree")?;
    if first.any(|line| line == "bare") {
        return Err("the repo is bare, so it has no main working tree".to_string());
    }
    let main = PathBuf::from(path);
    let same = |a: &Path, b: &Path| std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok();
    if same(&main, &git_common_dir(dir).await?) {
        return Err("git records no main working tree for this repo".to_string());
    }
    Ok(main)
}

/// The author and committer env for a commit Lucidos itself makes, so it
/// works when the repo has no user.name/email in its config. The engine
/// commits artifacts through git2 with a per-commit signature, not config.
pub fn lucidos_identity_env() -> Vec<(&'static str, &'static OsStr)> {
    vec![
        ("GIT_AUTHOR_NAME", OsStr::new("Lucidos")),
        ("GIT_AUTHOR_EMAIL", OsStr::new("lucidos@localhost")),
        ("GIT_COMMITTER_NAME", OsStr::new("Lucidos")),
        ("GIT_COMMITTER_EMAIL", OsStr::new("lucidos@localhost")),
    ]
}

/// Like [`git_cmd`] but with extra environment variables. Used by the command
/// checkpoint helpers, which set `GIT_INDEX_FILE` to a throwaway index so a
/// snapshot/restore never disturbs the repo's real index or working tree.
pub async fn git_cmd_env(
    args: &[&str],
    dir: &Path,
    envs: &[(&str, &OsStr)],
) -> Result<std::process::Output, String> {
    git_cmd_env_timeout(args, dir, envs, GIT_TIMEOUT).await
}

/// [`git_cmd_env`] with an explicit ceiling. Tests use it to force the expiry
/// branch. Production callers get [`GIT_TIMEOUT`], unless the command moves a
/// whole repository's history, like a restore fetching `origin`.
pub async fn git_cmd_env_timeout(
    args: &[&str],
    dir: &Path,
    envs: &[(&str, &OsStr)],
    timeout: Duration,
) -> Result<std::process::Output, String> {
    git_cmd_spawn(args, dir, envs, timeout, false).await
}

/// [`git_cmd`] for a command the engine may SIGKILL the moment it stops
/// waiting. Opt-in, because leaving a timed-out process alone is the right
/// default. Killing a half-written `commit` or `merge` strands a lock file over
/// a command that would have finished.
///
/// The opposite case is the pair `WORKTREE_ADMIN_MUTEX` guards.
/// Both write `$GIT_DIR/worktrees/<id>`, and a timeout releases that guard
/// while the process writes on. Whichever spawn holds the guard next then
/// races the one that gave up. Neither destroys anything pre-existing when
/// killed: a prune is idempotent, and an add owns only what this attempt is
/// creating.
pub async fn git_cmd_kill_on_timeout(
    args: &[&str],
    dir: &Path,
) -> Result<std::process::Output, String> {
    git_cmd_spawn(args, dir, &[], GIT_TIMEOUT, true).await
}

/// Run `program`, falling back to PATH's `git` when `program` is gone.
///
/// The resolved binary can vanish under a running engine: a Homebrew upgrade
/// deletes the versioned keg its exec path named, while PATH's `git` follows
/// the upgrade.
async fn run_git(
    program: &Path,
    args: &[&str],
    dir: &Path,
    envs: &[(&str, &OsStr)],
    kill_on_drop: bool,
) -> std::io::Result<std::process::Output> {
    let output = |program: &Path| {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args).current_dir(dir).kill_on_drop(kill_on_drop);
        // `git status` refreshes a stale index under `index.lock`. That lock
        // makes a concurrent `git add` or `git commit` in the same worktree
        // die. Mandatory locks are unaffected. `git diff` ignores this
        // setting, so a probe uses the plumbing `diff-files` instead.
        cmd.env("GIT_OPTIONAL_LOCKS", "0");
        for (key, value) in envs {
            cmd.env(key, value);
        }
        async move { cmd.output().await }
    };
    match output(program).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && program != Path::new("git") => {
            output(Path::new("git")).await
        }
        result => result,
    }
}

/// The one place a git command is spawned, through [`run_git`]. The only other
/// launch is [`git_program`]'s one-time probe, which finds the binary this runs.
/// `kill_on_timeout` decides whether giving up on the wait also kills the process.
async fn git_cmd_spawn(
    args: &[&str],
    dir: &Path,
    envs: &[(&str, &OsStr)],
    timeout: Duration,
    kill_on_timeout: bool,
) -> Result<std::process::Output, String> {
    let mut full_args: Vec<&str> = Vec::with_capacity(args.len() + 2);
    full_args.push("-c");
    full_args.push("core.quotepath=false");
    full_args.extend_from_slice(args);
    let run = run_git(git_program().await, &full_args, dir, envs, kill_on_timeout);
    match tokio::time::timeout(timeout, run).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(format!("git {} failed: {}", args.join(" "), e)),
        Err(_) => Err(format!(
            "git {} timed out after {}s",
            args.join(" "),
            timeout.as_secs()
        )),
    }
}

#[cfg(test)]
mod program_tests;

/// Whether `path` (repo-relative) is a plan file: what the `implementation-plan`
/// skill writes, and what the `cc-plan-gate` hook never blocks.
pub fn is_plan_file(path: &str) -> bool {
    path.starts_with("docs/plans/")
}

/// Whether every file a branch would land is a plan file. An empty list is no
/// evidence of a plan-only branch.
pub fn lands_plan_files_only(landing: &[String]) -> bool {
    !landing.is_empty() && landing.iter().all(|f| is_plan_file(f))
}

/// Whether Lucidos-source work must run `/harden` before it may become a
/// change or merge. A plan has no tests, so hardening runs on the branch that
/// implements it.
pub fn needs_hardening(hardened: bool, landing: &[String]) -> bool {
    !hardened && !lands_plan_files_only(landing)
}

/// Prefix of a lane batch's *integration branch* (ADR 0463).
pub const LANE_BRANCH_PREFIX: &str = "lane/";

/// The integration branch a lane batch's lane thread resolves on.
pub fn lane_branch(batch_id: Uuid) -> String {
    format!("{LANE_BRANCH_PREFIX}{batch_id}")
}

/// The batch a lane branch belongs to. `None` for any other branch.
pub fn lane_branch_batch(branch: &str) -> Option<Uuid> {
    branch
        .strip_prefix(LANE_BRANCH_PREFIX)
        .and_then(|id| Uuid::parse_str(id).ok())
}

/// Is this a lane batch's integration branch? Orphan recovery scans these
/// beside the coding-agent branches, since a lane thread works on one.
pub fn is_lane_branch(branch: &str) -> bool {
    lane_branch_batch(branch).is_some()
}

pub const LANE_BRANCH_IS_NOT_A_CHANGE: &str =
    "A lane batch's integration branch lands through the landing lane, never as a change.";

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

/// Directories whose files feed the served client bundle. Mirrors `watchDirs`
/// in `crates/lucidos-app/dev-build-watch.mjs`.
pub const CLIENT_BUNDLE_DIRS: &[&str] = &[
    "crates/lucidos-app/src/",
    "crates/lucidos-app/public/",
    "crates/lucidos-app/vite/",
    "packages/lucidos-sdk/src/",
];

/// Single files that feed the served client bundle. Mirrors `watchFiles` in
/// `crates/lucidos-app/dev-build-watch.mjs`.
pub const CLIENT_BUNDLE_FILES: &[&str] = &[
    "crates/lucidos-app/index.html",
    "crates/lucidos-app/vite.config.ts",
    "crates/lucidos-app/package.json",
    "package.json",
    "package-lock.json",
];

/// True if any of `files` is an input the build-watch rebuilds the served
/// client from. After a frontend-only Apply the engine waits for that rebuild,
/// so a file the watch ignores (an e2e spec, a script) must not count: no
/// build would run, and the wait would end in a false "not served yet" warning.
pub fn files_have_client_update(files: &[String]) -> bool {
    files.iter().any(|f| {
        CLIENT_BUNDLE_FILES.contains(&f.as_str())
            || CLIENT_BUNDLE_DIRS.iter().any(|dir| f.starts_with(dir))
    })
}
