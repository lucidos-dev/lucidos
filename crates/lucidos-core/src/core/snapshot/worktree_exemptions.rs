//! Durable cleanup exemptions live outside the rewound database. No key lookup
//! or initialization is needed to consult them before cleanup starts.
use super::{store, BoxError};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Exemptions {
    workspace: PathBuf,
    paths: BTreeSet<PathBuf>,
}

pub fn read(root: &Path, workspace: &Path) -> Result<BTreeSet<PathBuf>, BoxError> {
    let path = root.join("worktree-exemptions.json");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(error.into()),
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err("invalid snapshot worktree exemptions".into()),
    }
    let saved: Exemptions = serde_json::from_reader(std::fs::File::open(path)?)?;
    if saved.workspace != workspace.canonicalize()?
        || saved.paths.iter().any(|path| {
            path.parent()
                != Some(
                    saved
                        .workspace
                        .join(crate::paths::WORKTREES_SUBPATH)
                        .as_path(),
                )
        })
    {
        return Err("snapshot worktree exemption binding mismatch".into());
    }
    Ok(saved.paths)
}

/// Caller holds the archive lock. Extend rather than replace exemptions from
/// previous restores, including paths absent from the current database.
pub fn preserve(root: &Path, workspace: &Path) -> Result<(), BoxError> {
    let workspace = workspace.canonicalize()?;
    let mut paths = read(root, &workspace)?;
    let entries = match std::fs::read_dir(workspace.join(crate::paths::WORKTREES_SUBPATH)) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if let Some(entries) = entries {
        for entry in entries {
            paths.insert(entry?.path());
        }
    }
    store::write_json(
        &root.join("worktree-exemptions.json"),
        &Exemptions { workspace, paths },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exemptions_include_reclaiming_and_temporary_paths_and_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let trees = workspace.path().join(crate::paths::WORKTREES_SUBPATH);
        std::fs::create_dir_all(&trees).unwrap();
        for name in [
            "thread-12345678",
            "reclaiming-thread-87654321",
            "harden-temp",
        ] {
            std::fs::create_dir(trees.join(name)).unwrap();
        }
        preserve(root.path(), workspace.path()).unwrap();
        assert_eq!(read(root.path(), workspace.path()).unwrap().len(), 3);
        std::fs::remove_dir(trees.join("harden-temp")).unwrap();
        preserve(root.path(), workspace.path()).unwrap();
        assert_eq!(read(root.path(), workspace.path()).unwrap().len(), 3);
        let other = tempfile::tempdir().unwrap();
        assert!(read(root.path(), other.path()).is_err());
        std::fs::write(root.path().join("worktree-exemptions.json"), "broken").unwrap();
        assert!(read(root.path(), workspace.path()).is_err());
        assert!(preserve(root.path(), workspace.path()).is_err());
    }
}
