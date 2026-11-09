use super::{
    handle::Handle,
    journal::Journal,
    manifest::{AuthenticatedManifest, Kind, Manifest},
    BoxError,
};
use crate::core::secret_store::{key_store::KeyStore, SecretKey};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};
#[cfg(test)]
use uuid::Uuid;

pub struct Store {
    root: PathBuf,
    pub workspace_id: String,
}

impl Store {
    pub fn for_database(database: &str, workspace_id: impl Into<String>) -> Result<Self, BoxError> {
        Self::at(Self::database_root(database)?, workspace_id)
    }

    /// Status reads resolve the store without initializing directories or keys.
    pub fn existing_for_database(database: &str, workspace_id: String) -> Result<Self, BoxError> {
        Self::existing_at(Self::database_root(database)?, workspace_id)
    }

    /// Resolve an existing registry without changing permissions or files.
    pub fn existing_at(root: PathBuf, workspace_id: String) -> Result<Self, BoxError> {
        if workspace_id.trim().is_empty() || !std::fs::symlink_metadata(&root)?.is_dir() {
            return Err("snapshot_store_unavailable".into());
        }
        Ok(Self { root, workspace_id })
    }

    /// Resolve without creating a store, so unused snapshot support has no boot side effects.
    pub fn database_root(database: &str) -> Result<PathBuf, BoxError> {
        if database.is_empty()
            || database.starts_with('.')
            || !database
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            return Err("invalid snapshot database name".into());
        }
        let home = lucidos_home::path().ok_or("Lucidos home is unavailable")?;
        Ok(home.join("snapshots").join(database))
    }

    /// Explicit root for the offline worker and disposable test stores.
    pub fn at(root: PathBuf, workspace_id: impl Into<String>) -> Result<Self, BoxError> {
        let workspace_id = workspace_id.into();
        if workspace_id.trim().is_empty() {
            return Err("snapshot_workspace_identity_unavailable".into());
        }
        private_dir(&root)?;
        private_dir(&root.join(".partial"))?;
        private_dir(&root.join("journal"))?;
        Ok(Self { root, workspace_id })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn snapshot_dir(&self, handle: &Handle) -> PathBuf {
        self.root.join(handle.as_str())
    }

    /// Cross-process protection for the offline worker and engine boot cleanup.
    pub fn try_lock(&self) -> Result<StoreLock, BoxError> {
        let file = private_file(&self.root.join("archive-job.lock"))?;
        fs2::FileExt::try_lock_exclusive(&file).map_err(|_| "archive_job_busy")?;
        Ok(StoreLock(file, self.root.clone()))
    }

    pub fn begin(&self, handle: &Handle) -> Result<PathBuf, BoxError> {
        let path = self.root.join(".partial").join(handle.as_str());
        std::fs::create_dir(&path)?;
        restrict(&path, 0o700)?;
        Ok(path)
    }

    /// Publish only after callers have verified all components. The rename is
    /// durable and same-filesystem; no half-written snapshot can be listed.
    pub fn publish(&self, signed: &AuthenticatedManifest, key: &SecretKey) -> Result<(), BoxError> {
        let handle = &signed.manifest.handle;
        signed.verify(key, &self.workspace_id, handle)?;
        let partial = self.root.join(".partial").join(handle.as_str());
        for component in &signed.manifest.components {
            let path = partial.join(component.name.as_str());
            regular_file(&path)?;
            restrict(&path, 0o600)?;
            File::open(path)?.sync_all()?;
        }
        super::check::verify_components(&partial, &signed.manifest)?;
        write_json(&partial.join("manifest.json"), signed)?;
        File::open(&partial)?.sync_all()?;
        let destination = self.snapshot_dir(handle);
        if destination.try_exists()? {
            return Err("snapshot already exists".into());
        }
        std::fs::rename(&partial, destination)?;
        File::open(self.root.join(".partial"))?.sync_all()?;
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }

    pub fn get(&self, handle: &Handle, key: &SecretKey) -> Result<Manifest, BoxError> {
        let dir = self.snapshot_dir(handle);
        if !std::fs::symlink_metadata(&dir)?.is_dir() {
            return Err("invalid snapshot directory".into());
        }
        let path = dir.join("manifest.json");
        regular_file(&path)?;
        let signed: AuthenticatedManifest = serde_json::from_reader(File::open(path)?)?;
        signed.verify(key, &self.workspace_id, handle)?;
        Ok(signed.manifest)
    }

    pub fn list(&self, key: &SecretKey) -> Result<Vec<Manifest>, BoxError> {
        let mut manifests = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(handle) = Handle::try_from(name.to_owned()) {
                    manifests.push(self.get(&handle, key)?);
                }
            }
        }
        manifests.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        Ok(manifests)
    }

    pub fn journals(&self) -> Result<Vec<Journal>, BoxError> {
        let mut result = Vec::new();
        for entry in std::fs::read_dir(self.root.join("journal"))? {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            regular_file(&path)?;
            let journal: Journal = serde_json::from_reader(File::open(&path)?)?;
            if journal.workspace_id != self.workspace_id
                || path.file_stem().and_then(|s| s.to_str())
                    != Some(&journal.restore_id.to_string())
            {
                return Err("invalid snapshot restore journal".into());
            }
            result.push(journal);
        }
        Ok(result)
    }

    pub fn save_journal(&self, journal: &Journal) -> Result<(), BoxError> {
        if journal.workspace_id != self.workspace_id {
            return Err("snapshot_wrong_workspace".into());
        }
        let path = self
            .root
            .join("journal")
            .join(format!("{}.json", journal.restore_id));
        if path.try_exists()? {
            regular_file(&path)?;
            let previous: Journal = serde_json::from_reader(File::open(&path)?)?;
            if previous.workspace_id != journal.workspace_id
                || previous.restore_id != journal.restore_id
                || previous.snapshot != journal.snapshot
                || previous.files.as_ref().is_some_and(|files| {
                    journal.files.as_ref().is_none_or(|next| {
                        files.policy != next.policy
                            || !next.staging_roots.starts_with(&files.staging_roots)
                    })
                })
                || (previous.announcement.is_some()
                    && previous.announcement != journal.announcement)
                || previous.actor != journal.actor
                || previous.reason != journal.reason
                || (previous.recovery.is_some() && previous.recovery != journal.recovery)
                || (previous.phase() != journal.phase()
                    && !previous.phase().allows(journal.phase()))
            {
                return Err("invalid snapshot restore journal update".into());
            }
        } else if journal.phase() != super::journal::Phase::Requested {
            return Err("snapshot journal must start at requested".into());
        }
        write_json(&path, journal)
    }

    pub fn referenced(&self) -> Result<BTreeSet<Handle>, BoxError> {
        let mut handles = BTreeSet::new();
        for journal in self
            .journals()?
            .into_iter()
            .filter(|j| !j.phase().is_terminal() || j.phase().blocks_engine())
        {
            handles.insert(journal.snapshot);
            handles.extend(journal.recovery);
        }
        Ok(handles)
    }

    pub fn delete(&self, handle: &Handle, key: &SecretKey) -> Result<(), BoxError> {
        let manifests = self.list(key)?;
        let manifest = manifests
            .iter()
            .find(|m| &m.handle == handle)
            .ok_or("snapshot not found")?;
        let newest_recovery = manifests
            .iter()
            .find(|m| m.kind == Kind::Recovery)
            .map(|m| &m.handle);
        if manifest.keep || self.referenced()?.contains(handle) || newest_recovery == Some(handle) {
            return Err("snapshot is protected".into());
        }
        // Unpublish with one rename, so a crash never leaves a listed snapshot
        // missing parts. Boot's partial cleanup finishes a cut-short removal.
        let doomed = self.root.join(".partial").join(handle.as_str());
        std::fs::rename(self.snapshot_dir(handle), &doomed)?;
        File::open(&self.root)?.sync_all()?;
        std::fs::remove_dir_all(doomed)?;
        Ok(())
    }

    pub fn set_keep(
        &self,
        handle: &Handle,
        key: &SecretKey,
        keep: bool,
    ) -> Result<Manifest, BoxError> {
        let mut manifest = self.get(handle, key)?;
        manifest.keep = keep;
        let signed = AuthenticatedManifest::seal(manifest.clone(), key)?;
        write_json(&self.snapshot_dir(handle).join("manifest.json"), &signed)?;
        Ok(manifest)
    }

    pub fn preflight_space(&self, estimate: u64) -> Result<(), BoxError> {
        let (available, total) = volume_space(&self.root)?;
        check_floor(available, total, estimate)
    }

    /// Run at boot while holding the store lock. Only stale partial captures
    /// are eligible; worktrees and workspace files are never in this tree.
    pub fn clean_partial(&self, now: DateTime<Utc>, lock: &StoreLock) -> Result<(), BoxError> {
        if lock.1 != self.root {
            return Err("snapshot store lock mismatch".into());
        }
        for entry in std::fs::read_dir(self.root.join(".partial"))? {
            let entry = entry?;
            let metadata = std::fs::symlink_metadata(entry.path())?;
            let valid_handle = entry
                .file_name()
                .to_str()
                .is_some_and(|s| Handle::try_from(s.to_owned()).is_ok());
            let age = now.signed_duration_since(DateTime::<Utc>::from(metadata.modified()?));
            if valid_handle && metadata.is_dir() && age.num_seconds() > 3600 {
                std::fs::remove_dir_all(entry.path())?;
            }
        }
        Ok(())
    }
}

