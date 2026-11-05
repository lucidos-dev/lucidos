//! The Lucidos home and the storage-root rule.
//!
//! A *storage root* is a directory Lucidos stores data in and keeps at mode
//! 0700. The kernel then refuses another local user traversal, so nothing
//! inside is reachable whatever its own mode. Files inside keep the umask's
//! modes on purpose: a process umask would leak into every child, and an agent
//! would then write 0600 files into the user's own projects.
//!
//! The storage roots are the Lucidos home (`~/.lucidos`), the gateway data
//! dir, every workspace root and the user cache root. Whoever opens one calls
//! [`ensure_storage_root`] first, so an install created before the rule
//! converges on its next start.
//!
//! Design: `docs/plans/2026-10-12-other-local-users-cannot-read-lucidos-storage.md`.

use std::ffi::OsString;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The Lucidos home's name under `$HOME`.
pub const HOME_DIR_NAME: &str = ".lucidos";

/// The mode of every storage root: owner read, write and traverse, nobody else.
pub const STORAGE_ROOT_MODE: u32 = 0o700;

/// The variable that relocates the gateway data dir.
pub const GATEWAY_DATA_ENV: &str = "LUCIDOS_GATEWAY_DATA";

/// The gateway data dir's name under the Lucidos home, when nothing relocates it.
pub const GATEWAY_DIR_NAME: &str = "gateway";

/// `$HOME/.lucidos`, without touching the disk. `None` when `HOME` is unset or
/// empty, since an empty `HOME` would put the home in the working directory.
pub fn path() -> Option<PathBuf> {
    path_from(std::env::var_os("HOME"))
}

/// The Lucidos home under an explicit home directory.
pub fn path_in(home: &Path) -> PathBuf {
    home.join(HOME_DIR_NAME)
}

fn path_from(home: Option<OsString>) -> Option<PathBuf> {
    let home = home.filter(|h| !h.is_empty())?;
    Some(path_in(Path::new(&home)))
}

/// The gateway data dir: `$LUCIDOS_GATEWAY_DATA`, else `~/.lucidos/gateway`.
/// `None` only when neither `LUCIDOS_GATEWAY_DATA` nor `HOME` is set.
pub fn gateway_data_dir() -> Option<PathBuf> {
    gateway_data_dir_from(std::env::var_os(GATEWAY_DATA_ENV), path().as_deref())
}

/// [`gateway_data_dir`] with the Lucidos home given, so a caller that already
/// holds it needs no `HOME`.
pub fn gateway_data_dir_under(lucidos_home: &Path) -> PathBuf {
    gateway_data_dir_from(std::env::var_os(GATEWAY_DATA_ENV), Some(lucidos_home))
        .expect("a Lucidos home was given")
}

fn gateway_data_dir_from(
    relocated: Option<OsString>,
    lucidos_home: Option<&Path>,
) -> Option<PathBuf> {
    match relocated.filter(|dir| !dir.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => Some(lucidos_home?.join(GATEWAY_DIR_NAME)),
    }
}

/// The Lucidos home, made a storage root. Call it before creating anything
/// under the home.
pub fn ensure() -> io::Result<PathBuf> {
    let home = path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    ensure_storage_root(&home)?;
    Ok(home)
}

/// Make `dir` a storage root: create it and its missing parents at 0700, then
/// set it to 0700.
///
/// The `chmod` runs every time, with no check first. A root created before
/// the rule converges here, and no code path decides whether it needs to. A
/// root another user owns fails the `chmod`, and the error names its owner.
pub fn ensure_storage_root(dir: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(STORAGE_ROOT_MODE)
        .create(dir)
        .map_err(|e| annotate(e, dir, "create"))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(STORAGE_ROOT_MODE))
        .map_err(|e| refusal(e, dir))
}

/// Create `dir` as a new storage root, failing with `AlreadyExists` when
/// something is already there. Its parent must exist.
///
/// For a caller that must not reuse a directory, such as a restore whose every
/// failure deletes the tree it created.
///
/// A root it created but could not make owner-only is removed again, so the
/// caller can retry once the cause is fixed.
pub fn create_new_storage_root(dir: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new()
        .mode(STORAGE_ROOT_MODE)
        .create(dir)?;
    ensure_storage_root(dir).inspect_err(|_| {
        let _ = std::fs::remove_dir(dir);
    })
}

fn annotate(err: io::Error, dir: &Path, verb: &str) -> io::Error {
    io::Error::new(
        err.kind(),
        format!("cannot {verb} storage root {}: {err}", dir.display()),
    )
}

