//! Preservation-aware offline file installation. The caller holds the archive
//! locks and has durably journaled recovery before `apply`. Staging and trash
//! intentionally survive errors/crashes; recovery is the authoritative rollback.
use super::{check, manifest::Manifest, restore_files, BoxError};
use crate::core::backup::{
    capture::FileIndexEntry, classify_symlink, is_excluded_workspace_path, BackupIgnore,
    SymlinkAction,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

/// Stored externally before mutation; rollback never reads the restored policy.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenPolicy {
    workspace: PathBuf,
    data_link: Option<PathBuf>,
    patterns: Vec<String>,
    links: BTreeSet<PathBuf>,
}

impl FrozenPolicy {
    pub fn capture(workspace: &Path) -> Result<Self, BoxError> {
        let workspace = workspace.canonicalize()?;
        let ignore = BackupIgnore::load_for_restore(&workspace)?;
        let (_, links) = inventory(&workspace, &ignore)?;
        let data_link = data_binding(&workspace)?;
        Ok(Self {
            workspace,
            data_link,
            patterns: ignore.normalized_patterns()?,
            links,
        })
    }

    fn validate(&self, workspace: &Path) -> Result<BackupIgnore, BoxError> {
        if workspace.canonicalize()? != self.workspace
            || data_binding(workspace)? != self.data_link
            || self
                .links
                .iter()
                .any(|path| !restore_files::restorable_path(path))
        {
            return Err("snapshot preservation policy binding changed".into());
        }
        let ignore = BackupIgnore::from_normalized_patterns(&self.patterns)?;
        if inventory(workspace, &ignore)?.1 != self.links {
            return Err("snapshot preserved symbolic links changed".into());
        }
        Ok(ignore)
    }
}

fn data_binding(workspace: &Path) -> Result<Option<PathBuf>, BoxError> {
    if workspace.join("data").is_symlink() {
        Ok(Some(workspace.join("data").canonicalize()?))
    } else {
        Ok(None)
    }
}

/// Names the staging tree a swap creates beside each live root.
pub(super) const SWAP_STAGING_PREFIX: &str = ".lucidos-snapshot-swap-";

pub struct FileSwap {
    workspace: PathBuf,
    ignore: BackupIgnore,
    links: BTreeSet<PathBuf>,
    expected: BTreeMap<PathBuf, FileIndexEntry>,
    operations: Vec<Operation>,
    /// Retained for diagnosis, never automatically removed on drop.
    pub staging_roots: Vec<PathBuf>,
}

struct Operation {
    target: PathBuf,
    staged: Option<PathBuf>,
    trash: PathBuf,
}

impl FileSwap {
    /// [`Self::prepare_with_policy`] under the workspace's current policy.
    #[cfg(test)]
    pub fn prepare(
        archive: &Path,
        manifest: &Manifest,
        staged: &Path,
        workspace: &Path,
    ) -> Result<Self, BoxError> {
        let policy = FrozenPolicy::capture(workspace)?;
        Self::prepare_with_policy(archive, manifest, staged, workspace, &policy)
    }

