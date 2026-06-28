//! Shared helpers for locating the bundled `lucidos` CLI binary, installing
//! it as a workspace-relative symlink, and building env vars for spawned
//! scripts so they can call `lucidos data write` / `lucidos events emit` /
//! `lucidos events query` without knowing where the binary lives.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[cfg(windows)]
pub(crate) const LUCIDOS_BIN_NAME: &str = "lucidos.exe";
#[cfg(not(windows))]
pub(crate) const LUCIDOS_BIN_NAME: &str = "lucidos";

static CLI_DIR: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    let exe = std::env::current_exe().ok()?;
    find_lucidos_cli_dir(exe.parent()?)
});

/// Directory containing the bundled `lucidos` binary. The engine's exe path
/// is immutable for the process lifetime, so the lookup is memoized.
/// Returns None when the binary isn't reachable — caller skips CLI wiring.
pub fn lucidos_cli_dir() -> Option<&'static Path> {
    CLI_DIR.as_deref()
}

/// Walk up from `start` looking for a directory containing the `lucidos`
/// binary. Returns the first such directory, or None.
///
/// Why walk up: production engine binary lives at `target/release/lucidos-engine`
/// (sibling to `lucidos`), but `cargo test` binaries live at
/// `target/debug/deps/<test-hash>` — one level deeper than `lucidos`. Both must
/// resolve correctly.
pub(crate) fn find_lucidos_cli_dir(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    for _ in 0..3 {
        let d = dir?;
        if d.join(LUCIDOS_BIN_NAME).is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// The workspace-relative directory where the `lucidos` symlink is installed.
/// Stable path scripts and docs can reference regardless of how the engine
/// was packaged (cargo build, system install, Tauri bundle).
pub(crate) fn workspace_bin_dir(workspace: &Path) -> PathBuf {
    workspace.join(".lucidos").join("bin")
}

/// Ensure `<workspace>/.lucidos/bin/lucidos` is a symlink to the bundled
/// `lucidos` binary in `cli_dir`. Replaces stale symlinks. Returns the bin dir
/// on success so callers can prepend it to `PATH`. Returns None when
/// `cli_dir` is None, when the binary isn't actually there, when symlink
/// creation fails, or on Windows (symlinks need elevated perms there).
#[cfg(not(unix))]
pub(crate) fn ensure_workspace_bin_symlink(
    _workspace: &Path,
    _cli_dir: Option<&Path>,
) -> Option<PathBuf> {
    None
}

#[cfg(unix)]
pub(crate) fn ensure_workspace_bin_symlink(
    workspace: &Path,
    cli_dir: Option<&Path>,
) -> Option<PathBuf> {
    let cli_dir = cli_dir?;
    let target = cli_dir.join(LUCIDOS_BIN_NAME);
    // Guard against creating a dangling symlink if the engine binary moved.
    if !target.is_file() {
        return None;
    }
    let bin_dir = workspace_bin_dir(workspace);
    if let Err(e) = std::fs::create_dir_all(&bin_dir) {
        crate::log!("[LucidosCLI] failed to create {}: {}", bin_dir.display(), e);
        return None;
    }
    let link = bin_dir.join(LUCIDOS_BIN_NAME);
    if let Ok(existing) = std::fs::read_link(&link) {
        if existing == target {
            return Some(bin_dir);
        }
        let _ = std::fs::remove_file(&link);
    } else if link.exists() {
        // Pre-existing real file (user/script copied the binary in). Don't
        // clobber — trust it's a working `lucidos` and let PATH pick it up.
        return Some(bin_dir);
    }
    if let Err(e) = std::os::unix::fs::symlink(&target, &link) {
        crate::log!(
            "[LucidosCLI] failed to symlink {} -> {}: {}",
            link.display(),
            target.display(),
            e
        );
        return None;
    }
    Some(bin_dir)
}

/// Build a `PATH` value with `extra_dir` prepended to the engine's inherited
/// PATH. Uses `join_paths` so the OS-correct separator is used and entries
/// containing the separator get quoted.
pub(crate) fn path_with_prefix(
    extra_dir: &Path,
) -> Result<std::ffi::OsString, std::env::JoinPathsError> {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(
        std::iter::once(extra_dir.to_path_buf()).chain(std::env::split_paths(&existing)),
    )
}

/// Env vars to inject into every Lucidos-spawned script (Python, bash,
/// scheduled tasks). Always sets `LUCIDOS_WORKSPACE`; sets `PATH` (with
/// `<workspace>/.lucidos/bin` prepended) when the bundled CLI is reachable.
///
/// Side effect: ensures the workspace bin symlink exists so the prepended
/// PATH actually resolves `lucidos`.
pub(crate) fn workspace_script_env_vars(
    workspace: &Path,
    cli_dir: Option<&Path>,
) -> Vec<(String, String)> {
    let mut vars = vec![(
        "LUCIDOS_WORKSPACE".to_string(),
        workspace.display().to_string(),
    )];
    if let Some(bin_dir) = ensure_workspace_bin_symlink(workspace, cli_dir) {
        match path_with_prefix(&bin_dir) {
            Ok(p) => vars.push(("PATH".to_string(), p.to_string_lossy().into_owned())),
            Err(e) => crate::log!("[LucidosCLI] failed to join PATH: {}", e),
        }
    }
    vars
}

/// Skill content embedded at compile time — written into each CC worktree's
/// `.claude/skills/lucidos-cli/SKILL.md` so CC discovers the CLI workflow.
/// The embedded source is the very file we write to: in the Lucidos repo
/// itself, `.claude/skills/lucidos-cli/SKILL.md` is tracked, and pointing
/// `include_str!` at it makes the engine's compiled-in copy byte-identical to
/// what's committed — so `install_lucidos_cli_skill` is a no-op there and a
/// fresh CC worktree never starts with a phantom `M` on this path.
pub(crate) const LUCIDOS_CLI_SKILL: &str =
    include_str!("../../../../.claude/skills/lucidos-cli/SKILL.md");

/// Worktree-relative path the skill is written to. `install_lucidos_cli_skill`
/// writes here relative to its `worktree` arg (CC's cwd: the app folder for app
/// coding-agent threads, the worktree root otherwise), and the spawn path feeds
/// the same constant to `hide_phantom_tracked_skill` so the install site and the
/// phantom-change guard can never drift apart.
pub(crate) const LUCIDOS_CLI_SKILL_REL_PATH: &str = ".claude/skills/lucidos-cli/SKILL.md";

/// Install the lucidos-cli skill into a CC worktree. Skipped when the binary
/// isn't reachable — teaching CC about a tool it can't run wastes context.
/// Idempotent: skips rewriting if the on-disk content already matches, which
/// avoids mtime churn that would invalidate CC's skill cache on every spawn.
pub(crate) fn install_lucidos_cli_skill(
    worktree: &Path,
    cli_dir: Option<&Path>,
) -> std::io::Result<()> {
    if cli_dir.is_none() {
        return Ok(());
    }
    let skill_file = worktree.join(LUCIDOS_CLI_SKILL_REL_PATH);
    let skill_dir = skill_file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| worktree.to_path_buf());
    if let Ok(existing) = std::fs::read_to_string(&skill_file) {
        if existing == LUCIDOS_CLI_SKILL {
            return Ok(());
        }
    }
    std::fs::create_dir_all(&skill_dir)?;
    std::fs::write(skill_file, LUCIDOS_CLI_SKILL)
}

#[cfg(test)]
#[path = "lucidos_cli_tests.rs"]
mod tests;
