//! The private subtree: `~/.lucidos/private`, where every secret and control
//! file lives.
//!
//! An agent subprocess runs in an OS sandbox that denies this whole directory
//! (ADR 0645). A secret written here later is covered with no list to update.
//! Shared caches an agent uses, such as `venvs/`, stay outside it.
//!
//! # Homes an older release created
//!
//! Older releases kept these entries directly in the Lucidos home, and older
//! binaries keep running through an upgrade. So nothing moves them and nothing
//! links them. A shipped binary refuses a linked socket directory, and its
//! `network.toml` writer replaces a link with a file.
//!
//! Instead [`entry_in`] resolves each entry to the path an older binary also
//! opens, and the sandbox denies those old paths by name ([`legacy_entries`]).
//! Registry row: "Older homes keep their private entries in place" in
//! `docs/temporary-measures.md`.

use std::io;
use std::path::{Path, PathBuf};

pub const DIR_NAME: &str = "private";

pub const LOCAL_TOKEN: &str = "local-token";

pub const HOOK_SOCKET_TOKEN: &str = "hook-socket-token";

pub const SHARED_PG_PASSWORD: &str = "pg-shared-password";

pub const NETWORK_TOML: &str = "network.toml";

pub const NETWORK_TOML_LOCK: &str = "network.toml.lock";

pub const PAIRED_DEVICES: &str = "paired-devices.json";

pub const RUN_DIR: &str = "run";

pub const SECRET_KEYS_DIR: &str = "secret-keys";

pub const SNAPSHOTS_DIR: &str = "snapshots";

pub const LOOPBACK_CA_DIR: &str = "loopback-ca";

const LEGACY_WEBHOOK_TOKEN: &str = "webhook-token";

pub const MINT_LOCK_SUFFIX: &str = "lock";

pub fn mint_lock_name(name: &str) -> String {
    format!(".{name}.{MINT_LOCK_SUFFIX}")
}

const MINTED: [&str; 3] = [LOCAL_TOKEN, HOOK_SOCKET_TOKEN, SHARED_PG_PASSWORD];

pub fn legacy_entries() -> Vec<String> {
    let mut names: Vec<String> = [
        NETWORK_TOML,
        NETWORK_TOML_LOCK,
        PAIRED_DEVICES,
        LEGACY_WEBHOOK_TOKEN,
        RUN_DIR,
        SECRET_KEYS_DIR,
        SNAPSHOTS_DIR,
        crate::GATEWAY_DIR_NAME,
    ]
    .iter()
    .chain(MINTED.iter())
    .map(|name| name.to_string())
    .collect();
    names.extend(MINTED.iter().map(|name| mint_lock_name(name)));
    names
}

pub const DIR_ENV: &str = "LUCIDOS_PRIVATE_DIR";

pub fn relocated() -> Option<PathBuf> {
    relocated_from(std::env::var_os(DIR_ENV))
}

fn relocated_from(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value.filter(|dir| !dir.is_empty()).map(PathBuf::from)
}

pub fn path_in(lucidos_home: &Path) -> PathBuf {
    lucidos_home.join(DIR_NAME)
}

pub fn entry_in(lucidos_home: &Path, name: &str) -> PathBuf {
    let private = path_in(lucidos_home).join(name);
    if !legacy_entries().iter().any(|legacy| legacy == name) {
        return private;
    }
    let legacy = lucidos_home.join(name);
    if occupied(&legacy) {
        return legacy;
    }
    if occupied(&private) || !holds_legacy_entries(lucidos_home) {
        return private;
    }
    legacy
}

pub fn holds_legacy_entries(lucidos_home: &Path) -> bool {
    legacy_entries()
        .iter()
        .any(|name| occupied(&lucidos_home.join(name)))
}

fn occupied(path: &Path) -> bool {
    !matches!(
        std::fs::symlink_metadata(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound
    )
}

#[cfg(test)]
#[path = "private_tests.rs"]
mod tests;

const PATHS_FILE: &str = "private-paths";

pub fn machine_paths_file() -> Option<PathBuf> {
    crate::private_entry(PATHS_FILE)
}

pub fn workspace_paths_file(workspace: &Path) -> PathBuf {
    workspace.join(".lucidos").join(PATHS_FILE)
}

pub fn read_paths(file: &Path) -> io::Result<Vec<PathBuf>> {
    match std::fs::read_to_string(file) {
        Ok(text) => validate_paths(text.lines().map(str::to_owned).collect()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

pub fn validate_paths(paths: Vec<String>) -> io::Result<Vec<PathBuf>> {
    let mut paths = paths
        .into_iter()
        .map(|value| {
            let path = PathBuf::from(&value);
            if !path.is_absolute()
                || value.contains(['\n', '\r', '\0'])
                || path
                    .components()
                    .any(|part| part == std::path::Component::ParentDir)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Use absolute paths without parent traversal or line breaks.",
                ));
            }
            Ok(path.components().collect())
        })
        .collect::<io::Result<Vec<PathBuf>>>()?;
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub fn replace_paths(file: &Path, previous: &[PathBuf], paths: &[PathBuf]) -> io::Result<bool> {
    let parent = file.parent().ok_or(io::ErrorKind::InvalidInput)?;
    crate::ensure_storage_root(parent)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(parent.join(format!(".{PATHS_FILE}.lock")))?;
    lock.lock()?;
    if read_paths(file)? != previous {
        return Ok(false);
    }
    write_paths(file, paths)?;
    Ok(true)
}

fn write_paths(file: &Path, paths: &[PathBuf]) -> io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = file.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let (temp, mut output) = loop {
        let temp = parent.join(format!(
            ".{PATHS_FILE}.{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&temp) {
            Ok(output) => break (temp, output),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        for path in paths {
            writeln!(output, "{}", path.display())?;
        }
        output.sync_all()?;
        std::fs::rename(&temp, file)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

pub struct EffectivePaths {
    pub built_in: Vec<PathBuf>,
    pub configured: Vec<PathBuf>,
}

pub fn built_in_paths(home: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = home {
        paths.push(path_in(home));
        paths.extend(legacy_entries().into_iter().map(|name| home.join(name)));
    }
    paths
}

pub fn effective_paths(workspace: &Path) -> io::Result<EffectivePaths> {
    let mut built_in = built_in_paths(crate::path().as_deref());
    built_in.extend(relocated());
    let mut configured = match machine_paths_file() {
        Some(file) => read_paths(&file)?,
        None => Vec::new(),
    };
    configured.extend(read_paths(&workspace_paths_file(workspace))?);
    let aliases: Vec<_> = configured.iter().map(|path| resolve_path(path)).collect();
    configured.extend(aliases);
    configured.sort();
    configured.dedup();
    Ok(EffectivePaths {
        built_in,
        configured,
    })
}

pub fn resolve_path(path: &Path) -> PathBuf {
    for ancestor in path.ancestors() {
        if let Ok(real) = ancestor.canonicalize() {
            if let Ok(suffix) = path.strip_prefix(ancestor) {
                return real.join(suffix);
            }
        }
    }
    path.to_path_buf()
}
pub fn overlaps(path: &Path, other: &Path) -> bool {
    let overlaps = |a: &Path, b: &Path| a.starts_with(b) || b.starts_with(a);
    let real = resolve_path(path);
    let other_real = resolve_path(other);
    overlaps(path, other)
        || overlaps(&real, &other_real)
        || overlaps(path, &other_real)
        || overlaps(&real, other)
}
