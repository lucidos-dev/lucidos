//! Shared helpers for locating the bundled `lucidos` CLI binary, installing
//! it as a workspace-relative symlink, and building env vars for spawned
//! scripts so they can call `lucidos data write` / `lucidos events emit` /
//! `lucidos events query` without knowing where the binary lives.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};

#[cfg(windows)]
pub(crate) const LUCIDOS_BIN_NAME: &str = "lucidos.exe";
#[cfg(not(windows))]
pub(crate) const LUCIDOS_BIN_NAME: &str = "lucidos";

static CLI_DIR: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    let exe = std::env::current_exe().ok();
    let exe_parent = exe.as_deref().and_then(Path::parent);
    resolve_cli_dir(std::env::var_os("LUCIDOS_CLI_BIN"), exe_parent)
});

/// The workspace's pinned CLI directory, set once by `pin_cli_for_workspace`.
static PINNED_CLI_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Directory containing the `lucidos` binary that spawned processes run. It is
/// the engine's pinned copy when startup made one, else the resolved
/// directory. Both are fixed for the process lifetime.
/// Returns None when the binary isn't reachable — caller skips CLI wiring.
pub fn lucidos_cli_dir() -> Option<&'static Path> {
    PINNED_CLI_DIR
        .get()
        .map(PathBuf::as_path)
        .or(CLI_DIR.as_deref())
}

/// Where a workspace keeps the CLI its engine pinned.
pub(crate) fn pinned_cli_dir(workspace: &Path) -> PathBuf {
    workspace.join(".lucidos").join("cli")
}

/// Identity of a file on disk: it changes when a rename replaces the path.
pub type FileId = (u64, u64);

