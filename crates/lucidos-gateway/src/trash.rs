//! Durable delete journal. A directory move must succeed before database teardown.
use crate::{cleanup_process::CleanupCommand, registry::Workspace, BoxError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Safe projection: the stored Workspace also carries credentials and must
/// never be serialized directly to the picker.
#[derive(Serialize)]
pub struct Listing {
    pub id: String,
    pub name: String,
    pub status: String,
    pub restore_available: bool,
    /// The journaled workspace is machine-made, so an agent may restore it.
    pub machine_made: bool,
    pub bytes: Option<u64>,
    pub warning: Option<String>,
}

pub fn list(app_data: &Path) -> Result<Vec<Listing>, BoxError> {
    let mut out = Vec::new();
    for (directory, prefix) in [("deleted", "workspace"), ("deleted-snapshots", "snapshots")] {
        let parent = app_data.join(directory);
        let entries = match std::fs::read_dir(&parent) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        for item in entries {
            let item = item?;
            let name = item
                .file_name()
                .into_string()
                .map_err(|_| "Trash entry name is not UTF-8")?;
            if name.starts_with(".preparing-") {
                continue;
            }
            let mut row = Listing {
                id: if directory == "deleted" {
                    name.clone()
                } else {
                    format!("{prefix}:{name}")
                },
                name,
                status: "legacy".into(),
                restore_available: false,
                machine_made: false,
                bytes: None,
                warning: None,
            };
            if !item.file_type()?.is_dir() {
                row.status = "needs-attention".into();
                row.warning = Some("Trash entry is not a directory".into());
                out.push(row);
                continue;
            }
            if directory == "deleted"
                && row.id.starts_with("database-")
                && !legacy_folder(&parent, &row.id)?
            {
                match DatabaseEntry::load(&parent, &row.id) {
                    Ok((entry, root)) => {
                        entry.repair_audit(&root)?;
                        if !entry.abandoned_staging.is_empty() {
                            row.warning = Some(format!("Abandoned staging databases were preserved and still use disk space: {}", entry.abandoned_staging.join(", ")));
                        }
                        row.name = entry.database;
                        row.restore_available = entry.phase == Phase::Prepared
                            || (entry.dump_ready && entry.phase != Phase::Restored);
                        row.status = serde_json::to_value(&entry.phase)?
                            .as_str()
                            .ok_or("invalid phase")?
                            .into();
                        row.bytes = crate::cleanup::allocated_bytes(&root).ok();
                    }
                    Err(_) => {
                        row.status = "needs-attention".into();
                        row.warning = Some("Database Trash journal could not be read".into());
                    }
                }
                out.push(row);
                continue;
            }
            let journal = item.path().join("entry.json");
            match std::fs::symlink_metadata(&journal) {
                Ok(metadata) => {
                    let read = (|| -> Result<Entry, BoxError> {
                        if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
                            return Err("invalid Trash journal".into());
                        }
                        Ok(serde_json::from_reader(std::fs::File::open(&journal)?)?)
                    })();
                    match read {
                        Ok(entry) => {
                            entry.repair_audit(&item.path())?;
                            row.restore_available = match entry.phase {
                                Phase::Prepared => entry.source_identity.is_some(),
                                Phase::Moved
                                | Phase::DatabaseDropped
                                | Phase::Completed
                                | Phase::Restoring => entry.workspace.is_none() || entry.dump_ready,
                                Phase::Restored => false,
                            };
                            if entry.workspace.is_some()
                                && !entry.dump_ready
                                && matches!(entry.phase, Phase::DatabaseDropped | Phase::Completed)
                            {
                                row.warning = Some("No committed database recovery copy; full workspace restore is unavailable".into());
                            }
                            if let Some(recovery) = &entry.recovery {
                                if !recovery.abandoned_staging.is_empty() {
                                    row.warning = Some(format!("Abandoned staging databases were preserved and still use disk space: {}", recovery.abandoned_staging.join(", ")));
                                }
                            }
                            if !entry.source_repositories.is_empty() {
                                let warning = row.warning.get_or_insert_with(String::new);
                                if !warning.is_empty() {
                                    warning.push(' ');
                                }
                                warning.push_str("Linked worktrees restore with detached HEADs at saved commits; reattach branches explicitly. Archives without saved commits require manual recovery.");
                            }
                            row.machine_made = entry
                                .workspace
                                .as_ref()
                                .is_some_and(|ws| ws.cleanup.origin.is_machine_made());
                            row.name = entry
                                .workspace
                                .as_ref()
                                .map(|ws| ws.name.clone())
                                .unwrap_or_else(|| entry.original_dir.display().to_string());
                            row.status = match entry.phase {
                                Phase::Prepared => "prepared",
                                Phase::Moved => "moved",
                                Phase::DatabaseDropped => "database-dropped",
                                Phase::Completed => "completed",
                                Phase::Restoring => "restoring",
                                Phase::Restored => "restored",
                            }
                            .into();
                        }
                        Err(_) => {
                            row.status = "needs-attention".into();
                            row.warning = Some(
                                "Trash journal could not be read; keep this entry for recovery"
                                    .into(),
                            );
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    row.status = "needs-attention".into();
                    row.warning = Some("Trash journal is unavailable".into());
                }
            }
            match crate::cleanup::allocated_bytes(&item.path()) {
                Ok(bytes) => row.bytes = Some(bytes),
                Err(_) => row.warning = Some("Trash size is unavailable".into()),
            }
            out.push(row);
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Prepared,
    Moved,
    DatabaseDropped,
    Completed,
    Restoring,
    Restored,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub(crate) audit: Option<crate::cleanup_audit::Audit>,
    pub workspace: Option<Workspace>,
    pub original_dir: PathBuf,
    pub phase: Phase,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub source_repositories: Vec<PathBuf>,
    #[serde(default)]
    pub recovery: Option<Recovery>,
    #[serde(default)]
    pub dump_ready: bool,
    #[serde(default)]
    pub deletion_operation: Option<String>,
    #[serde(default)]
    source_identity: Option<(u64, u64)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recovery {
    pub operation: String,
    #[serde(default)]
    pub abandoned_staging: Vec<String>,
    folder: (u64, u64),
    snapshots: Option<(u64, u64)>,
}

/// Never replace even an empty destination directory. Both supported platforms
/// provide an atomic exclusive rename; ordinary rename overwrites empty dirs.
pub(crate) fn rename_exclusive(source: &Path, target: &Path) -> Result<(), BoxError> {
    use std::os::unix::ffi::OsStrExt;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    let target = std::ffi::CString::new(target.as_os_str().as_bytes())?;
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

// Probe the exact rename primitive before publishing a journal or touching a database.
// Cross-device and unsupported filesystems refuse cleanup while the original stays usable.
fn preflight_folder_move(source: &Path, trash: &Path) -> Result<(), BoxError> {
    let parent = source.parent().ok_or("workspace has no parent")?;
    let mount = cleanup_mount_identity(source)?;
    if mount != cleanup_mount_identity(parent)? || mount != cleanup_mount_identity(trash)? {
        return Err("Workspace cleanup cannot move a mounted volume or cross filesystems; no resources changed".into());
    }
    let probe = tempfile::Builder::new()
        .prefix(".lucidos-move-probe-")
        .tempdir_in(parent)?;
    let destination = tempfile::tempdir_in(trash)?;
    let moved = destination.path().join("probe");
    rename_exclusive(probe.path(), &moved).map_err(|error| {
        format!("Workspace cleanup needs atomic rename to Trash on the same filesystem; no resources changed: {error}")
    })?;
    std::fs::remove_dir(moved)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn cleanup_mount_identity(path: &Path) -> Result<u64, BoxError> {
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::metadata(path)?.dev())
}

#[cfg(target_os = "linux")]
fn cleanup_mount_identity(path: &Path) -> Result<u64, BoxError> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    // O_PATH pins the kernel-resolved mount, including stacked and bind mounts.
    // fdinfo predates statx mount IDs and needs no filesystem file-handle support.
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_CLOEXEC)
        .open(path)?;
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", directory.as_raw_fd()))?;
    mount_identity_from_fdinfo(&info)
}

#[cfg(any(target_os = "linux", test))]
fn mount_identity_from_fdinfo(info: &str) -> Result<u64, BoxError> {
    let mut identities = info.lines().filter_map(|line| line.strip_prefix("mnt_id:"));
    let id = identities
        .next()
        .ok_or("mount identity unavailable; cleanup refused")?
        .trim()
        .parse::<u64>()?;
    if id == 0 || identities.next().is_some() {
        return Err("invalid mount identity; cleanup refused".into());
    }
    Ok(id)
}

fn folder_identity(path: &Path) -> Result<(u64, u64), BoxError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err("recovery folder is not a directory".into());
    }
    Ok((metadata.dev(), metadata.ino()))
}

impl Entry {
    /// An active journal owns a registration until explicit recovery or completion.
    /// Match the registration, not the new caller: retries retain the original actor.
    pub(crate) fn find_workspace(
        parent: &Path,
        workspace: &Workspace,
    ) -> Result<Option<(Self, PathBuf)>, BoxError> {
        let entries = match std::fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut found = None;
        for item in entries {
            let item = item?;
            let name = item.file_name();
            let Some(name) = name.to_str().filter(|name| name.starts_with("workspace-")) else {
                continue;
            };
            if legacy_folder(parent, name)? {
                continue;
            }
            let (entry, root) = Self::load(parent, name)?;
            if matches!(entry.phase, Phase::Completed | Phase::Restored) {
                continue;
            }
            if entry
                .workspace
                .as_ref()
                .is_some_and(|ws| ws.id == workspace.id)
            {
                if entry.workspace.as_ref() != Some(workspace) || found.is_some() {
                    return Err("interrupted cleanup registration is ambiguous or changed".into());
                }
                found = Some((entry, root));
            }
        }
        Ok(found)
    }

    /// Locate the original attempt rather than preparing another journal after a crash.
    pub(crate) fn find_audit(
        parent: &Path,
        audit: &crate::cleanup_audit::Audit,
    ) -> Result<Option<(Self, PathBuf)>, BoxError> {
        let entries = match std::fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut found = None;
        for item in entries {
            let item = item?;
            let name = item.file_name();
            let Some(name) = name.to_str().filter(|name| name.starts_with("workspace-")) else {
                continue;
            };
            if legacy_folder(parent, name)? {
                continue;
            }
            let (entry, root) = Self::load(parent, name)?;
            if entry.audit.as_ref() == Some(audit) {
                if found.is_some() {
                    return Err("multiple Trash journals for cleanup audit".into());
                }
                found = Some((entry, root));
            }
        }
        Ok(found)
    }

    /// Prove the rename succeeded, including a crash before saving Moved.
    /// Never create the original path while inspecting an interrupted deletion.
    pub(crate) fn reconcile_moved(&mut self, root: &Path) -> Result<(), BoxError> {
        if !matches!(
            self.phase,
            Phase::Prepared | Phase::Moved | Phase::DatabaseDropped
        ) {
            return Err("cleanup journal requires explicit Trash recovery".into());
        }
        match std::fs::symlink_metadata(&self.original_dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err("original cleanup destination is occupied or unavailable".into()),
        }
        if self.source_identity.is_none()
            || self.source_identity != Some(folder_identity(&root.join("workspace"))?)
        {
            return Err("moved cleanup folder identity changed".into());
        }
        if self.phase == Phase::Prepared {
            self.phase = Phase::Moved;
            self.save(root)?;
        }
        Ok(())
    }

    /// While Prepared is durable, deletion has not touched the database or snapshots.
    /// Only the exact original folder can cancel this preparation. Retain all
    /// partial recovery artifacts for inspection and explicit Empty Trash.
    pub fn cancel_prepared(&mut self, root: &Path) -> Result<(), BoxError> {
        if self.phase != Phase::Prepared || self.source_identity.is_none() {
            return Err("prepared deletion no longer has its original folder".into());
        }
        match std::fs::symlink_metadata(root.join("workspace")) {
            Ok(_) => {
                // A crash can land between the rename and saving Moved. No
                // database or snapshot operation can have begun at this point.
                if self.source_identity != Some(folder_identity(&root.join("workspace"))?) {
                    return Err("prepared Trash folder identity changed".into());
                }
                rename_exclusive(&root.join("workspace"), &self.original_dir)?;
                std::fs::File::open(root)?.sync_all()?;
                std::fs::File::open(
                    self.original_dir
                        .parent()
                        .ok_or("workspace parent missing")?,
                )?
                .sync_all()?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        if self.source_identity != Some(folder_identity(&self.original_dir)?) {
            return Err("prepared workspace folder identity changed".into());
        }
        self.phase = Phase::Restored;
        self.save(root)
    }

    pub fn load(parent: &Path, id: &str) -> Result<(Self, PathBuf), BoxError> {
        if !id.starts_with("workspace-")
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("invalid Trash entry identity".into());
        }
        let root = parent.join(id);
        folder_identity(&root)?;
        let journal = root.join("entry.json");
        let metadata = std::fs::symlink_metadata(&journal)?;
        if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
            return Err("invalid Trash journal".into());
        }
        Ok((
            serde_json::from_reader(std::fs::File::open(journal)?)?,
            root,
        ))
    }

    /// Persist identity before database work or folder publication. Retrying a
    /// rename is allowed only for the exact inode frozen before that rename.
    pub fn begin_recovery(&mut self, root: &Path) -> Result<(), BoxError> {
        if !self.source_repositories.is_empty() {
            let output = cleanup_git_command()?
                .arg("--version")
                .cleanup_output()
                .map_err(|error| {
                    format!("Git is required for worktree recovery; install Git and retry: {error}")
                })?;
            if !output.status.success() {
                return Err(
                    "Git is unavailable; repair the Git installation and retry recovery".into(),
                );
            }
        }
        self.repair_audit(root)?;
        if self.workspace.is_some() && !self.dump_ready {
            return Err("Trash has no committed database recovery copy".into());
        }
        if self.phase == Phase::Restoring && self.recovery.is_some() {
            return Ok(());
        }
        if !matches!(
            self.phase,
            Phase::Completed | Phase::DatabaseDropped | Phase::Moved
        ) {
            return Err("interrupted deletion needs reconciliation before restore".into());
        }
        match std::fs::symlink_metadata(&self.original_dir) {
            Ok(_) => return Err("workspace destination is occupied".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        use std::io::Read;
        let mut token = [0u8; 16];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut token)?;
        let token = token.iter().map(|b| format!("{b:02x}")).collect();
        self.recovery = Some(Recovery {
            operation: token,
            abandoned_staging: Vec::new(),
            folder: folder_identity(&root.join("workspace"))?,
            snapshots: if root.join("snapshots").try_exists()? {
                Some(folder_identity(&root.join("snapshots"))?)
            } else {
                None
            },
        });
        self.phase = Phase::Restoring;
        self.save(root)
    }

    pub fn recover_snapshots(&self, root: &Path, target: &Path) -> Result<(), BoxError> {
        let recovery = self.recovery.as_ref().ok_or("missing recovery identity")?;
        let Some(identity) = recovery.snapshots else {
            return Ok(());
        };
        let source = root.join("snapshots");
        if source.try_exists()? {
            if folder_identity(&source)? != identity {
                return Err("Trash snapshot identity changed".into());
            }
            std::fs::create_dir_all(target.parent().ok_or("snapshot parent missing")?)?;
            rename_exclusive(&source, target)?;
        } else if folder_identity(target)? != identity {
            return Err("snapshot destination occupied".into());
        }
        std::fs::File::open(root)?.sync_all()?;
        std::fs::File::open(target.parent().ok_or("snapshot parent missing")?)?.sync_all()?;
        Ok(())
    }

    pub fn recover_folder(&self, root: &Path) -> Result<(), BoxError> {
        let recovery = self.recovery.as_ref().ok_or("missing recovery identity")?;
        let source = root.join("workspace");
        if source.try_exists()? {
            if folder_identity(&source)? != recovery.folder {
                return Err("Trash folder identity changed".into());
            }
            rename_exclusive(&source, &self.original_dir)?;
        } else if folder_identity(&self.original_dir)? != recovery.folder {
            return Err("workspace destination is occupied".into());
        }
        std::fs::File::open(root)?.sync_all()?;
        std::fs::File::open(
            self.original_dir
                .parent()
                .ok_or("workspace parent missing")?,
        )?
        .sync_all()?;
        self.restore_worktrees(root)
    }

    pub fn save(&self, root: &Path) -> Result<(), BoxError> {
        let mut index = BLOCKED_RESOURCES.lock().unwrap();
        *index = None;
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        serde_json::to_writer(file.as_file_mut(), self)?;
        file.as_file().sync_all()?;
        file.persist(root.join("entry.json"))?;
        std::fs::File::open(root)?.sync_all()?;
        self.repair_audit(root)
    }

    pub(crate) fn repair_audit(&self, root: &Path) -> Result<(), BoxError> {
        repair_audit(root, &self.phase, self.audit.as_ref())
    }

    /// The dump callback must succeed before any move. Its output is synced
    /// before returning, so a committed move never outruns its recovery copy.
    pub fn prepare(
        parent: &Path,
        workspace: &Workspace,
        original_dir: PathBuf,
        dump: impl FnOnce(&Path) -> Result<(), BoxError>,
    ) -> Result<(Self, PathBuf), BoxError> {
        Self::prepare_resource(parent, Some(workspace.clone()), original_dir, dump)
    }

    pub fn prepare_folder(
        parent: &Path,
        original_dir: PathBuf,
    ) -> Result<(Self, PathBuf), BoxError> {
        Self::prepare_resource(parent, None, original_dir, |_| Ok(()))
    }

    fn prepare_resource(
        parent: &Path,
        workspace: Option<Workspace>,
        original_dir: PathBuf,
        dump: impl FnOnce(&Path) -> Result<(), BoxError>,
    ) -> Result<(Self, PathBuf), BoxError> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        let source_identity = match folder_identity(&original_dir) {
            Ok(identity) => Some(identity),
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                None
            }
            Err(e) => return Err(e),
        };
        if source_identity.is_some() {
            preflight_folder_move(&original_dir, parent)?;
        }
        use std::io::Read;
        let mut token = [0u8; 16];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut token)?;
        let deletion_operation = Some(token.iter().map(|b| format!("{b:02x}")).collect());
        let mut entry = Self {
            audit: None,
            deletion_operation,
            source_identity,
            dump_ready: false,
            source_repositories: source_repositories(&original_dir)?,
            workspace,
            original_dir,
            recovery: None,
            phase: Phase::Prepared,
            created_at: chrono::Utc::now(),
        };
        let pending = tempfile::Builder::new()
            .prefix(".preparing-workspace-")
            .tempdir_in(parent)?;
        entry.save(pending.path())?;
        let root = publish_prepared_entry(pending)?;
        std::fs::File::open(parent)?.sync_all()?;
        entry.backup_worktree_records(&root)?;
        dump(&root.join("database.dump"))?;
        match std::fs::symlink_metadata(root.join("database.dump")) {
            Ok(metadata) if metadata.is_file() && metadata.len() > 0 => {
                std::fs::File::open(root.join("database.dump"))?.sync_all()?;
                entry.dump_ready = true;
                entry.save(&root)?;
            }
            Ok(_) => return Err("invalid database recovery copy".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok((entry, root))
    }

    pub fn move_folder(&mut self, root: &Path) -> Result<(), BoxError> {
        if self.phase != Phase::Prepared
            || self.source_identity != Some(folder_identity(&self.original_dir)?)
        {
            return Err("workspace changed after deletion preparation".into());
        }
        rename_exclusive(&self.original_dir, &root.join("workspace"))?;
        // Sync both sides before allowing a database drop.
        std::fs::File::open(root)?.sync_all()?;
        if let Some(parent) = self.original_dir.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        self.phase = Phase::Moved;
        self.save(root)
    }

    /// Preserve indexes, HEADs and reflogs before Git prunes administrative
    /// records. The directory layout is git-worktrees/<repository index>/<id>;
    /// repository indexes refer to the journal's frozen source_repositories.
    fn backup_worktree_records(&self, root: &Path) -> Result<(), BoxError> {
        if self.source_repositories.is_empty() {
            return Ok(());
        }
        let original = self.original_dir.canonicalize()?;
        let backup = root.join("git-worktrees");
        std::fs::create_dir(&backup)?;
        for (index, repository) in self.source_repositories.iter().enumerate() {
            let repository = if repository.is_absolute() {
                repository.clone()
            } else {
                original.join(repository)
            };
            let records = match std::fs::read_dir(repository.join("worktrees")) {
                Ok(records) => records,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            let destination = backup.join(index.to_string());
            std::fs::create_dir(&destination)?;
            for record in records {
                let record = record?;
                if !record.file_type()?.is_dir() {
                    return Err("unexpected Git worktree administrative entry".into());
                }
                let gitdir = std::fs::read(record.path().join("gitdir"))?;
                use std::os::unix::ffi::OsStringExt;
                let gitdir = PathBuf::from(std::ffi::OsString::from_vec(
                    gitdir.strip_suffix(b"\n").unwrap_or(&gitdir).to_vec(),
                ));
                if gitdir.starts_with(&original) {
                    let target = destination.join(record.file_name());
                    copy_git_record(&record.path(), &target)?;
                    backup_index_objects(&record.path(), &target)?;
                }
            }
            std::fs::File::open(&destination)?.sync_all()?;
        }
        // A partial copy must never authorize pruning after recovery.
        std::fs::File::create(backup.join("complete"))?.sync_all()?;
        std::fs::File::open(&backup)?.sync_all()?;
        std::fs::File::open(root)?.sync_all()?;
        Ok(())
    }

    /// Replay administrative records only after the workspace folder has been
    /// returned to its original location, detaching HEAD at the archived commit.
    /// Never reset branches or replace an occupied record. Keep the original
    /// symbolic HEAD in the archive until the user explicitly empties Trash.
    pub fn restore_worktrees(&self, root: &Path) -> Result<(), BoxError> {
        if self.source_repositories.is_empty() {
            return Ok(());
        }
        let backup = root.join("git-worktrees");
        if !std::fs::symlink_metadata(backup.join("complete"))?.is_file() {
            return Err("worktree recovery copy is incomplete".into());
        }
        let mut records = Vec::new();
        for (index, repository) in self.source_repositories.iter().enumerate() {
            let repository = if repository.is_absolute() {
                repository.clone()
            } else {
                self.original_dir.join(repository)
            };
            let directory = backup.join(index.to_string());
            let saved = match std::fs::read_dir(&directory) {
                Ok(saved) => saved,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            for record in saved {
                let record = record?;
                if !record.file_type()?.is_dir() {
                    return Err("invalid worktree recovery record".into());
                }
                let destination = repository.join("worktrees").join(record.file_name());
                // Validate and detach before publication. Git's branch check is
                // not serialized with another checkout, so never replay a symref.
                let staging = tempfile::tempdir()?;
                let staged = staging.path().join("record");
                copy_git_record(&record.path(), &staged)?;
                let commit = std::fs::read(staged.join("lucidos-recovery-head"))
                    .map_err(|_| "missing saved worktree commit; manual recovery required")?;
                let oid = commit.strip_suffix(b"\n").unwrap_or(&commit);
                if !valid_object_id(oid) {
                    return Err("invalid saved worktree commit; manual recovery required".into());
                }
                std::fs::write(staged.join("HEAD"), &commit)?;
                std::fs::File::open(staged.join("HEAD"))?.sync_all()?;
                // Archive metadata must not become live Git administration:
                // a later cleanup needs to create its own saved identity.
                std::fs::remove_file(staged.join("lucidos-recovery-head"))?;
                match std::fs::symlink_metadata(&destination) {
                    Ok(_) => {
                        if same_record(&staged, &destination)? {
                            continue;
                        }
                        return Err("Git worktree administrative record is occupied".into());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
                records.push((repository.clone(), staging, destination));
            }
        }
        for (repository, validated, destination) in records {
            let saved = validated.path().join("record");
            let pack = saved.join("index-objects.pack");
            if pack.try_exists()? {
                let output = cleanup_git_command()?
                    .arg("--git-dir")
                    .arg(&repository)
                    .arg("unpack-objects")
                    .stdin(std::fs::File::open(&pack)?)
                    .cleanup_output()?;
                if !output.status.success() {
                    return Err("could not recover staged Git objects".into());
                }
            }
            let commit = std::fs::read_to_string(saved.join("HEAD"))?;
            let output = cleanup_git_command()?
                .arg("--git-dir")
                .arg(&repository)
                .args(["cat-file", "-t", commit.trim()])
                .cleanup_output()?;
            if !output.status.success() || output.stdout != b"commit\n" {
                return Err(
                    "saved worktree commit is unavailable; manual recovery required".into(),
                );
            }
            let parent = destination.parent().ok_or("missing worktree parent")?;
            std::fs::create_dir_all(parent)?;
            // create_dir is exclusive: a concurrent Git operation wins rather
            // than having its administration replaced by our recovery copy.
            let staging = tempfile::tempdir_in(parent)?;
            let staged = staging.path().join("record");
            copy_git_record(&saved, &staged)?;
            if staged.join("index-objects.pack").try_exists()? {
                std::fs::remove_file(staged.join("index-objects.pack"))?;
            }
            std::fs::File::open(&staged)?.sync_all()?;
            rename_exclusive(&staged, &destination)?;
            std::fs::File::open(parent)?.sync_all()?;
            std::fs::File::open(&repository)?.sync_all()?;
        }
        Ok(())
    }

    /// Git only removes stale administrative records here. Branch refs and
    /// their unique commits remain available for recovery; prepare also saves
    /// the records needed to recover staged changes.
    pub fn prune_worktrees(&self, root: &Path) -> Result<(), BoxError> {
        if self.phase != Phase::DatabaseDropped {
            return Err("worktree pruning requires completed database teardown".into());
        }
        if !self.source_repositories.is_empty()
            && !std::fs::symlink_metadata(root.join("git-worktrees/complete"))?.is_file()
        {
            return Err("worktree recovery copy is incomplete".into());
        }
        for repository in &self.source_repositories {
            let repository = if repository.is_absolute() {
                repository.clone()
            } else {
                root.join("workspace").join(repository)
            };
            let output = cleanup_git_command()?
                .arg("--git-dir")
                .arg(&repository)
                .args(["worktree", "prune", "--expire", "now"])
                .cleanup_output()?;
            if !output.status.success() {
                return Err(format!(
                    "worktree prune failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )
                .into());
            }
        }
        Ok(())
    }
}

/// Preserve the resolved HEAD with its history and every index conflict stage.
/// Gitlinks refer to another repository and are not local blob objects.
fn backup_index_objects(record: &Path, destination: &Path) -> Result<(), BoxError> {
    use std::io::Write;
    let head = cleanup_git_command()?
        .arg("--git-dir")
        .arg(record)
        .args(["rev-parse", "--verify", "HEAD^{commit}"])
        .cleanup_output()?;
    let oid = head.stdout.strip_suffix(b"\n").unwrap_or(&head.stdout);
    if !head.status.success() || !valid_object_id(oid) {
        return Err("could not resolve worktree commit for recovery".into());
    }
    let mut saved_head = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(destination.join("lucidos-recovery-head"))?;
    saved_head.write_all(&head.stdout)?;
    saved_head.sync_all()?;
    let mut objects = std::collections::BTreeSet::from([oid.to_vec()]);
    let output = cleanup_git_command()?
        .arg("--git-dir")
        .arg(record)
        .args(["ls-files", "--stage", "-z"])
        .cleanup_output()?;
    if !output.status.success() {
        return Err("could not read worktree index for recovery".into());
    }
    for entry in output.stdout.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let header = entry
            .split(|b| *b == b'\t')
            .next()
            .ok_or("invalid Git index record")?;
        let mut fields = header.split(|b| *b == b' ');
        let mode = fields.next().ok_or("missing Git index mode")?;
        let oid = fields.next().ok_or("missing Git index object")?;
        if mode == b"160000" || oid.iter().all(|b| *b == b'0') {
            continue;
        }
        if !valid_object_id(oid) {
            return Err("invalid Git index object".into());
        }
        objects.insert(oid.to_vec());
    }
    let pack = std::fs::File::create(destination.join("index-objects.pack"))?;
    let mut input = tempfile::tempfile()?;
    for oid in objects {
        input.write_all(&oid)?;
        input.write_all(b"\n")?;
    }
    use std::io::Seek;
    input.rewind()?;
    let output = cleanup_git_command()?
        .arg("--git-dir")
        .arg(record)
        .args(["pack-objects", "--stdout", "--revs"])
        .stdin(input)
        .cleanup_output_to(pack.try_clone()?)?;
    if !output.status.success() {
        return Err("could not preserve staged Git objects".into());
    }
    pack.sync_all()?;
    std::fs::File::open(destination)?.sync_all()?;
    Ok(())
}

fn valid_object_id(oid: &[u8]) -> bool {
    matches!(oid.len(), 40 | 64) && oid.iter().all(u8::is_ascii_hexdigit)
}

fn same_record(saved: &Path, destination: &Path) -> Result<bool, BoxError> {
    fn files(root: &Path) -> Result<std::collections::BTreeMap<PathBuf, Vec<u8>>, BoxError> {
        fn walk(
            root: &Path,
            at: &Path,
            out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>,
        ) -> Result<(), BoxError> {
            for item in std::fs::read_dir(at)? {
                let item = item?;
                if item.file_name() == "index-objects.pack" {
                    continue;
                }
                let kind = item.file_type()?;
                if kind.is_dir() {
                    walk(root, &item.path(), out)?;
                } else if kind.is_file() {
                    out.insert(
                        item.path().strip_prefix(root)?.to_owned(),
                        std::fs::read(item.path())?,
                    );
                } else {
                    return Err("unsafe Git recovery record".into());
                }
            }
            Ok(())
        }
        folder_identity(root)?;
        let mut out = std::collections::BTreeMap::new();
        walk(root, root, &mut out)?;
        Ok(out)
    }
    Ok(files(saved)? == files(destination)?)
}

fn copy_git_record(source: &Path, destination: &Path) -> Result<(), BoxError> {
    std::fs::create_dir(destination)?;
    for item in std::fs::read_dir(source)? {
        let item = item?;
        let target = destination.join(item.file_name());
        let kind = item.file_type()?;
        if kind.is_dir() {
            copy_git_record(&item.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(item.path(), &target)?;
            std::fs::File::open(target)?.sync_all()?;
        } else {
            return Err(
                "Git worktree recovery records must not contain symlinks or special files".into(),
            );
        }
    }
    std::fs::File::open(destination)?.sync_all()?;
    Ok(())
}

/// Packaged gateways may start without the user's shell PATH.
fn cleanup_git_command() -> Result<std::process::Command, BoxError> {
    let path = resolve_cleanup_git(std::env::var_os("PATH").as_deref())?;
    Ok(std::process::Command::new(path))
}

fn resolve_cleanup_git(path: Option<&std::ffi::OsStr>) -> Result<PathBuf, BoxError> {
    resolve_cleanup_git_in(
        path,
        &[
            "/usr/bin/git",
            "/usr/local/bin/git",
            "/opt/homebrew/bin/git",
            "/bin/git",
        ],
    )
}

fn resolve_cleanup_git_in(
    path: Option<&std::ffi::OsStr>,
    standard: &[&str],
) -> Result<PathBuf, BoxError> {
    use std::os::unix::fs::PermissionsExt;
    standard
        .iter()
        .map(PathBuf::from)
        .chain(
            path.into_iter()
                .flat_map(std::env::split_paths)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join("git")),
        )
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .ok_or_else(|| {
            "Git was not found; install Git to archive or restore workspace worktrees, then retry"
                .into()
        })
}

/// Branches whose linked checkout records will be archived and pruned.
/// Read Git's administrative HEAD rather than infer a ref from the folder name.
pub(crate) fn retained_branches(workspace: &Path) -> Result<Vec<String>, BoxError> {
    use std::os::unix::ffi::OsStringExt;
    let repositories = source_repositories(workspace)?;
    let original = workspace.canonicalize()?;
    let mut branches = std::collections::BTreeSet::new();
    for repository in repositories {
        let repository = if repository.is_absolute() {
            repository
        } else {
            original.join(repository)
        };
        let records = match std::fs::read_dir(repository.join("worktrees")) {
            Ok(records) => records,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        for record in records {
            let record = record?;
            if !record.file_type()?.is_dir() {
                return Err("invalid Git administrative record".into());
            }
            let gitdir = std::fs::read(record.path().join("gitdir"))?;
            let gitdir = PathBuf::from(std::ffi::OsString::from_vec(
                gitdir.strip_suffix(b"\n").unwrap_or(&gitdir).to_vec(),
            ));
            if !gitdir.starts_with(&original) {
                continue;
            }
            let head = std::fs::read_to_string(record.path().join("HEAD"))?;
            if let Some(branch) = head.trim_end_matches('\n').strip_prefix("ref: refs/heads/") {
                branches.insert(branch.to_owned());
            }
        }
    }
    Ok(branches.into_iter().collect())
}

/// Read git's own common-directory identity before moving linked worktrees.
/// Never guess an external repository from a workspace name or branch prefix.
fn source_repositories(workspace: &Path) -> Result<Vec<PathBuf>, BoxError> {
    use std::os::unix::ffi::OsStringExt;
    let mut candidates = vec![workspace.to_owned()];
    let worktrees = workspace.join(".lucidos/worktrees");
    match std::fs::read_dir(&worktrees) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    candidates.push(entry.path());
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut repositories = std::collections::BTreeSet::new();
    for candidate in candidates {
        if !candidate.join(".git").try_exists()? {
            continue;
        }
        let output = cleanup_git_command()?
            .arg("-C")
            .arg(&candidate)
            .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .cleanup_output()?;
        if !output.status.success() {
            return Err(format!(
                "Cannot discover worktree repository; install or repair Git and retry: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let mut bytes = output.stdout;
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        let repository = PathBuf::from(std::ffi::OsString::from_vec(bytes)).canonicalize()?;
        // Internal repositories move into Trash; external ones stay in place.
        // Relative paths preserve the identity across the move.
        let workspace = workspace.canonicalize()?;
        repositories.insert(
            repository
                .strip_prefix(&workspace)
                .map(Path::to_owned)
                .unwrap_or(repository),
        );
    }
    Ok(repositories.into_iter().collect())
}

fn publish_prepared_entry(pending: tempfile::TempDir) -> Result<PathBuf, BoxError> {
    let mut index = BLOCKED_RESOURCES.lock().unwrap();
    *index = None;
    let name = pending
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".preparing-"))
        .ok_or("invalid pending Trash identity")?;
    let parent = pending.path().parent().ok_or("missing Trash parent")?;
    let root = parent.join(name);
    rename_exclusive(pending.path(), &root)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(root)
}

// Legacy deletions used <workspace-id>-YYYYMMDD-HHMMSS without journals.
// Only that explicit shape may bypass modern journal validation.
pub(crate) fn legacy_folder(parent: &Path, name: &str) -> Result<bool, BoxError> {
    let mut parts = name.rsplitn(3, '-');
    let time = parts.next().unwrap_or("");
    let date = parts.next().unwrap_or("");
    let id = parts.next().unwrap_or("");
    Ok(!id.is_empty()
        && date.len() == 8
        && date.bytes().all(|b| b.is_ascii_digit())
        && time.len() == 6
        && time.bytes().all(|b| b.is_ascii_digit())
        && !parent.join(name).join("entry.json").try_exists()?)
}

#[derive(Default)]
struct BlockedResources {
    workspaces: std::collections::HashSet<String>,
    databases: std::collections::HashSet<String>,
}

struct BlockedIndex {
    parent: PathBuf,
    directory: Option<(u64, u64, i64, i64)>,
    resources: BlockedResources,
}

// A gateway exclusively owns its app-data directory. Journals remain authoritative;
// writers serialize cache invalidation with readers, including failed writes.
static BLOCKED_RESOURCES: std::sync::Mutex<Option<BlockedIndex>> = std::sync::Mutex::new(None);

/// Interrupted deletions block engine startup and id reuse until recovered.
/// Malformed journals fail closed rather than recreate an empty database.
pub fn blocks(parent: &Path, id: &str) -> Result<bool, BoxError> {
    use std::os::unix::fs::MetadataExt;
    let mut cache = BLOCKED_RESOURCES.lock().unwrap();
    let directory = match std::fs::metadata(parent) {
        Ok(meta) => Some((meta.dev(), meta.ino(), meta.mtime(), meta.mtime_nsec())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if cache
        .as_ref()
        .is_none_or(|index| index.parent != parent || index.directory != directory)
    {
        *cache = Some(BlockedIndex {
            parent: parent.to_owned(),
            directory,
            resources: scan_blocked_resources(parent)?,
        });
    }
    let resources = &cache.as_ref().unwrap().resources;
    Ok(resources.workspaces.contains(id)
        || resources
            .databases
            .contains(&crate::postgres::database_name(id)?))
}

fn scan_blocked_resources(parent: &Path) -> Result<BlockedResources, BoxError> {
    let mut blocked = BlockedResources::default();
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(blocked),
        Err(e) => return Err(e.into()),
    };
    for item in entries {
        let item = item?;
        let name = item.file_name();
        if let Some(name) = name.to_str() {
            if name.starts_with(".preparing-") {
                continue;
            }
            if legacy_folder(parent, name)? {
                continue;
            }
            if name.starts_with("workspace-") && !item.path().join("entry.json").try_exists()? {
                return Err("modern Trash entry has no journal".into());
            }
        }
        if name
            .to_str()
            .is_some_and(|name| name.starts_with("database-"))
        {
            let (entry, _) =
                DatabaseEntry::load(parent, name.to_str().ok_or("invalid Trash name")?)?;
            if !matches!(entry.phase, Phase::Completed | Phase::Restored) {
                blocked.databases.insert(entry.database);
            }
            continue;
        }
        let path = item.path().join("entry.json");
        // Older deleted folders predate the journal format.
        if !path.try_exists()? {
            continue;
        }
        let entry: Entry = serde_json::from_reader(std::fs::File::open(path)?)?;
        let active = !matches!(entry.phase, Phase::Completed | Phase::Restored);
        if let Some(workspace) = entry.workspace.filter(|_| active) {
            blocked.workspaces.insert(workspace.id);
        }
    }
    Ok(blocked)
}

#[cfg(test)]
mod tests {
    #[test]
    fn descriptor_mount_identity_requires_one_kernel_value() {
        assert_eq!(
            mount_identity_from_fdinfo("pos:\t0\nflags:\t010000000\nmnt_id:\t42\nino:\t100\n")
                .unwrap(),
            42
        );
        for invalid in ["", "mnt_id: invalid", "mnt_id: 0", "mnt_id: 1\nmnt_id: 2"] {
            assert!(mount_identity_from_fdinfo(invalid).is_err());
        }
    }

    #[test]
    fn move_preflight_leaves_source_and_trash_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let trash = root.path().join("trash");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&trash).unwrap();
        std::fs::write(source.join("keep"), "data").unwrap();
        preflight_folder_move(&source, &trash).unwrap();
        assert_eq!(
            std::fs::read_to_string(source.join("keep")).unwrap(),
            "data"
        );
        assert_eq!(std::fs::read_dir(&trash).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    }

    #[test]
    fn blocked_index_tracks_journal_transitions_without_directory_changes() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("deleted");
        assert!(!blocks(&parent, "indexed").unwrap());
        let (mut entry, root) = DatabaseEntry::prepare(&parent, "lucidos_indexed").unwrap();
        assert!(blocks(&parent, "indexed").unwrap());
        assert!(!blocks(&parent, "unrelated").unwrap());
        let before = std::fs::metadata(&parent).unwrap().modified().unwrap();
        entry.phase = Phase::Restored;
        entry.save(&root).unwrap();
        assert_eq!(
            std::fs::metadata(&parent).unwrap().modified().unwrap(),
            before
        );
        assert!(!blocks(&parent, "indexed").unwrap());
        entry.phase = Phase::Moved;
        entry.save(&root).unwrap();
        assert!(blocks(&parent, "indexed").unwrap());
    }

    #[test]
    fn cleanup_git_resolves_without_shell_path_and_refuses_missing_installation() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("git");
        std::fs::write(&executable, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            super::resolve_cleanup_git_in(None, &[executable.to_str().unwrap()]).unwrap(),
            executable
        );
        assert_eq!(
            super::resolve_cleanup_git_in(Some(root.path().as_os_str()), &[]).unwrap(),
            executable
        );
        assert!(super::resolve_cleanup_git_in(None, &[])
            .unwrap_err()
            .to_string()
            .contains("install Git"));
    }

    use super::*;
    #[test]
    fn pending_database_journal_is_invisible_until_atomic_publication() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("deleted");
        std::fs::create_dir(&parent).unwrap();
        let pending = tempfile::Builder::new()
            .prefix(".preparing-database-")
            .tempdir_in(&parent)
            .unwrap();
        assert!(!blocks(&parent, "sample").unwrap());
        assert!(list(temp.path()).unwrap().is_empty());
        let (entry, root) = DatabaseEntry::prepare(&parent, "lucidos_sample").unwrap();
        entry.save(pending.path()).unwrap();
        assert!(!blocks(&parent, "another").unwrap());
        assert_eq!(list(temp.path()).unwrap().len(), 1);
        assert!(root.join("entry.json").is_file());
        assert!(blocks(&parent, "sample").unwrap());
        let published = publish_prepared_entry(pending).unwrap();
        assert!(published
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("database-"));
        assert!(published.join("entry.json").is_file());
    }

    #[test]
    fn failed_repository_observation_leaves_no_unjournalled_entry() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join(".git"), "gitdir: missing-repository\n").unwrap();
        let parent = temp.path().join("deleted");
        assert!(Entry::prepare_folder(&parent, source.clone()).is_err());
        assert!(source.is_dir());
        assert!(!blocks(&parent, "scratch").unwrap());
        assert_eq!(std::fs::read_dir(parent).unwrap().count(), 0);
    }

    #[test]
    fn legacy_prefixed_folders_do_not_poison_journal_scans() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = Workspace::gateway_provisioned("scratch".into(), "Scratch".into(), 9100);
        let audit = crate::cleanup_audit::Audit::begin(
            temp.path(),
            "scratch",
            crate::cleanup_audit::Actor::Device {
                device_id: "test-device".into(),
            },
        )
        .unwrap();
        let parent = temp.path().join("deleted");
        for name in [
            "database-20250101-120000",
            "database-notes-20250101-120000",
            "workspace-2-20250101-120000",
        ] {
            std::fs::create_dir_all(parent.join(name)).unwrap();
        }
        assert!(!blocks(&parent, "scratch").unwrap());
        assert!(Entry::find_workspace(&parent, &workspace)
            .unwrap()
            .is_none());
        assert!(Entry::find_audit(&parent, &audit).unwrap().is_none());
        assert!(DatabaseEntry::find_audit(&parent, &audit)
            .unwrap()
            .is_none());
        std::fs::create_dir(parent.join("database-abcdef")).unwrap();
        assert!(blocks(&parent, "scratch").is_err());
        assert!(DatabaseEntry::find_audit(&parent, &audit).is_err());
        std::fs::remove_dir(parent.join("database-abcdef")).unwrap();
        std::fs::create_dir(parent.join("workspace-abcdef")).unwrap();
        assert!(blocks(&parent, "scratch").is_err());
        assert!(Entry::find_workspace(&parent, &workspace).is_err());
        assert!(Entry::find_audit(&parent, &audit).is_err());
    }

    #[test]
    fn workspace_retry_refuses_changed_or_multiple_active_journals() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let mut workspace =
            Workspace::gateway_provisioned("scratch".into(), "Scratch".into(), 9100);
        workspace.dir = source.to_string_lossy().into_owned();
        let parent = temp.path().join("deleted");
        let (mut entry, root) =
            Entry::prepare(&parent, &workspace, source.clone(), |_| Ok(())).unwrap();
        assert!(Entry::find_workspace(&parent, &workspace)
            .unwrap()
            .is_some());
        let mut changed = workspace.clone();
        changed.name = "Changed".into();
        assert!(Entry::find_workspace(&parent, &changed).is_err());
        let (mut second, second_root) =
            Entry::prepare(&parent, &workspace, source, |_| Ok(())).unwrap();
        assert!(Entry::find_workspace(&parent, &workspace).is_err());
        second.phase = Phase::Restored;
        second.save(&second_root).unwrap();
        assert!(Entry::find_workspace(&parent, &workspace)
            .unwrap()
            .is_some());
        entry.phase = Phase::Completed;
        entry.save(&root).unwrap();
        assert!(Entry::find_workspace(&parent, &workspace)
            .unwrap()
            .is_none());
    }

    #[test]
    fn moved_reconciliation_requires_exact_folder_and_vacant_original() {
        for obstruction in [
            "original",
            "replacement",
            "symlink",
            "missing-identity",
            "restoring",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            std::fs::create_dir(&source).unwrap();
            let (mut entry, root) =
                Entry::prepare_folder(&temp.path().join("deleted"), source.clone()).unwrap();
            rename_exclusive(&source, &root.join("workspace")).unwrap();
            match obstruction {
                "original" => std::fs::create_dir(&source).unwrap(),
                "replacement" => {
                    std::fs::rename(root.join("workspace"), root.join("retained")).unwrap();
                    std::fs::create_dir(root.join("workspace")).unwrap();
                }
                "symlink" => {
                    std::fs::rename(root.join("workspace"), root.join("retained")).unwrap();
                    std::os::unix::fs::symlink(root.join("retained"), root.join("workspace"))
                        .unwrap();
                }
                "missing-identity" => entry.source_identity = None,
                "restoring" => entry.phase = Phase::Restoring,
                _ => unreachable!(),
            }
            assert!(entry.reconcile_moved(&root).is_err(), "{obstruction}");
            assert_ne!(entry.phase, Phase::Completed);
        }
    }

    #[test]
    fn folder_only_journal_restores_without_database_or_registration() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("unregistered");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("retained"), "unique work").unwrap();
        let (mut entry, root) =
            Entry::prepare_folder(&temp.path().join("deleted"), source.clone()).unwrap();
        assert!(entry.workspace.is_none());
        assert!(!entry.dump_ready);
        entry.move_folder(&root).unwrap();
        entry.phase = Phase::DatabaseDropped;
        entry.save(&root).unwrap();
        entry.prune_worktrees(&root).unwrap();
        entry.phase = Phase::Completed;
        entry.save(&root).unwrap();
        assert!(list(temp.path()).unwrap()[0].restore_available);
        entry.begin_recovery(&root).unwrap();
        entry.recover_folder(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(source.join("retained")).unwrap(),
            "unique work"
        );
        assert!(!root.join("database.dump").exists());
    }

    #[test]
    fn prepared_recovery_reverses_move_before_phase_save_without_overwriting() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("retained"), "data").unwrap();
        let (mut entry, root) = Entry::prepare(
            &temp.path().join("trash"),
            &workspace(),
            source.clone(),
            |_| Ok(()),
        )
        .unwrap();
        std::fs::rename(&source, root.join("workspace")).unwrap();
        std::fs::create_dir(&source).unwrap();
        assert!(entry.cancel_prepared(&root).is_err());
        assert!(root.join("workspace/retained").exists());
        std::fs::remove_dir(&source).unwrap();
        entry.cancel_prepared(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(source.join("retained")).unwrap(),
            "data"
        );
        // Simulate the folder publication landing before the journal write.
        entry.phase = Phase::Prepared;
        entry.save(&root).unwrap();
        entry.cancel_prepared(&root).unwrap();
        assert_eq!(entry.phase, Phase::Restored);
    }

    #[test]
    fn prepared_cancellation_retains_partial_dump_and_checks_source_identity() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("trash");
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        assert!(
            Entry::prepare(&parent, &workspace(), source.clone(), |dump| {
                std::fs::write(dump, "partial")?;
                Err("injected dump failure".into())
            })
            .is_err()
        );
        let root = std::fs::read_dir(&parent)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let id = root.file_name().unwrap().to_str().unwrap();
        let (mut entry, _) = Entry::load(&parent, id).unwrap();
        assert!(!entry.dump_ready);
        std::fs::rename(&source, temp.path().join("original")).unwrap();
        std::fs::create_dir(&source).unwrap();
        assert!(entry.cancel_prepared(&root).is_err());
        std::fs::remove_dir(&source).unwrap();
        std::fs::rename(temp.path().join("original"), &source).unwrap();
        entry.cancel_prepared(&root).unwrap();
        assert_eq!(Entry::load(&parent, id).unwrap().0.phase, Phase::Restored);
        assert_eq!(
            std::fs::read_to_string(root.join("database.dump")).unwrap(),
            "partial"
        );
        assert!(!blocks(&parent, "scratch").unwrap());
    }

    #[test]
    fn move_refuses_replaced_source_and_occupied_trash_destination() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let (mut entry, root) = Entry::prepare(
            &temp.path().join("trash"),
            &workspace(),
            source.clone(),
            |_| Ok(()),
        )
        .unwrap();
        std::fs::rename(&source, temp.path().join("original")).unwrap();
        std::fs::create_dir(&source).unwrap();
        assert!(entry.move_folder(&root).is_err());
        std::fs::remove_dir(&source).unwrap();
        std::fs::rename(temp.path().join("original"), &source).unwrap();
        std::fs::create_dir(root.join("workspace")).unwrap();
        assert!(entry.move_folder(&root).is_err());
        assert!(source.is_dir());
        assert_eq!(entry.phase, Phase::Prepared);
    }

    #[test]
    fn completed_restore_refuses_uncommitted_dump() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let (mut entry, root) =
            Entry::prepare(&temp.path().join("trash"), &workspace(), source, |_| Ok(())).unwrap();
        entry.move_folder(&root).unwrap();
        std::fs::write(root.join("database.dump"), "partial").unwrap();
        entry.phase = Phase::Completed;
        entry.save(&root).unwrap();
        assert!(entry.begin_recovery(&root).is_err());
        assert!(entry.recovery.is_none());
    }

    #[test]
    fn recovery_retries_folder_publication_and_retains_dump() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("user-data"), "keep").unwrap();
        let (mut entry, root) = Entry::prepare(
            &temp.path().join("trash"),
            &workspace(),
            source.clone(),
            |dump| {
                std::fs::write(dump, "database")?;
                Ok(())
            },
        )
        .unwrap();
        entry.move_folder(&root).unwrap();
        entry.phase = Phase::Completed;
        entry.save(&root).unwrap();
        entry.begin_recovery(&root).unwrap();
        let operation = entry.recovery.as_ref().unwrap().operation.clone();
        entry.recover_folder(&root).unwrap();
        let mut loaded: Entry =
            serde_json::from_reader(std::fs::File::open(root.join("entry.json")).unwrap()).unwrap();
        loaded.begin_recovery(&root).unwrap();
        assert_eq!(loaded.recovery.as_ref().unwrap().operation, operation);
        loaded.recover_folder(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(source.join("user-data")).unwrap(),
            "keep"
        );
        assert!(root.join("database.dump").exists());
        std::fs::rename(&source, temp.path().join("saved-source")).unwrap();
        std::fs::create_dir(&source).unwrap();
        assert!(loaded.recover_folder(&root).is_err());
    }

    #[test]
    fn recovery_snapshot_publication_is_exclusive_and_retryable() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let (mut entry, root) =
            Entry::prepare(&temp.path().join("trash"), &workspace(), source, |dump| {
                std::fs::write(dump, "database")?;
                Ok(())
            })
            .unwrap();
        entry.move_folder(&root).unwrap();
        std::fs::create_dir(root.join("snapshots")).unwrap();
        std::fs::write(root.join("snapshots/retained"), "snapshot").unwrap();
        entry.phase = Phase::Completed;
        entry.begin_recovery(&root).unwrap();
        let target = temp.path().join("snapshots");
        std::fs::create_dir(&target).unwrap();
        assert!(entry.recover_snapshots(&root, &target).is_err());
        assert!(root.join("snapshots/retained").exists());
        std::fs::remove_dir(&target).unwrap();
        entry.recover_snapshots(&root, &target).unwrap();
        entry.recover_snapshots(&root, &target).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("retained")).unwrap(),
            "snapshot"
        );
    }

    #[test]
    fn recovery_refuses_occupied_destination_before_journaling() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let (mut entry, root) = Entry::prepare(
            &temp.path().join("trash"),
            &workspace(),
            source.clone(),
            |_| Ok(()),
        )
        .unwrap();
        entry.move_folder(&root).unwrap();
        entry.phase = Phase::Completed;
        std::fs::create_dir(&source).unwrap();
        assert!(entry.begin_recovery(&root).is_err());
        assert!(entry.recovery.is_none());
        assert!(root.join("workspace").exists());
        assert!(Entry::load(&temp.path().join("trash"), "../source").is_err());
    }

    #[test]
    fn listing_reports_no_dump_entries_as_files_only() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original");
        std::fs::create_dir(&original).unwrap();
        let workspace = workspace();
        let (mut entry, root) =
            Entry::prepare(&temp.path().join("deleted"), &workspace, original, |_| {
                Ok(())
            })
            .unwrap();
        entry.move_folder(&root).unwrap();
        entry.phase = Phase::Completed;
        entry.save(&root).unwrap();
        let rows = super::list(temp.path()).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].restore_available);
        assert!(rows[0]
            .warning
            .as_deref()
            .unwrap()
            .contains("full workspace restore is unavailable"));
        assert!(entry.begin_recovery(&root).is_err());
        assert!(root.join("workspace").is_dir());
    }

    #[test]
    fn listing_keeps_legacy_and_corrupt_entries_visible_without_exposing_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let trash = temp.path().join("deleted");
        std::fs::create_dir_all(trash.join("legacy")).unwrap();
        std::fs::create_dir(trash.join("broken")).unwrap();
        std::fs::write(trash.join("broken/entry.json"), "invalid").unwrap();
        std::fs::create_dir_all(temp.path().join("deleted-snapshots/legacy")).unwrap();
        std::os::unix::fs::symlink(temp.path(), trash.join("linked")).unwrap();
        let mut ws = workspace();
        ws.database_url = Some("secret".into());
        ws.cleanup.owner_token_digest = Some("private".into());
        let (_, root) =
            Entry::prepare(&trash, &ws, temp.path().join("source"), |_| Ok(())).unwrap();
        let listing = list(temp.path()).unwrap();
        assert_eq!(listing.len(), 5);
        assert!(listing
            .iter()
            .any(|e| e.id == "snapshots:legacy" && e.status == "legacy"));
        assert!(listing
            .iter()
            .any(|e| e.id == "linked" && e.status == "needs-attention" && e.bytes.is_none()));
        assert!(listing.iter().any(|e| e.status == "legacy"));
        assert!(listing.iter().any(|e| e.status == "needs-attention"));
        let prepared = listing
            .iter()
            .find(|e| e.id == root.file_name().unwrap().to_str().unwrap())
            .unwrap();
        assert_eq!(prepared.status, "prepared");
        let json = serde_json::to_string(&listing).unwrap();
        assert!(!json.contains("secret"));
        assert!(!json.contains("private"));
    }
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn external_worktree_prune_preserves_unique_branch_commits() {
        worktree_prune_preserves_unique_branch_commits(false);
    }

    #[test]
    fn internal_worktree_prune_preserves_unique_branch_commits() {
        worktree_prune_preserves_unique_branch_commits(true);
    }

    fn worktree_prune_preserves_unique_branch_commits(internal: bool) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("workspace");
        let worktrees = source.join(".lucidos/worktrees");
        std::fs::create_dir_all(&worktrees).unwrap();
        let repo = if internal {
            source.clone()
        } else {
            temp.path().join("external")
        };
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init"]);
        git(&repo, &["commit", "--allow-empty", "-m", "initial"]);
        let linked = worktrees.join("scratch");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "keep-work",
                linked.to_str().unwrap(),
            ],
        );
        git(&linked, &["commit", "--allow-empty", "-m", "unique work"]);
        assert_eq!(retained_branches(&source).unwrap(), vec!["keep-work"]);
        let before = crate::cleanup::folder_fingerprint(&source, false, 0).unwrap();
        git(&linked, &["checkout", "-b", "renamed-work"]);
        assert_ne!(
            before,
            crate::cleanup::folder_fingerprint(&source, false, 0).unwrap()
        );
        git(&linked, &["checkout", "keep-work"]);
        let tip = git(&repo, &["rev-parse", "keep-work"]);
        std::fs::write(linked.join("staged.txt"), b"staged version").unwrap();
        git(&linked, &["add", "staged.txt"]);
        std::fs::write(linked.join("staged.txt"), b"unstaged version").unwrap();
        let index = std::fs::read(repo.join(".git/worktrees/scratch/index")).unwrap();
        let blob = git(&linked, &["rev-parse", ":staged.txt"]);
        let (mut entry, root) =
            Entry::prepare(&temp.path().join("deleted"), &workspace(), source, |_| {
                Ok(())
            })
            .unwrap();
        assert_eq!(entry.source_repositories.len(), 1);
        entry.move_folder(&root).unwrap();
        assert!(
            entry.prune_worktrees(&root).is_err(),
            "must not prune before database teardown"
        );
        entry.phase = Phase::DatabaseDropped;
        entry.save(&root).unwrap();
        let complete = root.join("git-worktrees/complete");
        std::fs::rename(&complete, root.join("incomplete")).unwrap();
        assert!(entry.prune_worktrees(&root).is_err());
        std::fs::rename(root.join("incomplete"), &complete).unwrap();
        entry.prune_worktrees(&root).unwrap();
        let repo = if internal {
            root.join("workspace")
        } else {
            repo
        };
        assert!(
            !git(&repo, &["worktree", "list", "--porcelain"]).contains(linked.to_str().unwrap())
        );
        assert_eq!(git(&repo, &["rev-parse", "keep-work"]), tip);
        let backup = root.join("git-worktrees/0/scratch");
        assert_eq!(std::fs::read(backup.join("index")).unwrap(), index);
        assert_eq!(
            std::fs::read_to_string(backup.join("HEAD")).unwrap(),
            "ref: refs/heads/keep-work\n"
        );
        assert_eq!(
            std::fs::read(root.join("workspace/.lucidos/worktrees/scratch/staged.txt")).unwrap(),
            b"unstaged version"
        );
        // Replaying the saved administration after returning the folder can
        // recover both versions without checking out or resetting a branch.
        std::fs::rename(root.join("workspace"), &entry.original_dir).unwrap();
        let repo = if internal {
            entry.original_dir.clone()
        } else {
            repo
        };
        let occupied = temp.path().join("occupied");
        git(
            &repo,
            &["worktree", "add", occupied.to_str().unwrap(), "keep-work"],
        );
        git(&occupied, &["commit", "--allow-empty", "-m", "later work"]);
        let advanced = git(&occupied, &["rev-parse", "HEAD"]);
        let saved_head = backup.join("lucidos-recovery-head");
        let saved_commit = std::fs::read(&saved_head).unwrap();
        std::fs::rename(&saved_head, backup.join("missing-head")).unwrap();
        assert!(entry.restore_worktrees(&root).is_err());
        std::fs::rename(backup.join("missing-head"), &saved_head).unwrap();
        for invalid in [b"ref: refs/heads/keep-work\n".as_slice(), blob.as_bytes()] {
            std::fs::write(&saved_head, invalid).unwrap();
            assert!(entry.restore_worktrees(&root).is_err());
            assert!(!repo.join(".git/worktrees/scratch").exists());
        }
        std::fs::write(&saved_head, saved_commit).unwrap();
        assert!(list(temp.path()).unwrap()[0]
            .warning
            .as_ref()
            .unwrap()
            .contains("detached HEADs"));
        assert!(entry.restore_worktrees(&root).is_ok());
        assert_eq!(git(&linked, &["rev-parse", "HEAD"]), tip);
        assert_eq!(
            git(&linked, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "HEAD\n"
        );
        assert_eq!(git(&occupied, &["rev-parse", "HEAD"]), advanced);
        // Exact replay is idempotent; a changed record is never overwritten.
        assert!(entry.restore_worktrees(&root).is_ok());
        std::fs::write(repo.join(".git/worktrees/scratch/HEAD"), "occupied").unwrap();
        assert!(entry.restore_worktrees(&root).is_err());
        std::fs::rename(
            repo.join(".git/worktrees/scratch"),
            temp.path().join("first-replay"),
        )
        .unwrap();
        let blob = blob.trim();
        let loose = repo.join(".git/objects").join(&blob[..2]).join(&blob[2..]);
        // Simulate GC of an object no longer reachable from a live index.
        std::fs::rename(loose, temp.path().join("removed-blob")).unwrap();
        let tip = tip.trim();
        let loose_commit = repo.join(".git/objects").join(&tip[..2]).join(&tip[2..]);
        std::fs::rename(loose_commit, temp.path().join("removed-commit")).unwrap();
        entry.restore_worktrees(&root).unwrap();
        assert_eq!(git(&linked, &["status", "--porcelain"]), "AM staged.txt\n");
        assert_eq!(git(&linked, &["show", ":staged.txt"]), "staged version");
        assert_eq!(git(&repo, &["rev-parse", "keep-work"]), advanced);
        // A restored checkout can be archived again without colliding with
        // recovery-only metadata from the previous deletion.
        Entry::prepare(
            &temp.path().join("deleted-again"),
            &workspace(),
            entry.original_dir.clone(),
            |_| Ok(()),
        )
        .unwrap();
    }

    #[test]
    fn detached_recovery_preserves_conflict_stages() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let source = temp.path().join("workspace");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir_all(source.join(".lucidos/worktrees")).unwrap();
        git(&repo, &["init"]);
        std::fs::write(repo.join("file"), "base\n").unwrap();
        git(&repo, &["add", "file"]);
        git(&repo, &["commit", "-m", "base"]);
        let linked = source.join(".lucidos/worktrees/conflict");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "conflict",
                linked.to_str().unwrap(),
            ],
        );
        git(&linked, &["checkout", "-b", "other"]);
        std::fs::write(linked.join("file"), "other\n").unwrap();
        git(&linked, &["commit", "-am", "other"]);
        git(&linked, &["checkout", "conflict"]);
        std::fs::write(linked.join("file"), "ours\n").unwrap();
        git(&linked, &["commit", "-am", "ours"]);
        let merged = std::process::Command::new("git")
            .current_dir(&linked)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "merge",
                "other",
            ])
            .output()
            .unwrap();
        assert!(!merged.status.success());
        let stages = git(&linked, &["ls-files", "--stage"]);
        assert_eq!(stages.lines().count(), 3);
        let contents = std::fs::read(linked.join("file")).unwrap();
        let head = git(&linked, &["rev-parse", "HEAD"]);
        let (mut entry, root) =
            Entry::prepare(&temp.path().join("trash"), &workspace(), source, |_| Ok(())).unwrap();
        entry.move_folder(&root).unwrap();
        entry.phase = Phase::DatabaseDropped;
        entry.save(&root).unwrap();
        entry.prune_worktrees(&root).unwrap();
        std::fs::rename(root.join("workspace"), &entry.original_dir).unwrap();
        entry.restore_worktrees(&root).unwrap();
        assert_eq!(git(&linked, &["ls-files", "--stage"]), stages);
        assert_eq!(std::fs::read(linked.join("file")).unwrap(), contents);
        assert_eq!(git(&linked, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            git(&linked, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "HEAD\n"
        );
    }

    #[test]
    fn recovery_copy_refuses_symlinks_without_reading_their_targets() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("record");
        std::fs::create_dir(&source).unwrap();
        let target = temp.path().join("private");
        std::fs::write(&target, b"not a Git record").unwrap();
        std::os::unix::fs::symlink(&target, source.join("index")).unwrap();
        let backup = temp.path().join("backup");
        assert!(copy_git_record(&source, &backup).is_err());
        assert!(!backup.join("index").exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"not a Git record");
    }
    fn workspace() -> Workspace {
        serde_json::from_value(
            serde_json::json!({"id":"scratch","name":"Scratch","port":9200,"dir":"scratch"}),
        )
        .unwrap()
    }
    #[test]
    fn failed_dump_keeps_source_and_journal() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        assert!(Entry::prepare(
            &temp.path().join("trash"),
            &workspace(),
            source.clone(),
            |_| Err("dump failed".into())
        )
        .is_err());
        assert!(source.is_dir());
        let journal = std::fs::read_dir(temp.path().join("trash"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join("entry.json");
        let entry: Entry = serde_json::from_reader(std::fs::File::open(journal).unwrap()).unwrap();
        assert_eq!(entry.phase, Phase::Prepared);
        assert!(blocks(&temp.path().join("trash"), "scratch").unwrap());
    }
    #[test]
    fn failed_move_never_advances_to_database_drop() {
        let temp = tempfile::tempdir().unwrap();
        let (mut entry, root) = Entry::prepare(
            &temp.path().join("trash"),
            &workspace(),
            temp.path().join("missing"),
            |_| Ok(()),
        )
        .unwrap();
        assert!(entry.move_folder(&root).is_err());
        assert_eq!(entry.phase, Phase::Prepared);
        assert!(blocks(&temp.path().join("trash"), "scratch").unwrap());
    }
    #[test]
    fn dump_precedes_move_and_journal_survives_reload() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let (mut entry, root) = Entry::prepare(
            &temp.path().join("trash"),
            &workspace(),
            source.clone(),
            |path| {
                assert!(source.exists());
                std::fs::write(path, b"recovery")?;
                Ok(())
            },
        )
        .unwrap();
        entry.move_folder(&root).unwrap();
        assert!(!source.exists());
        assert!(root.join("workspace").is_dir());
        assert_eq!(
            std::fs::read(root.join("database.dump")).unwrap(),
            b"recovery"
        );
        let loaded: Entry =
            serde_json::from_reader(std::fs::File::open(root.join("entry.json")).unwrap()).unwrap();
        assert_eq!(loaded.phase, Phase::Moved);
        assert!(blocks(&temp.path().join("trash"), "scratch").unwrap());
        entry.phase = Phase::Completed;
        entry.save(&root).unwrap();
        assert!(!blocks(&temp.path().join("trash"), "scratch").unwrap());
    }
}

