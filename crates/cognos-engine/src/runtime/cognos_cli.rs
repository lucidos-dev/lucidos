//! Shared helpers for locating the bundled `cognos` CLI binary, installing
//! it as a workspace-relative symlink, and building env vars for spawned
//! scripts so they can call `cognos data write` / `cognos events emit` /
//! `cognos events query` without knowing where the binary lives.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[cfg(windows)]
pub(crate) const COGNOS_BIN_NAME: &str = "cognos.exe";
#[cfg(not(windows))]
pub(crate) const COGNOS_BIN_NAME: &str = "cognos";

static CLI_DIR: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    let exe = std::env::current_exe().ok()?;
    find_cognos_cli_dir(exe.parent()?)
});

/// Directory containing the bundled `cognos` binary. The engine's exe path
/// is immutable for the process lifetime, so the lookup is memoized.
/// Returns None when the binary isn't reachable — caller skips CLI wiring.
pub fn cognos_cli_dir() -> Option<&'static Path> {
    CLI_DIR.as_deref()
}

/// Walk up from `start` looking for a directory containing the `cognos`
/// binary. Returns the first such directory, or None.
///
/// Why walk up: production engine binary lives at `target/release/cognos-engine`
/// (sibling to `cognos`), but `cargo test` binaries live at
/// `target/debug/deps/<test-XXX>` — one level deeper than `cognos`. Both must
/// resolve correctly.
pub(crate) fn find_cognos_cli_dir(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    for _ in 0..3 {
        let d = dir?;
        if d.join(COGNOS_BIN_NAME).is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// The workspace-relative directory where the `cognos` symlink is installed.
/// Stable path scripts and docs can reference regardless of how the engine
/// was packaged (cargo build, system install, Tauri bundle).
pub(crate) fn workspace_bin_dir(workspace: &Path) -> PathBuf {
    workspace.join(".cognos").join("bin")
}

/// Ensure `<workspace>/.cognos/bin/cognos` is a symlink to the bundled
/// `cognos` binary in `cli_dir`. Replaces stale symlinks. Returns the bin dir
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
    let target = cli_dir.join(COGNOS_BIN_NAME);
    // Guard against creating a dangling symlink if the engine binary moved.
    if !target.is_file() {
        return None;
    }
    let bin_dir = workspace_bin_dir(workspace);
    if let Err(e) = std::fs::create_dir_all(&bin_dir) {
        crate::log!("[CognosCLI] failed to create {}: {}", bin_dir.display(), e);
        return None;
    }
    let link = bin_dir.join(COGNOS_BIN_NAME);
    if let Ok(existing) = std::fs::read_link(&link) {
        if existing == target {
            return Some(bin_dir);
        }
        let _ = std::fs::remove_file(&link);
    } else if link.exists() {
        // Pre-existing real file (user/script copied the binary in). Don't
        // clobber — trust it's a working `cognos` and let PATH pick it up.
        return Some(bin_dir);
    }
    if let Err(e) = std::os::unix::fs::symlink(&target, &link) {
        crate::log!(
            "[CognosCLI] failed to symlink {} -> {}: {}",
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

/// Env vars to inject into every CognOS-spawned script (Python, bash,
/// scheduled tasks). Always sets `COGNOS_WORKSPACE`; sets `PATH` (with
/// `<workspace>/.cognos/bin` prepended) when the bundled CLI is reachable.
///
/// Side effect: ensures the workspace bin symlink exists so the prepended
/// PATH actually resolves `cognos`.
pub(crate) fn workspace_script_env_vars(
    workspace: &Path,
    cli_dir: Option<&Path>,
) -> Vec<(String, String)> {
    let mut vars = vec![(
        "COGNOS_WORKSPACE".to_string(),
        workspace.display().to_string(),
    )];
    if let Some(bin_dir) = ensure_workspace_bin_symlink(workspace, cli_dir) {
        match path_with_prefix(&bin_dir) {
            Ok(p) => vars.push(("PATH".to_string(), p.to_string_lossy().into_owned())),
            Err(e) => crate::log!("[CognosCLI] failed to join PATH: {}", e),
        }
    }
    vars
}

/// Skill content embedded at compile time — written into each CC worktree's
/// `.claude/skills/cognos-cli/SKILL.md` so CC discovers the CLI workflow.
pub(crate) const COGNOS_CLI_SKILL: &str = include_str!("../../../cognos-cli/skill.md");

/// Install the cognos-cli skill into a CC worktree. Skipped when the binary
/// isn't reachable — teaching CC about a tool it can't run wastes context.
/// Idempotent: skips rewriting if the on-disk content already matches, which
/// avoids mtime churn that would invalidate CC's skill cache on every spawn.
pub(crate) fn install_cognos_cli_skill(
    worktree: &Path,
    cli_dir: Option<&Path>,
) -> std::io::Result<()> {
    if cli_dir.is_none() {
        return Ok(());
    }
    let skill_dir = worktree.join(".claude/skills/cognos-cli");
    let skill_file = skill_dir.join("SKILL.md");
    if let Ok(existing) = std::fs::read_to_string(&skill_file) {
        if existing == COGNOS_CLI_SKILL {
            return Ok(());
        }
    }
    std::fs::create_dir_all(&skill_dir)?;
    std::fs::write(skill_file, COGNOS_CLI_SKILL)
}

#[cfg(test)]
#[path = "cognos_cli_tests.rs"]
mod tests;
