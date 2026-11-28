use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};

pub(crate) use crate::core::agent_sandbox::LUCIDOS_BIN_NAME;

static CLI_DIR: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    let exe = std::env::current_exe().ok();
    let exe_parent = exe.as_deref().and_then(Path::parent);
    resolve_cli_dir(std::env::var_os("LUCIDOS_CLI_BIN"), exe_parent)
});

static PINNED_CLI_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn lucidos_cli_dir() -> Option<&'static Path> {
    PINNED_CLI_DIR
        .get()
        .map(PathBuf::as_path)
        .or(CLI_DIR.as_deref())
}

pub(crate) fn pinned_cli_dir(workspace: &Path) -> PathBuf {
    workspace.join(".lucidos").join("cli")
}

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

/// INVARIANT: The identity of the file this engine was launched from. Take it as the
/// first act of `main`, before any wait, so it names the binary that started.
pub fn launch_file_id() -> Option<FileId> {
    std::env::current_exe().ok().and_then(|exe| file_id(&exe))
}

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
            crate::log_legacy!("[LucidosCLI] pinned the CLI into {}", pin_dir.display());
            let _ = PINNED_CLI_DIR.set(pin_dir);
        }
        Err(e) => crate::log_legacy!(
            "[LucidosCLI] CLI not pinned, using {}: {}",
            source_dir.display(),
            e
        ),
    }
}

/// INVARIANT: Put `source_dir/lucidos` at `pin_dir/lucidos` through a hardlink, or a copy
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
    let _ = std::fs::remove_file(&temp);
    Ok(pin_dir.to_path_buf())
}

pub(crate) fn resolve_cli_dir(
    env_bin: Option<std::ffi::OsString>,
    exe_parent: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(bin) = env_bin {
        let path = PathBuf::from(bin);
        if path.is_file() {
            return path.parent().map(Path::to_path_buf);
        }
    }
    exe_parent.and_then(find_lucidos_cli_dir)
}

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

pub(crate) fn workspace_bin_dir(workspace: &Path) -> PathBuf {
    workspace.join(".lucidos").join("bin")
}

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
    if !target.is_file() {
        return None;
    }
    let bin_dir = workspace_bin_dir(workspace);
    if let Err(e) = std::fs::create_dir_all(&bin_dir) {
        crate::log_legacy!("[LucidosCLI] failed to create {}: {}", bin_dir.display(), e);
        return None;
    }
    let link = bin_dir.join(LUCIDOS_BIN_NAME);
    if let Ok(existing) = std::fs::read_link(&link) {
        if existing == target {
            return Some(bin_dir);
        }
        let _ = std::fs::remove_file(&link);
    } else if link.exists() {
        return Some(bin_dir);
    }
    if let Err(e) = std::os::unix::fs::symlink(&target, &link) {
        crate::log_legacy!(
            "[LucidosCLI] failed to symlink {} -> {}: {}",
            link.display(),
            target.display(),
            e
        );
        return None;
    }
    Some(bin_dir)
}

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

pub(crate) fn workspace_script_env_vars(
    workspace: &Path,
    cli_dir: Option<&Path>,
    ps_shim_dir: Option<&Path>,
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
    prefixes.extend(ps_shim_dir.map(Path::to_path_buf));
    if let Some(pg_bin) = pg_bin_dir {
        if pg_bin.is_dir() {
            prefixes.push(pg_bin.to_path_buf());
        }
    }
    if !prefixes.is_empty() {
        match path_with_prefixes(&prefixes) {
            Ok(p) => vars.push(("PATH".to_string(), p.to_string_lossy().into_owned())),
            Err(e) => crate::log_legacy!("[LucidosCLI] failed to join PATH: {}", e),
        }
    }
    vars
}

pub(crate) const LUCIDOS_CLI_SKILL: &str =
    include_str!("../../../../.claude/skills/lucidos-cli/SKILL.md");

pub(crate) const LUCIDOS_CLI_SKILL_REL_PATH: &str = ".claude/skills/lucidos-cli/SKILL.md";

/// INVARIANT: Give a CC session the lucidos-cli skill. Who owns the file decides how.
///
/// - **Lucidos source repo:** the branch's tracked copy owns it, and `main`
///   edits it. The engine never writes or hides it. An engine built from
///   another commit would otherwise block every merge of `main` that changes it.
/// - **App and external repos:** the engine owns it and writes `engine_copy`,
///   then hides a tracked stale copy (`hide_phantom_tracked_skill`).
pub(crate) async fn place_lucidos_cli_skill(
    cwd: &Path,
    kind: crate::engine::thread_events::CodingAgentKind,
    engine_copy: &str,
) {
    use crate::engine::git_ops::hide_phantom_tracked_skill;
    use crate::engine::thread_events::CodingAgentKind;

    match kind {
        CodingAgentKind::Lucidos => {}
        CodingAgentKind::App | CodingAgentKind::External => {
            if let Err(e) = write_lucidos_cli_skill(cwd, engine_copy) {
                crate::log_legacy!(
                    "[LucidosCLI] failed to install lucidos-cli skill into {}: {}",
                    cwd.display(),
                    e
                );
            }
            hide_phantom_tracked_skill(cwd, LUCIDOS_CLI_SKILL_REL_PATH).await;
        }
    }
}

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