    /// Authenticate/check the archive before calling this method. No live file
    /// is changed here. Every replacement is copied, checked and fsynced on its
    /// destination device before the first live rename is permitted.
    pub fn prepare_with_policy(
        archive: &Path,
        manifest: &Manifest,
        staged: &Path,
        workspace: &Path,
        policy: &FrozenPolicy,
    ) -> Result<Self, BoxError> {
        let ignore = policy.validate(workspace)?;
        restore_files::verify_tree(archive, manifest, staged)?;
        let workspace = workspace.canonicalize()?;
        let mut live_roots = vec![workspace.clone()];
        if workspace.join("data").is_symlink() {
            live_roots.push(workspace.join("data").canonicalize()?);
        }
        for input in [archive, staged] {
            let input = input.canonicalize()?;
            if live_roots.iter().any(|root| input.starts_with(root)) {
                return Err("snapshot swap inputs must be outside the live trees".into());
            }
        }
        let (current, links) = inventory(&workspace, &ignore)?;
        let index = BufReader::new(zstd::Decoder::new(File::open(
            archive.join("files.index.jsonl.zst"),
        )?)?);
        let mut expected = BTreeMap::new();
        for line in index.lines() {
            let record: FileIndexEntry = serde_json::from_str(&line?)?;
            if !is_excluded_workspace_path(&record.path, &ignore)
                && !links.iter().any(|link| record.path.starts_with(link))
            {
                expected.insert(record.path.clone(), record);
            }
        }
        // Current links win, including a link that blocks an archived parent.
        // Refuse an incompatible shape rather than removing preserved content.
        for path in expected.keys() {
            if links.iter().any(|link| link.starts_with(path)) {
                return Err("snapshot file conflicts with a preserved symbolic link".into());
            }
            let target = workspace.join(path);
            if target.is_dir() {
                check_replaceable_directory(&target, &workspace, &current)?;
            }
        }
        let mut roots = BTreeMap::new();
        let mut operations = Vec::new();
        let paths: BTreeSet<_> = current
            .union(&expected.keys().cloned().collect())
            .cloned()
            .collect();
        for (number, relative) in paths.into_iter().enumerate() {
            let target = workspace.join(&relative);
            let base = if relative.starts_with("data")
                && workspace.join("data").is_symlink()
                && !same_filesystem(&target, &workspace)?
            {
                workspace.join("data").canonicalize()?
            } else {
                workspace.clone()
            };
            let parent = base
                .parent()
                .ok_or("snapshot volume has no staging parent")?;
            if live_roots.iter().any(|root| parent.starts_with(root)) {
                return Err("snapshot volume has no external staging parent".into());
            }
            let root = match roots.get(parent) {
                Some(root) => PathBuf::clone(root),
                None => {
                    let root = tempfile::Builder::new()
                        .prefix(SWAP_STAGING_PREFIX)
                        .tempdir_in(parent)?
                        .keep();
                    File::open(parent)?.sync_all()?;
                    roots.insert(parent.to_owned(), root.clone());
                    root
                }
            };
            same_device(&target, &root)?;
            let replacement = if let Some(record) = expected.get(&relative) {
                let path = root.join(format!("new-{number}"));
                std::fs::copy(staged.join(&relative), &path)?;
                verify_file(&path, record)?;
                File::open(&path)?.sync_all()?;
                Some(path)
            } else {
                None
            };
            operations.push(Operation {
                target,
                staged: replacement,
                trash: root.join(format!("old-{number}")),
            });
        }
        for root in roots.values() {
            File::open(root)?.sync_all()?;
        }
        Ok(Self {
            workspace,
            ignore,
            links,
            expected,
            operations,
            staging_roots: roots.into_values().collect(),
        })
    }

