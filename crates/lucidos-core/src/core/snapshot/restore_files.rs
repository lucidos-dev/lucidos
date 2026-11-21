//! Offline restore file staging. Callers authenticate the manifest against the
//! workspace before invoking these helpers. Nothing here mutates the live tree.
use super::{check, manifest::Manifest, BoxError};
use crate::core::backup::capture::FileIndexEntry;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufRead, BufReader},
    path::{Component, Path, PathBuf},
};

/// Shared by archive checking and extraction: even an authenticated archive
/// must never replace runtime state, worktrees, or physical PostgreSQL data.
pub(super) fn restorable_path(path: &Path) -> bool {
    if path.as_os_str().is_empty()
        || !path.components().all(|p| matches!(p, Component::Normal(_)))
        || path.to_str().is_none_or(|p| p.starts_with('\\'))
        || path.starts_with(".lucidos")
    {
        return false;
    }
    let mut components = path.components();
    !matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(first)), Some(Component::Normal(second)))
            if first == "data"
                && (second == "postgres" || second.to_string_lossy().starts_with("postgres."))
    )
}

/// `destination` must not exist. It belongs to this attempt only after the
/// exclusive mkdir succeeds. Failed staging is retained for diagnosis; no
/// failure path recursively deletes a directory supplied by a caller.
pub fn stage(directory: &Path, manifest: &Manifest, destination: &Path) -> Result<(), BoxError> {
    check::verify_components(directory, manifest)?;
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(destination)?;
    let mut archive = tar::Archive::new(zstd::Decoder::new(File::open(
        directory.join("files.tar.zst"),
    )?)?);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if !restorable_path(&path) || !entry.header().entry_type().is_file() {
            return Err("snapshot contains a protected path".into());
        }
        if !entry.unpack_in(destination)? {
            return Err("snapshot staging path refused".into());
        }
    }
    verify_tree(directory, manifest, destination)
}

/// Verify the installed bytes and modes against the authenticated index, and
/// refuse extra files or links. Intended for staging; a live workspace also
/// contains excluded paths and needs a separate preservation-aware walk.
pub fn verify_tree(directory: &Path, manifest: &Manifest, tree: &Path) -> Result<(), BoxError> {
    check::verify_components(directory, manifest)?;
    if !std::fs::symlink_metadata(tree)?.is_dir() {
        return Err("snapshot tree must be a directory".into());
    }
    let index = BufReader::new(zstd::Decoder::new(File::open(
        directory.join("files.index.jsonl.zst"),
    )?)?);
    let mut expected = BTreeSet::new();
    for line in index.lines() {
        let record: FileIndexEntry = serde_json::from_str(&line?)?;
        if !restorable_path(&record.path) {
            return Err("snapshot contains a protected path".into());
        }
        let mut path = tree.to_owned();
        for part in record.path.components() {
            path.push(part);
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return Err("snapshot tree contains a symbolic link".into());
            }
        }
        let (bytes, hash) = check::component_hash(&path)?;
        if bytes != record.size || hash != record.sha256 {
            return Err("restored snapshot file hash mismatch".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::metadata(&path)?.permissions().mode() & 0o7777 != record.mode & 0o7777 {
                return Err("restored snapshot file mode mismatch".into());
            }
        }
        if !expected.insert(record.path) {
            return Err("duplicate snapshot index path".into());
        }
    }
    let mut actual = BTreeSet::new();
    tree_files(tree, tree, &mut actual)?;
    if actual != expected {
        return Err("restored snapshot tree contains unexpected files".into());
    }
    Ok(())
}

fn tree_files(root: &Path, path: &Path, files: &mut BTreeSet<PathBuf>) -> Result<(), BoxError> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            tree_files(root, &entry.path(), files)?;
        } else if kind.is_file() {
            files.insert(entry.path().strip_prefix(root)?.to_owned());
        } else {
            return Err("snapshot tree contains a nonregular entry".into());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "restore_files_tests.rs"]
mod tests;