/// A database-only deletion has no folder or registration identity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseEntry {
    #[serde(default)]
    pub(crate) audit: Option<crate::cleanup_audit::Audit>,
    pub database: String,
    pub phase: Phase,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub dump_ready: bool,
    /// None preserves conservative recovery of journals predating this checkpoint.
    /// False proves that no database DROP was attempted by this operation.
    #[serde(default)]
    pub drop_started: Option<bool>,
    pub deletion_operation: String,
    pub restore_operation: Option<String>,
    #[serde(default)]
    pub abandoned_staging: Vec<String>,
}
impl DatabaseEntry {
    /// Find the journal already bound to a frozen member. Never mint a second
    /// deletion attempt after an external effect may have succeeded.
    pub(crate) fn find_audit(
        parent: &Path,
        audit: &crate::cleanup_audit::Audit,
    ) -> Result<Option<(Self, PathBuf)>, BoxError> {
        let entries = match std::fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut found = None;
        for item in entries {
            let item = item?;
            let name = item.file_name();
            let Some(name) = name.to_str().filter(|name| name.starts_with("database-")) else {
                continue;
            };
            if legacy_folder(parent, name)? {
                continue;
            }
            let (entry, root) = Self::load(parent, name)?;
            if entry.audit.as_ref() == Some(audit) {
                if found.is_some() {
                    return Err("multiple Trash journals for cleanup audit".into());
                }
                found = Some((entry, root));
            }
        }
        Ok(found)
    }