/// Names the extraction tree a restore stages under `.partial`.
pub(super) const RESTORE_STAGING_PREFIX: &str = "restore-";

impl Store {
    /// Remove the staging trees a settled restore left behind. Every original
    /// they hold is also in the verified recovery snapshot (ADR 0612). It
    /// touches only a tree named like this store's own staging, whatever a
    /// journal lists. A tree held by an unsettled restore stays.
    pub fn discard_settled_staging(&self, lock: &StoreLock) -> Result<(), BoxError> {
        use super::journal::Phase;
        if lock.1 != self.root {
            return Err("snapshot store lock mismatch".into());
        }
        let partial = self.root.join(".partial");
        for journal in self.journals()? {
            if !matches!(
                journal.phase(),
                Phase::Completed | Phase::RolledBack | Phase::FailedUnchanged
            ) {
                continue;
            }
            for root in journal.files.iter().flat_map(|files| &files.staging_roots) {
                let name = root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                let ours = name.starts_with(super::file_swap::SWAP_STAGING_PREFIX)
                    || (root.parent() == Some(partial.as_path())
                        && name.starts_with(RESTORE_STAGING_PREFIX));
                match std::fs::symlink_metadata(root) {
                    Ok(metadata) if ours && metadata.is_dir() => std::fs::remove_dir_all(root)?,
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

pub struct StoreLock(File, PathBuf);
impl Drop for StoreLock {
    fn drop(&mut self) {
        if fs2::FileExt::unlock(&self.0).is_err() {
            crate::log!("[Snapshots] Cannot release archive lock");
        }
    }
}

pub fn mac_key(store: &dyn KeyStore, database: &str) -> Result<SecretKey, BoxError> {
    let name = crate::core::secret_store::key_store::snapshot_key_name(database);
    if let Some(key) = store.get(&name)? {
        return Ok(key);
    }
    let key = SecretKey::generate();
    store.put_new(&name, &key)?;
    store
        .get(&name)?
        .ok_or_else(|| "snapshot MAC key unavailable".into())
}

pub fn retention_victims(
    manifests: &[Manifest],
    referenced: &BTreeSet<Handle>,
    manual: usize,
    recovery: usize,
) -> Vec<Handle> {
    let mut sorted: Vec<_> = manifests.iter().collect();
    sorted.sort_by_key(|m| std::cmp::Reverse(m.created_at));
    let mut counts = [0usize; 2];
    let mut victims = Vec::new();
    for manifest in sorted {
        if manifest.keep || referenced.contains(&manifest.handle) {
            continue;
        }
        let index = usize::from(manifest.kind == Kind::Recovery);
        counts[index] += 1;
        let limit = if index == 0 { manual } else { recovery.max(1) };
        if counts[index] > limit {
            victims.push(manifest.handle.clone());
        }
    }
    victims
}

/// The free-disk floor refused a capture or restore. `needed_bytes` is the
/// space the user must free. The capture job branches on this type to tell the
/// user; every other caller sees only the `snapshot_disk_floor` slug.
#[derive(Debug)]
pub struct DiskFloorRefused {
    pub needed_bytes: u64,
}

impl std::fmt::Display for DiskFloorRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("snapshot_disk_floor")
    }
}

impl std::error::Error for DiskFloorRefused {}

/// Available and total bytes on the volume holding `path`.
#[cfg(not(test))]
pub(super) fn volume_space(path: &Path) -> std::io::Result<(u64, u64)> {
    Ok((fs2::available_space(path)?, fs2::total_space(path)?))
}

/// Tests read a fixed, roomy volume, so no snapshot test depends on how
/// full the host's disk is. The floor arithmetic has its own tests.
#[cfg(test)]
pub(super) fn volume_space(_path: &Path) -> std::io::Result<(u64, u64)> {
    const TIB: u64 = 1 << 40;
    Ok((TIB, 2 * TIB))
}

pub fn check_floor(available: u64, total: u64, estimate: u64) -> Result<(), BoxError> {
    let floor = (5 * 1024 * 1024 * 1024u64).max(total.div_ceil(10));
    match floor.checked_add(estimate) {
        Some(needed) if available >= needed => Ok(()),
        // A need past `u64::MAX` can never be met, so report the most there is.
        needed => Err(Box::new(DiskFloorRefused {
            needed_bytes: needed.map_or(u64::MAX, |needed| needed - available),
        })),
    }
}

pub(super) fn regular_file(path: &Path) -> Result<(), BoxError> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err("invalid snapshot component".into());
    }
    Ok(())
}

pub(super) fn private_dir(path: &Path) -> Result<(), BoxError> {
    std::fs::create_dir_all(path)?;
    if !std::fs::symlink_metadata(path)?.is_dir() {
        return Err("invalid snapshot directory".into());
    }
    restrict(path, 0o700)
}

fn private_file(path: &Path) -> Result<File, BoxError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).read(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    restrict(path, 0o600)?;
    Ok(file)
}

fn restrict(path: &Path, mode: u32) -> Result<(), BoxError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> Result<(), BoxError> {
    let parent = path.parent().ok_or("snapshot path has no parent")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    restrict(staged.path(), 0o600)?;
    staged.write_all(&serde_json::to_vec(value)?)?;
    staged.as_file().sync_all()?;
    staged.persist(path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