    /// Call only after persisting FileSwap in the external restore journal.
    /// A failure (including interruption) requires recovery-snapshot rollback;
    /// this operation is deliberately not advertised as retryable.
    pub fn apply(&self) -> Result<(), BoxError> {
        // Remove leaves first, allowing directory/file shape changes without
        // replacing a directory containing excluded descendants.
        for operation in &self.operations {
            match std::fs::symlink_metadata(&operation.target) {
                Ok(metadata) if metadata.is_file() => {
                    std::fs::rename(&operation.target, &operation.trash)?;
                    sync_parent(&operation.target)?;
                    sync_parent(&operation.trash)?;
                }
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => return Err("snapshot swap encountered a changed entry type".into()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        for operation in &self.operations {
            let Some(staged) = &operation.staged else {
                continue;
            };
            if operation.target.is_dir() {
                remove_empty_tree(&operation.target)?;
            }
            create_parents(
                &self.workspace,
                operation.target.parent().ok_or("missing parent")?,
            )?;
            std::fs::rename(staged, &operation.target)?;
            sync_parent(staged)?;
            sync_parent(&operation.target)?;
        }
        self.verify()
    }

    /// Compare eligible live files against the archive using the *original*
    /// ignore policy, even after data/.backupignore itself has been restored.
    pub fn verify(&self) -> Result<(), BoxError> {
        let (actual, links) = inventory(&self.workspace, &self.ignore)?;
        if actual != self.expected.keys().cloned().collect() || links != self.links {
            return Err("restored snapshot live tree differs from expected files".into());
        }
        for (relative, record) in &self.expected {
            verify_file(&self.workspace.join(relative), record)?;
        }
        Ok(())
    }
}

fn inventory(
    root: &Path,
    ignore: &BackupIgnore,
) -> Result<(BTreeSet<PathBuf>, BTreeSet<PathBuf>), BoxError> {
    fn visit(
        root: &Path,
        dir: &Path,
        ignore: &BackupIgnore,
        files: &mut BTreeSet<PathBuf>,
        links: &mut BTreeSet<PathBuf>,
    ) -> Result<(), BoxError> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path.strip_prefix(root)?;
            if !restore_files::restorable_path(relative) {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                match classify_symlink(&path, root)? {
                    SymlinkAction::Follow => visit(root, &path, ignore, files, links)?,
                    SymlinkAction::Skip => {
                        links.insert(relative.to_owned());
                    }
                }
            } else if kind.is_dir() {
                // Use capture's shared predicate at each leaf. It owns both
                // prefix and glob ancestor semantics, including exclusions
                // nested inside otherwise restorable directories.
                visit(root, &path, ignore, files, links)?;
            } else if !is_excluded_workspace_path(relative, ignore) {
                if !kind.is_file() {
                    return Err("snapshot live tree contains a nonregular entry".into());
                }
                files.insert(relative.to_owned());
            }
        }
        Ok(())
    }
    let mut files = BTreeSet::new();
    let mut links = BTreeSet::new();
    visit(root, root, ignore, &mut files, &mut links)?;
    Ok((files, links))
}

fn verify_file(path: &Path, record: &FileIndexEntry) -> Result<(), BoxError> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err("snapshot replacement is not a regular file".into());
    }
    let (size, hash) = check::component_hash(path)?;
    if size != record.size || hash != record.sha256 {
        return Err("snapshot replacement hash mismatch".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(path)?.permissions().mode() & 0o7777 != record.mode & 0o7777 {
            return Err("snapshot replacement mode mismatch".into());
        }
    }
    Ok(())
}

fn sync_parent(path: &Path) -> Result<(), BoxError> {
    File::open(path.parent().ok_or("missing snapshot parent")?)?.sync_all()?;
    Ok(())
}

fn same_device(target: &Path, stage: &Path) -> Result<(), BoxError> {
    if !same_filesystem(target, stage)? {
        return Err("snapshot staging would require a cross-device move".into());
    }
    Ok(())
}

fn same_filesystem(target: &Path, stage: &Path) -> Result<bool, BoxError> {
    let mut ancestor = target;
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or("missing snapshot volume")?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(std::fs::metadata(ancestor)?.dev() == std::fs::metadata(stage)?.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = (ancestor, stage);
        Err("snapshot file swaps require a supported Unix platform".into())
    }
}

fn remove_empty_tree(path: &Path) -> Result<(), BoxError> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            return Err("snapshot swap would remove preserved content".into());
        }
        remove_empty_tree(&entry.path())?;
    }
    std::fs::remove_dir(path)?;
    sync_parent(path)
}

fn create_parents(root: &Path, path: &Path) -> Result<(), BoxError> {
    if path == root {
        return Ok(());
    }
    if path.exists() {
        if path.is_dir() && (!path.is_symlink() || path == root.join("data")) {
            return Ok(());
        }
        return Err("snapshot parent is not a directory".into());
    }
    create_parents(root, path.parent().ok_or("missing snapshot parent")?)?;
    std::fs::create_dir(path)?;
    sync_parent(path)
}

fn check_replaceable_directory(
    path: &Path,
    root: &Path,
    current: &BTreeSet<PathBuf>,
) -> Result<(), BoxError> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            check_replaceable_directory(&entry.path(), root, current)?;
        } else if !current.contains(entry.path().strip_prefix(root)?) {
            return Err("snapshot file conflicts with preserved content".into());
        }
    }
    Ok(())
}