    pub(crate) fn finish_dropped(
        &mut self,
        root: &Path,
        database_absent: bool,
    ) -> Result<(), BoxError> {
        if self.phase != Phase::Moved || self.drop_started != Some(true) || !self.dump_ready {
            return Err("interrupted database cleanup requires explicit Trash recovery".into());
        }
        if !database_absent {
            return Err(
                "database still exists; interrupted cleanup requires explicit Trash recovery"
                    .into(),
            );
        }
        self.commit_dump(root)?;
        self.phase = Phase::Completed;
        self.save(root)
    }

    pub fn prepare(parent: &Path, database: &str) -> Result<(Self, PathBuf), BoxError> {
        use std::os::unix::fs::DirBuilderExt;
        crate::postgres::validate_cleanup_database(database)?;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        let pending = tempfile::Builder::new()
            .prefix(".preparing-database-")
            .tempdir_in(parent)?;
        let entry = Self {
            audit: None,
            database: database.into(),
            phase: Phase::Prepared,
            created_at: chrono::Utc::now(),
            dump_ready: false,
            drop_started: Some(false),
            deletion_operation: database_operation()?,
            restore_operation: None,
            abandoned_staging: Vec::new(),
        };
        entry.save(pending.path())?;
        let root = publish_prepared_entry(pending)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok((entry, root))
    }
    pub fn can_cancel_without_database(&self) -> bool {
        self.phase == Phase::Prepared || self.drop_started == Some(false)
    }
    pub fn load(parent: &Path, id: &str) -> Result<(Self, PathBuf), BoxError> {
        if !id.starts_with("database-")
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("invalid database Trash identity".into());
        }
        let root = parent.join(id);
        folder_identity(&root)?;
        let journal = root.join("entry.json");
        let metadata = std::fs::symlink_metadata(&journal)?;
        if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
            return Err("invalid database Trash journal".into());
        }
        let entry: Self = serde_json::from_reader(std::fs::File::open(journal)?)?;
        crate::postgres::validate_cleanup_database(&entry.database)?;
        Ok((entry, root))
    }
    pub fn save(&self, root: &Path) -> Result<(), BoxError> {
        let mut index = BLOCKED_RESOURCES.lock().unwrap();
        *index = None;
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        serde_json::to_writer(file.as_file_mut(), self)?;
        file.as_file().sync_all()?;
        file.persist(root.join("entry.json"))?;
        std::fs::File::open(root)?.sync_all()?;
        self.repair_audit(root)
    }
    pub(crate) fn repair_audit(&self, root: &Path) -> Result<(), BoxError> {
        repair_audit(root, &self.phase, self.audit.as_ref())
    }
    pub fn commit_dump(&mut self, root: &Path) -> Result<(), BoxError> {
        let dump = root.join("database.dump");
        let metadata = std::fs::symlink_metadata(&dump)?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err("database recovery copy is unavailable".into());
        }
        std::fs::File::open(dump)?.sync_all()?;
        self.dump_ready = true;
        self.save(root)
    }
    pub fn begin_restore(&mut self, root: &Path) -> Result<(), BoxError> {
        self.repair_audit(root)?;
        if !self.dump_ready || self.phase == Phase::Restored {
            return Err("database recovery copy is not committed or already restored".into());
        }
        let metadata = std::fs::symlink_metadata(root.join("database.dump"))?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err("database recovery copy is unavailable".into());
        }
        if self.restore_operation.is_none() {
            self.restore_operation = Some(database_operation()?);
        }
        self.phase = Phase::Restoring;
        self.save(root)
    }
}
fn database_operation() -> Result<String, BoxError> {
    use std::io::Read;
    let mut token = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut token)?;
    Ok(token.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod database_journal_tests {
    use super::*;
    #[test]
    fn database_journal_blocks_only_matching_workspace_reuse() {
        let temp = tempfile::tempdir().unwrap();
        let (mut entry, root) =
            DatabaseEntry::prepare(temp.path(), "lucidos_unregistered").unwrap();
        assert!(!blocks(temp.path(), "unrelated").unwrap());
        assert!(blocks(temp.path(), "unregistered").unwrap());
        entry.phase = Phase::Restored;
        entry.save(&root).unwrap();
        assert!(!blocks(temp.path(), "unregistered").unwrap());
    }

    #[test]
    fn database_journal_listing_preserves_incomplete_entries() {
        let temp = tempfile::tempdir().unwrap();
        let (mut entry, root) =
            DatabaseEntry::prepare(&temp.path().join("deleted"), "lucidos_unregistered").unwrap();
        let rows = list(temp.path()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "lucidos_unregistered");
        assert_eq!(rows[0].status, "prepared");
        assert!(rows[0].restore_available);
        entry.phase = Phase::Completed;
        entry.save(&root).unwrap();
        assert!(!list(temp.path()).unwrap()[0].restore_available);
        std::fs::write(root.join("entry.json"), b"broken").unwrap();
        assert_eq!(list(temp.path()).unwrap()[0].status, "needs-attention");
    }

    #[test]
    fn database_journal_requires_committed_dump_and_keeps_exact_name() {
        let temp = tempfile::tempdir().unwrap();
        let (mut entry, root) = DatabaseEntry::prepare(temp.path(), "lucidos_Legacy-name").unwrap();
        assert!(entry.begin_restore(&root).is_err());
        assert!(entry.commit_dump(&root).is_err());
        std::fs::write(root.join("database.dump"), b"recovery").unwrap();
        entry.commit_dump(&root).unwrap();
        entry.begin_restore(&root).unwrap();
        let operation = entry.restore_operation.clone();
        entry.begin_restore(&root).unwrap();
        assert_eq!(entry.restore_operation, operation);
        let (loaded, _) =
            DatabaseEntry::load(temp.path(), root.file_name().unwrap().to_str().unwrap()).unwrap();
        assert_eq!(loaded.database, "lucidos_Legacy-name");
        assert_eq!(loaded.phase, Phase::Restoring);
        assert!(DatabaseEntry::load(temp.path(), "../outside").is_err());
        assert!(DatabaseEntry::prepare(temp.path(), "postgres").is_err());
    }
}

/// A terminal journal is proof of successful effects. Recreate its immutable
/// completion before its last recovery copy can be purged. Nonterminal journals
/// never claim success; their external effects require explicit reconciliation.
fn repair_audit(
    root: &Path,
    phase: &Phase,
    audit: Option<&crate::cleanup_audit::Audit>,
) -> Result<(), BoxError> {
    if *phase == Phase::Completed {
        if let Some(audit) = audit {
            let home = root
                .parent()
                .and_then(Path::parent)
                .ok_or("missing Trash home")?;
            audit.completed(home)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod database_completion_tests {
    use super::*;

    #[test]
    fn recovery_requires_proven_drop_attempt_absence_and_durable_dump() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("deleted");
        let audit = crate::cleanup_audit::Audit::begin(
            temp.path(),
            "database:lucidos_scratch",
            crate::cleanup_audit::Actor::Device {
                device_id: "device".into(),
            },
        )
        .unwrap();
        let (mut entry, root) = DatabaseEntry::prepare(&parent, "lucidos_scratch").unwrap();
        entry.audit = Some(audit.clone());
        entry.save(&root).unwrap();
        assert!(!audit.is_completed(temp.path()).unwrap());
        assert!(entry.finish_dropped(&root, true).is_err());
        std::fs::write(root.join("database.dump"), b"dump").unwrap();
        entry.commit_dump(&root).unwrap();
        entry.phase = Phase::Moved;
        for started in [None, Some(false)] {
            entry.drop_started = started;
            assert!(entry.finish_dropped(&root, true).is_err());
        }
        entry.drop_started = Some(true);
        entry.save(&root).unwrap();
        assert!(entry.finish_dropped(&root, false).is_err());
        assert!(!audit.is_completed(temp.path()).unwrap());
        let (mut recovered, recovered_root) =
            DatabaseEntry::find_audit(&parent, &audit).unwrap().unwrap();
        assert_eq!(recovered_root, root);
        std::fs::remove_file(root.join("database.dump")).unwrap();
        assert!(recovered.finish_dropped(&root, true).is_err());
        assert!(!audit.is_completed(temp.path()).unwrap());
        std::fs::write(root.join("database.dump"), b"dump").unwrap();
        recovered.finish_dropped(&root, true).unwrap();
        assert!(audit.is_completed(temp.path()).unwrap());
        assert_eq!(
            DatabaseEntry::load(&parent, root.file_name().unwrap().to_str().unwrap())
                .unwrap()
                .0
                .phase,
            Phase::Completed
        );
        let (mut duplicate, _) = DatabaseEntry::prepare(&parent, "lucidos_scratch").unwrap();
        duplicate.audit = Some(audit.clone());
        // Persist a conflicting second journal without repairing a completion.
        let second = std::fs::read_dir(&parent)
            .unwrap()
            .map(|item| item.unwrap().path())
            .find(|path| path != &root)
            .unwrap();
        duplicate.save(&second).unwrap();
        assert!(DatabaseEntry::find_audit(&parent, &audit).is_err());
    }
}