fn refusal(err: io::Error, dir: &Path) -> io::Error {
    if err.kind() != io::ErrorKind::PermissionDenied {
        return annotate(err, dir, "set mode 0700 on");
    }
    let owner = std::fs::metadata(dir).map(|m| m.uid().to_string());
    let owner = owner.unwrap_or_else(|_| "unknown".to_string());
    io::Error::new(
        err.kind(),
        format!(
            "storage root {} (owned by uid {owner}) cannot be made owner-only. Either \
             another user owns it, or its filesystem refuses mode changes: {err}",
            dir.display()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set the process umask, returning the previous one. Nothing else in this
    /// test binary reads the umask, and every other test sets explicit modes.
    fn umask(mask: libc::mode_t) -> libc::mode_t {
        // SAFETY: umask has no preconditions and cannot fail.
        unsafe { libc::umask(mask) }
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn the_home_sits_under_home() {
        assert_eq!(
            path_from(Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.lucidos"))
        );
    }

    #[test]
    fn an_unset_or_empty_home_has_no_lucidos_home() {
        assert_eq!(path_from(None), None);
        assert_eq!(path_from(Some(OsString::new())), None);
    }

    #[test]
    fn a_new_root_is_owner_only_at_every_level_whatever_the_umask() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("a/b/root");
        // The widest umask, so only the helper's own mode can make it 0700.
        let previous = umask(0);
        let result = ensure_storage_root(&root);
        umask(previous);
        result.unwrap();
        for dir in [scratch.path().join("a"), scratch.path().join("a/b"), root] {
            assert_eq!(mode_of(&dir), STORAGE_ROOT_MODE, "{}", dir.display());
        }
    }

    #[test]
    fn the_gateway_data_dir_defaults_under_the_home_unless_relocated() {
        let home = Path::new("/home/u/.lucidos");
        assert_eq!(
            gateway_data_dir_from(None, Some(home)),
            Some(PathBuf::from("/home/u/.lucidos/gateway"))
        );
        assert_eq!(
            gateway_data_dir_from(Some("/srv/gw".into()), None),
            Some(PathBuf::from("/srv/gw"))
        );
        // Empty reads as unset, as the shell's `${VAR:-default}` does.
        assert_eq!(
            gateway_data_dir_from(Some(OsString::new()), Some(home)),
            Some(PathBuf::from("/home/u/.lucidos/gateway"))
        );
        assert_eq!(gateway_data_dir_from(None, None), None);
    }

    #[test]
    fn an_existing_open_root_becomes_owner_only_and_its_contents_are_untouched() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("ws");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = root.join("note.md");
        std::fs::write(&file, "x").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();

        ensure_storage_root(&root).unwrap();

        assert_eq!(mode_of(&root), STORAGE_ROOT_MODE);
        assert_eq!(mode_of(&file), 0o644);
    }

    #[test]
    fn asserting_twice_is_harmless() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("ws");
        ensure_storage_root(&root).unwrap();
        ensure_storage_root(&root).unwrap();
        assert_eq!(mode_of(&root), STORAGE_ROOT_MODE);
    }

    #[test]
    fn a_root_another_user_owns_is_refused_naming_the_path_and_its_owner() {
        let own_uid = std::fs::metadata(tempfile::tempdir().unwrap().path())
            .unwrap()
            .uid();
        if own_uid == 0 {
            return; // root may chmod anything, so there is nothing to refuse
        }
        let err = ensure_storage_root(Path::new("/")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        let message = err.to_string();
        assert!(message.contains("storage root /"), "{message}");
        assert!(message.contains("uid 0"), "{message}");
    }

    #[test]
    fn a_new_root_is_created_owner_only_and_an_existing_one_is_refused() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("ws");
        let previous = umask(0);
        let result = create_new_storage_root(&root);
        umask(previous);
        result.unwrap();
        assert_eq!(mode_of(&root), STORAGE_ROOT_MODE);

        let err = create_new_storage_root(&root).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    // ── The shell copies, which cannot import this crate ──────────────────

    const STORAGE_ROOT_SH: &str = include_str!("../../../scripts/lib/storage_root.sh");
    const INSTALL_SH: &str = include_str!("../../../install.sh");
    const SERVICE_SH: &str = include_str!("../../../scripts/lib/service.sh");

    /// The body of the shell function `name` in `script`, braces excluded.
    fn shell_function_body<'a>(script: &'a str, name: &str) -> &'a str {
        let head = format!("\n{name}() {{\n");
        let start = script
            .find(&head)
            .unwrap_or_else(|| panic!("{name} is defined"))
            + head.len();
        let len = script[start..].find("\n}\n").expect("the function closes");
        &script[start..start + len]
    }

    #[test]
    fn the_shell_twin_names_the_same_home() {
        let line = format!("LUCIDOS_HOME_DIR_NAME=\"{HOME_DIR_NAME}\"");
        assert!(STORAGE_ROOT_SH.contains(&line), "storage_root.sh: {line}");
        let prefix = format!("${{LUCIDOS_PREFIX:-$HOME/{HOME_DIR_NAME}}}");
        assert!(INSTALL_SH.contains(&prefix), "install.sh's default prefix");
        let network_toml = format!("/{HOME_DIR_NAME}/network.toml");
        assert!(
            SERVICE_SH.contains(&network_toml),
            "service.sh's network.toml"
        );
        let gateway = shell_function_body(STORAGE_ROOT_SH, "gateway_data_dir");
        let default = format!("${{{GATEWAY_DATA_ENV}:-$(lucidos_home_dir)/{GATEWAY_DIR_NAME}}}");
        assert!(
            gateway.contains(&default),
            "storage_root.sh's gateway_data_dir"
        );
    }

    #[test]
    fn both_shell_helpers_assert_the_same_mode_the_same_way() {
        let lib = shell_function_body(STORAGE_ROOT_SH, "ensure_storage_root");
        let installer = shell_function_body(INSTALL_SH, "ensure_storage_root");
        assert_eq!(
            lib, installer,
            "install.sh's copy drifted from storage_root.sh"
        );
        assert!(
            lib.contains(&format!("chmod {STORAGE_ROOT_MODE:o} ")),
            "{lib}"
        );
        // The umask makes every created parent owner-only too, as DirBuilder does.
        let umask = 0o777 & !STORAGE_ROOT_MODE;
        assert!(lib.contains(&format!("umask {umask:03o} ")), "{lib}");
    }

    #[test]
    fn a_root_under_a_file_fails_naming_the_path() {
        let scratch = tempfile::tempdir().unwrap();
        let file = scratch.path().join("file");
        std::fs::write(&file, "x").unwrap();
        let root = file.join("ws");
        let err = ensure_storage_root(&root).unwrap_err();
        assert!(
            err.to_string().contains(&root.display().to_string()),
            "{err}"
        );
    }
}