#[cfg(unix)]
fn file_id(path: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn file_id(_path: &Path) -> Option<FileId> {
    None
}

/// The identity of the file this engine was launched from. Take it as the
/// first act of `main`, before any wait, so it names the binary that started.
pub fn launch_file_id() -> Option<FileId> {
    std::env::current_exe().ok().and_then(|exe| file_id(&exe))
}

/// Pin the CLI this engine was published with. A later publish into the launch
/// directory then cannot hand its subprocesses a newer CLI than the engine.
/// Call once at startup, after the workspace is known. `launched_as` is
/// `launch_file_id()` as taken at the start of `main`.
///
/// A pin failure never blocks startup: it logs and leaves `lucidos_cli_dir`
/// on the resolved directory. Packaged builds name their CLI through
/// `LUCIDOS_CLI_BIN` and are not pinned, because an update replaces the bundle
/// and restarts the engine together.
pub fn pin_cli_for_workspace(workspace: &Path, launched_as: Option<FileId>) {
    if std::env::var_os("LUCIDOS_CLI_BIN").is_some_and(|bin| Path::new(&bin).is_file()) {
        return;
    }
    let Some(source_dir) = CLI_DIR.as_deref() else {
        return;
    };
    let pinned = match launched_as {
        Some(launched_as) => pin_cli(
            source_dir,
            &pinned_cli_dir(workspace),
            launched_as,
            launch_file_id,
        ),
        None => Err("the engine's launch path could not be probed at startup".into()),
    };
    match pinned {
        Ok(pin_dir) => {
            crate::log!("[LucidosCLI] pinned the CLI into {}", pin_dir.display());
            // A second call keeps the first pin, the one already handed out.
            let _ = PINNED_CLI_DIR.set(pin_dir);
        }
        Err(e) => crate::log!(
            "[LucidosCLI] CLI not pinned, using {}: {}",
            source_dir.display(),
            e
        ),
    }
}

/// Put `source_dir/lucidos` at `pin_dir/lucidos` through a hardlink, or a copy
/// when the two sit on different filesystems. The write goes to a temp name,
/// then a rename, so a CLI process already running from the old pin keeps its
/// inode.
///
/// A build publishes the engine before the CLI. So a CLI newer than this
/// engine always comes with a replaced engine file. `launched_as` is that
/// file's identity at launch, and `launch_id_now` probes it again after the
/// link. A probe that differs or fails means a publish may have preceded the
/// pin. The pin then never lands, and the caller falls back to the published
/// directory.
pub(crate) fn pin_cli(
    source_dir: &Path,
    pin_dir: &Path,
    launched_as: FileId,
    launch_id_now: impl Fn() -> Option<FileId>,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let source = source_dir.join(LUCIDOS_BIN_NAME);
    let pin = pin_dir.join(LUCIDOS_BIN_NAME);
    // One fixed name: a crash between the link and the rename leaves a file
    // the next start overwrites, so none accumulates.
    let temp = pin_dir.join(format!("{LUCIDOS_BIN_NAME}.pin-tmp"));
    std::fs::create_dir_all(pin_dir)?;

    let _ = std::fs::remove_file(&temp);
    if std::fs::hard_link(&source, &temp).is_err() {
        std::fs::copy(&source, &temp)?;
    }
    if launch_id_now() != Some(launched_as) {
        let _ = std::fs::remove_file(&temp);
        return Err("a publish replaced the engine before the CLI was pinned".into());
    }
    std::fs::rename(&temp, &pin)?;
    // Renaming onto a hardlink of the same inode does nothing, and leaves the
    // temp name behind.
    let _ = std::fs::remove_file(&temp);
    Ok(pin_dir.to_path_buf())
}

/// Resolve the directory containing the bundled `lucidos` CLI.
///
/// Packaged builds (the macOS `.app` / headless tarball / install service) stamp
/// `LUCIDOS_CLI_BIN` with the absolute path of the staged binary — checked FIRST
/// because the packaged engine lives at `<resources>/lucidos-engine` with the
/// CLI as a flat sibling `<resources>/lucidos`, and the launchd PATH never
/// carries it. Dev / `cargo` / test builds have no such env and fall back to the
/// sibling-walk up from the engine's exe dir. `env_bin` is `LUCIDOS_CLI_BIN`,
/// `exe_parent` is `current_exe().parent()`; both are injected so the resolution
/// is unit-testable without touching process env / `current_exe`.
pub(crate) fn resolve_cli_dir(
    env_bin: Option<std::ffi::OsString>,
    exe_parent: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(bin) = env_bin {
        let path = PathBuf::from(bin);
        if path.is_file() {
            return path.parent().map(Path::to_path_buf);
        }
        // Set but the file is gone (mis-staged / quarantined): fall through to
        // the sibling-walk rather than returning a dir without `lucidos`.
    }
    exe_parent.and_then(find_lucidos_cli_dir)
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

/// Build a `PATH` value with `extra_dirs` prepended (in order) to the engine's
/// inherited PATH. Uses `join_paths` so the OS-correct separator is used and
/// entries containing the separator get quoted.
pub(crate) fn path_with_prefixes(
    extra_dirs: &[PathBuf],
) -> Result<std::ffi::OsString, std::env::JoinPathsError> {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(
        extra_dirs
            .iter()
            .cloned()
            .chain(std::env::split_paths(&existing)),
    )
}

/// Env vars to inject into every Lucidos-spawned script (Python, bash,
/// scheduled tasks). Always sets `LUCIDOS_WORKSPACE`; prepends to `PATH`
/// `<workspace>/.lucidos/bin` (when the bundled CLI is reachable, so scripts
/// resolve bare `lucidos …`) AND `pg_bin_dir` (when given + present, so
/// `psql "$LUCIDOS_DATABASE_URL"` resolves — in a packaged build psql lives at
/// `<resources>/postgres/bin` = `LUCIDOS_PG_BIN_DIR`, which is not on the
/// launchd minimal PATH). `pg_bin_dir` is passed in (from the caller's
/// `LUCIDOS_PG_BIN_DIR` read) so this stays pure/testable.
///
/// Side effect: ensures the workspace bin symlink exists so the prepended
/// PATH actually resolves `lucidos`.
pub(crate) fn workspace_script_env_vars(
    workspace: &Path,
    cli_dir: Option<&Path>,
    pg_bin_dir: Option<&Path>,
) -> Vec<(String, String)> {
    let mut vars = vec![(
        "LUCIDOS_WORKSPACE".to_string(),
        workspace.display().to_string(),
    )];
    let mut prefixes: Vec<PathBuf> = Vec::new();
    if let Some(bin_dir) = ensure_workspace_bin_symlink(workspace, cli_dir) {
        prefixes.push(bin_dir);
    }
    if let Some(pg_bin) = pg_bin_dir {
        if pg_bin.is_dir() {
            prefixes.push(pg_bin.to_path_buf());
        }
    }
    if !prefixes.is_empty() {
        match path_with_prefixes(&prefixes) {
            Ok(p) => vars.push(("PATH".to_string(), p.to_string_lossy().into_owned())),
            Err(e) => crate::log!("[LucidosCLI] failed to join PATH: {}", e),
        }
    }
    vars
}

/// Skill content embedded at compile time, so CC discovers the CLI workflow in
/// app and external repos. It is the Lucidos repo's own tracked copy, as of the
/// commit the engine was built from.
pub(crate) const LUCIDOS_CLI_SKILL: &str =
    include_str!("../../../../.claude/skills/lucidos-cli/SKILL.md");

/// Where the skill lives, relative to CC's cwd: the app folder for app
/// coding-agent threads, the worktree root otherwise.
pub(crate) const LUCIDOS_CLI_SKILL_REL_PATH: &str = ".claude/skills/lucidos-cli/SKILL.md";

/// Give a CC session the lucidos-cli skill. Who owns the file decides how.
///
/// - **Lucidos source repo:** the branch's tracked copy owns it, and `main`
///   edits it. The engine never writes or hides it. An engine built from
///   another commit would otherwise block every merge of `main` that changes it.
/// - **App and external repos:** the engine owns it and writes `engine_copy`,
///   then hides a tracked stale copy (`hide_phantom_tracked_skill`).
pub(crate) async fn place_lucidos_cli_skill(
    cwd: &Path,
    kind: crate::engine::agent_session::CodingAgentKind,
    engine_copy: &str,
) {
    use crate::engine::agent_session::CodingAgentKind;
    use crate::engine::git_ops::hide_phantom_tracked_skill;

    match kind {
        CodingAgentKind::Lucidos => {}
        CodingAgentKind::App | CodingAgentKind::External => {
            if let Err(e) = write_lucidos_cli_skill(cwd, engine_copy) {
                crate::log!(
                    "[LucidosCLI] failed to install lucidos-cli skill into {}: {}",
                    cwd.display(),
                    e
                );
            }
            hide_phantom_tracked_skill(cwd, LUCIDOS_CLI_SKILL_REL_PATH).await;
        }
    }
}

/// Idempotent: skips rewriting if the on-disk content already matches, which
/// avoids mtime churn that would invalidate CC's skill cache on every spawn.
fn write_lucidos_cli_skill(worktree: &Path, content: &str) -> std::io::Result<()> {
    let skill_file = worktree.join(LUCIDOS_CLI_SKILL_REL_PATH);
    let skill_dir = skill_file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| worktree.to_path_buf());
    if let Ok(existing) = std::fs::read_to_string(&skill_file) {
        if existing == content {
            return Ok(());
        }
    }
    std::fs::create_dir_all(&skill_dir)?;
    std::fs::write(skill_file, content)
}

#[cfg(test)]
#[path = "lucidos_cli_tests.rs"]
mod tests;
