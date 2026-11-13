//! Read-only cleanup discovery. Uncertain observations never authorize deletion.

use crate::registry::Workspace;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Read peer files strictly: Registry::load's first-run default is unsafe when
/// deciding whether a database belongs to another install.
pub(crate) struct RegisteredResources {
    pub database_names: BTreeSet<String>,
    pub directories: Vec<PathBuf>,
}

impl RegisteredResources {
    /// The caller excludes the selected registration, so any remaining owner
    /// protects the resource even when two installs reuse the same workspace id.
    pub fn require_unowned(&self, directory: &Path, database: &str) -> Result<(), BoxError> {
        self.require_unowned_database(database)?;
        self.require_unowned_directory(directory)
    }

    /// Database ownership comes from registry ids, never directory basenames.
    /// A database-only candidate does not need a fabricated directory.
    pub fn require_unowned_database(&self, database: &str) -> Result<(), BoxError> {
        if self.database_names.contains(database) {
            return Err("database is registered by another workspace or install".into());
        }
        Ok(())
    }

    /// A recovery target is absent; resolve its parent without following a
    /// replacement at the final component. Existing targets still fail the
    /// exclusive publication in Trash recovery.
    pub fn require_unowned_destination(&self, directory: &Path) -> Result<(), BoxError> {
        let directory = directory
            .parent()
            .ok_or("missing recovery parent")?
            .canonicalize()?
            .join(directory.file_name().ok_or("missing recovery name")?);
        for registered in &self.directories {
            let registered = reserved_directory(registered)?;
            if registered.starts_with(&directory) || directory.starts_with(&registered) {
                return Err("recovery folder overlaps a registration".into());
            }
        }
        Ok(())
    }

    /// A folder-only candidate does not imply ownership of any database.
    /// Missing registered folders still reserve their resolved location.
    pub fn require_unowned_directory(&self, directory: &Path) -> Result<(), BoxError> {
        let directory = std::fs::canonicalize(directory)?;
        for registered in &self.directories {
            let registered = reserved_directory(registered)?;
            if registered.starts_with(&directory) || directory.starts_with(&registered) {
                return Err("workspace folder overlaps another registration".into());
            }
        }
        Ok(())
    }
}

// Preserve missing registrations' path claims without blocking unrelated resources.
// Existing symlinks must resolve; a dangling link is not proof of an absent folder.
fn reserved_directory(path: &Path) -> Result<PathBuf, BoxError> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::symlink_metadata(path) {
                Err(missing) if missing.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(error.into()),
            }
            let parent = path
                .parent()
                .ok_or("registered directory has no existing ancestor")?;
            Ok(reserved_directory(parent)?
                .join(path.file_name().ok_or("invalid registered directory")?))
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn registered_resources(
    own: &[Workspace],
    app_data: &Path,
    peers: &[PathBuf],
) -> Result<RegisteredResources, BoxError> {
    let mut ids: BTreeSet<String> = own.iter().map(|w| w.id.clone()).collect();
    let mut directories: Vec<_> = own.iter().map(|w| w.resolve_dir(app_data)).collect();
    for path in peers {
        // Registry's defaulted workspaces field is useful at first run but
        // cannot prove ownership when a peer document is incomplete.
        #[derive(Deserialize)]
        struct PeerRegistry {
            workspaces: Vec<Workspace>,
        }
        let registry: PeerRegistry = serde_json::from_slice(&std::fs::read(path)?)?;
        let peer_data = path
            .parent()
            .and_then(Path::parent)
            .ok_or("peer registry has no app-data directory")?;
        directories.extend(registry.workspaces.iter().map(|w| w.resolve_dir(peer_data)));
        ids.extend(registry.workspaces.into_iter().map(|w| w.id));
    }
    let database_names = ids
        .into_iter()
        .map(|id| crate::postgres::database_name(&id))
        .collect::<Result<_, _>>()?;
    Ok(RegisteredResources {
        database_names,
        directories,
    })
}

fn unregistered_siblings(
    own: &[PathBuf],
    registered: &[PathBuf],
) -> Result<Vec<PathBuf>, BoxError> {
    let protected: BTreeSet<_> = registered
        .iter()
        .map(|path| reserved_directory(path))
        .collect::<Result<_, _>>()?;
    Ok(sibling_workspaces(own)?
        .into_iter()
        .filter(|dir| {
            !protected
                .iter()
                .any(|registered| dir.starts_with(registered) || registered.starts_with(dir))
        })
        .collect())
}

fn sibling_workspaces(registered: &[PathBuf]) -> Result<Vec<PathBuf>, BoxError> {
    let registered: BTreeSet<_> = registered
        .iter()
        .map(std::fs::canonicalize)
        .collect::<Result<_, _>>()?;
    let parents: BTreeSet<_> = registered.iter().filter_map(|p| p.parent()).collect();
    let mut found = BTreeSet::new();
    for parent in parents {
        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir()
                && !registered.contains(&path)
                && path.join(".lucidos").is_dir()
                && path.join("data").is_dir()
            {
                found.insert(path);
            }
        }
    }
    Ok(found.into_iter().collect())
}

/// Allocated blocks, counting hard links once and never following symlinks.
pub(crate) fn allocated_bytes(root: &Path) -> Result<u64, BoxError> {
    allocated_bytes_many([root])
}

/// A selection may contain hard links across different Trash entries.
pub(crate) fn allocated_bytes_many<'a>(
    roots: impl IntoIterator<Item = &'a Path>,
) -> Result<u64, BoxError> {
    fn walk(path: &Path, seen: &mut BTreeSet<(u64, u64)>) -> Result<u64, BoxError> {
        let meta = std::fs::symlink_metadata(path)?;
        if !seen.insert((meta.dev(), meta.ino())) {
            return Ok(0);
        }
        let mut bytes = meta.blocks().saturating_mul(512);
        if meta.is_dir() {
            for entry in std::fs::read_dir(path)? {
                bytes = bytes.saturating_add(walk(&entry?.path(), seen)?);
            }
        }
        Ok(bytes)
    }
    let mut seen = BTreeSet::new();
    roots.into_iter().try_fold(0u64, |bytes, root| {
        bytes
            .checked_add(walk(root, &mut seen)?)
            .ok_or_else(|| "allocation size overflow".into())
    })
}

#[derive(Clone, Default, Serialize)]
pub struct Review {
    pub scanned_at: Option<chrono::DateTime<chrono::Utc>>,
    pub refreshing: bool,
    pub rows: Vec<ReviewRow>,
    pub searched_parents: Vec<PathBuf>,
    pub registered_databases: Option<BTreeSet<String>>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Serialize)]
pub struct ReviewRow {
    pub id: String,
    pub workspace: Option<Workspace>,
    pub directory: PathBuf,
    pub state: &'static str,
    pub folder_bytes: Option<u64>,
    pub folder_modified: Option<chrono::DateTime<chrono::Utc>>,
    pub warning: Option<String>,
    pub database: Option<DatabaseObservation>,
    pub running: Option<bool>,
    pub fingerprint: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DatabaseObservation {
    pub name: String,
    pub bytes: u64,
    pub last_event: Option<chrono::DateTime<chrono::Utc>>,
    pub warning: Option<String>,
}

pub fn database_fingerprint(database: &DatabaseObservation) -> Option<String> {
    if database.warning.is_some() {
        return None;
    }
    serde_json::to_string(database)
        .ok()
        .map(|json| crate::auth::digest(&json))
}

pub fn folder_fingerprint(directory: &Path, running: bool, bytes: u64) -> Result<String, BoxError> {
    let metadata = std::fs::symlink_metadata(directory)?;
    if !metadata.is_dir() {
        return Err("workspace folder is not a directory".into());
    }
    Ok(crate::auth::digest(&serde_json::to_string(&(
        directory,
        metadata.dev(),
        metadata.ino(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
        running,
        bytes,
        crate::trash::retained_branches(directory)?,
    ))?))
}

pub fn fingerprint(
    ws: &Workspace,
    dir: &Path,
    last_event: Option<chrono::DateTime<chrono::Utc>>,
    running: bool,
) -> Result<String, BoxError> {
    let metadata = std::fs::symlink_metadata(dir)?;
    let modified: chrono::DateTime<chrono::Utc> = metadata.modified()?.into();
    Ok(crate::auth::digest(&serde_json::to_string(&(
        ws,
        dir,
        metadata.dev(),
        metadata.ino(),
        modified,
        last_event,
        running,
        crate::trash::retained_branches(dir)?,
    ))?))
}

/// A registered workspace needs both resources observed successfully. Bind the
/// full database observation and allocated folder bytes to the frozen selection.
pub fn observed_workspace_fingerprint(
    ws: &Workspace,
    dir: &Path,
    database: Option<&DatabaseObservation>,
    running: Option<bool>,
    folder_bytes: Option<u64>,
) -> Option<String> {
    let database = database?;
    let database = database_fingerprint(database)?;
    let folder = fingerprint(ws, dir, None, running?).ok()?;
    let bytes = folder_bytes?;
    Some(crate::auth::digest(
        &serde_json::to_string(&(folder, database, bytes)).ok()?,
    ))
}

pub fn merge_databases(report: &mut Review, databases: Vec<DatabaseObservation>) {
    for database in databases {
        if let Some(row) = report.rows.iter_mut().find(|row| {
            row.workspace
                .as_ref()
                .and_then(|ws| crate::postgres::database_name(&ws.id).ok())
                .as_deref()
                == Some(&database.name)
        }) {
            row.database = Some(database);
        } else if database.name.starts_with("lucidos_restore_") {
            report.warnings.push(format!(
                "{} is reserved for Trash recovery; inspect recovery before reclaiming it",
                database.name
            ));
        } else if report
            .registered_databases
            .as_ref()
            .is_some_and(|names| !names.contains(&database.name))
        {
            report.rows.push(ReviewRow {
                id: format!("database:{}", database.name),
                workspace: None,
                directory: PathBuf::new(),
                state: "database-only",
                folder_bytes: None,
                folder_modified: None,
                warning: None,
                running: None,
                fingerprint: database_fingerprint(&database),
                database: Some(database),
            });
        }
    }
}

#[derive(Default)]
pub struct ReviewCache {
    pub report: Review,
    pub running: bool,
    pub generation: u64,
    pub mutations: usize,
}

impl ReviewCache {
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.report = Review::default();
    }

    pub fn publish(&mut self, generation: u64, mut report: Review) {
        if self.generation == generation && self.mutations == 0 {
            report.scanned_at = Some(chrono::Utc::now());
            self.report = report;
        }
        self.running = false;
    }
}

/// This entire scan runs on a blocking gateway job, outside the request path.
pub fn scan(own: Vec<Workspace>, app_data: &Path, peers: Vec<PathBuf>) -> Review {
    let dirs: Vec<_> = own.iter().map(|w| w.resolve_dir(app_data)).collect();
    let mut report = Review {
        scanned_at: Some(chrono::Utc::now()),
        searched_parents: dirs
            .iter()
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        ..Review::default()
    };
    let registrations = registered_resources(&own, app_data, &peers);
    match &registrations {
        Ok(resources) => report.registered_databases = Some(resources.database_names.clone()),
        Err(e) => report
            .warnings
            .push(format!("Orphan discovery unavailable: {e}")),
    }
    for workspace in own {
        let dir = workspace.resolve_dir(app_data);
        report.rows.push(row(Some(workspace), dir, "registered"));
    }
    let siblings =
        registrations.and_then(|resources| unregistered_siblings(&dirs, &resources.directories));
    match siblings {
        Ok(siblings) => report.rows.extend(
            siblings
                .into_iter()
                .map(|dir| row(None, dir, "not-registered")),
        ),
        Err(e) => report
            .warnings
            .push(format!("Sibling discovery incomplete: {e}")),
    }
    report
}

pub(crate) fn folder_id(directory: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    format!(
        "folder:{}",
        crate::auth::digest(
            &directory
                .as_os_str()
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
    )
}

fn row(workspace: Option<Workspace>, directory: PathBuf, state: &'static str) -> ReviewRow {
    let id = workspace
        .as_ref()
        .map(|ws| ws.id.clone())
        .unwrap_or_else(|| folder_id(&directory));
    let running =
        lucidos_owner_lock::is_held(&lucidos_owner_lock::engine::lock_path(&directory)).ok();
    let mut fingerprint = workspace
        .as_ref()
        .and_then(|ws| running.and_then(|running| fingerprint(ws, &directory, None, running).ok()));
    let workspace = workspace.map(|mut workspace| {
        workspace.database_url = None;
        workspace.cleanup.owner_token_digest = None;
        workspace
    });
    let (folder_bytes, warning) = match allocated_bytes(&directory) {
        Ok(bytes) => (Some(bytes), None),
        Err(e) => (None, Some(e.to_string())),
    };
    if workspace.is_none() {
        fingerprint = running
            .zip(folder_bytes)
            .and_then(|(running, bytes)| folder_fingerprint(&directory, running, bytes).ok());
    }
    let folder_modified = std::fs::symlink_metadata(&directory)
        .and_then(|m| m.modified())
        .ok()
        .map(Into::into);
    ReviewRow {
        id,
        workspace,
        directory,
        state,
        folder_bytes,
        folder_modified,
        warning,
        database: None,
        running,
        fingerprint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;

    #[test]
    fn absent_registration_reserves_only_its_own_location() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing/nested");
        let other = root.path().join("other");
        std::fs::create_dir(&other).unwrap();
        let resources = RegisteredResources {
            database_names: Default::default(),
            directories: vec![missing.clone()],
        };
        resources.require_unowned_directory(&other).unwrap();
        resources
            .require_unowned_destination(&root.path().join("restore"))
            .unwrap();
        assert!(resources.require_unowned_directory(root.path()).is_err());
        assert!(resources
            .require_unowned_destination(&root.path().join("missing"))
            .is_err());
    }

    #[test]
    fn invalidation_rejects_a_scan_started_before_resource_mutation() {
        let mut cache = ReviewCache::default();
        let generation = cache.generation;
        cache.running = true;
        cache.invalidate();
        cache.publish(
            generation,
            Review {
                warnings: vec!["stale resource".into()],
                ..Review::default()
            },
        );
        assert!(cache.report.scanned_at.is_none());
        assert!(cache.report.warnings.is_empty());
        assert!(!cache.running);
    }

    #[test]
    fn scan_ttl_starts_at_publication_after_a_slow_scan() {
        let mut cache = ReviewCache::default();
        let started = chrono::Utc::now() - chrono::Duration::minutes(5);
        cache.publish(
            cache.generation,
            Review {
                scanned_at: Some(started),
                ..Review::default()
            },
        );
        assert!(cache.report.scanned_at.unwrap() > started + chrono::Duration::minutes(4));
    }

    #[test]
    fn restore_staging_is_never_an_orphan_candidate() {
        let mut report = Review {
            registered_databases: Some(BTreeSet::new()),
            ..Review::default()
        };
        merge_databases(
            &mut report,
            vec![DatabaseObservation {
                name: "lucidos_restore_0123456789abcdef0123456789abcdef".into(),
                bytes: 4096,
                last_event: None,
                warning: None,
            }],
        );
        assert!(
            report.rows.is_empty(),
            "recovery staging must never be deletable as an orphan"
        );
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("recovery")));
    }

    #[test]
    fn discovered_resources_have_stable_selection_ids_and_change_sensitive_fingerprints() {
        let temp = tempfile::tempdir().unwrap();
        let first = row(None, temp.path().to_owned(), "not-registered");
        let second = row(None, temp.path().to_owned(), "not-registered");
        assert_eq!(first.id, second.id);
        assert!(first.id.starts_with("folder:"));
        assert!(!first.id.contains(temp.path().to_str().unwrap()));
        assert!(first.fingerprint.is_some());
        assert_eq!(first.fingerprint, second.fingerprint);
        std::fs::write(temp.path().join("new-file"), "changed").unwrap();
        assert_ne!(
            first.fingerprint,
            row(None, temp.path().to_owned(), "not-registered").fingerprint
        );
        let database = DatabaseObservation {
            name: "lucidos_orphan".into(),
            bytes: 4096,
            last_event: None,
            warning: None,
        };
        let mut report = Review {
            registered_databases: Some(BTreeSet::new()),
            ..Review::default()
        };
        merge_databases(&mut report, vec![database.clone()]);
        assert_eq!(report.rows[0].id, "database:lucidos_orphan");
        let fingerprint = report.rows[0].fingerprint.clone();
        assert!(fingerprint.is_some());
        let mut changed = database.clone();
        changed.bytes += 4096;
        report.rows.clear();
        merge_databases(&mut report, vec![changed]);
        assert_ne!(fingerprint, report.rows[0].fingerprint);
        let mut unavailable = database;
        unavailable.warning = Some("activity unavailable".into());
        report.rows.clear();
        merge_databases(&mut report, vec![unavailable]);
        assert!(report.rows[0].fingerprint.is_none());
    }

    #[test]
    fn registered_fingerprint_requires_complete_observations() {
        let root = tempfile::tempdir().unwrap();
        let ws = Workspace::gateway_provisioned("sample".into(), "Sample".into(), 1234);
        let database = DatabaseObservation {
            name: "lucidos_sample".into(),
            bytes: 4096,
            last_event: None,
            warning: None,
        };
        let bytes = allocated_bytes(root.path()).unwrap();
        let observed =
            |db, size| observed_workspace_fingerprint(&ws, root.path(), db, Some(false), size);
        assert!(observed(None, Some(bytes)).is_none());
        assert!(observed(Some(&database), None).is_none());
        let first = observed(Some(&database), Some(bytes)).unwrap();
        let changed = DatabaseObservation {
            bytes: database.bytes + 4096,
            ..database.clone()
        };
        assert_ne!(Some(first), observed(Some(&changed), Some(bytes)));
        let unavailable = DatabaseObservation {
            warning: Some("activity unavailable".into()),
            ..database.clone()
        };
        assert!(observed(Some(&unavailable), Some(bytes)).is_none());
    }

    #[test]
    fn fingerprint_changes_when_engine_starts_or_registry_changes() {
        let root = tempfile::tempdir().unwrap();
        let ws = Workspace::gateway_provisioned("sample".into(), "Sample".into(), 1234);
        let first = fingerprint(&ws, root.path(), None, false).unwrap();
        assert_ne!(first, fingerprint(&ws, root.path(), None, true).unwrap());
        let mut renamed = ws;
        renamed.name = "Renamed".into();
        assert_ne!(
            first,
            fingerprint(&renamed, root.path(), None, false).unwrap()
        );
    }

    #[test]
    fn unreadable_peer_disables_orphan_rows() {
        let root = tempfile::tempdir().unwrap();
        let mut report = scan(vec![], root.path(), vec![root.path().join("missing")]);
        merge_databases(
            &mut report,
            vec![DatabaseObservation {
                name: "lucidos_sample".into(),
                bytes: 1024,
                last_event: None,
                warning: None,
            }],
        );
        assert!(report.rows.is_empty());
        assert!(!report.warnings.is_empty());
    }

    #[test]
    fn review_omits_database_credentials_and_owner_digest() {
        let root = tempfile::tempdir().unwrap();
        let mut workspace = Workspace::gateway_provisioned("sample".into(), "Sample".into(), 1234);
        workspace.database_url = Some("secret-database-url".into());
        workspace.cleanup.owner_token_digest = Some("secret-owner-digest".into());
        let serialized =
            serde_json::to_string(&row(Some(workspace), root.path().into(), "registered")).unwrap();
        assert!(!serialized.contains("secret-database-url"));
        assert!(!serialized.contains("secret-owner-digest"));
    }

    #[test]
    fn orphan_discovery_requires_every_peer_registry() {
        let root = tempfile::tempdir().unwrap();
        let peer = root.path().join("peer.json");
        std::fs::write(&peer, r#"{"workspaces":[]}"#).unwrap();
        assert!(registered_resources(&[], root.path(), std::slice::from_ref(&peer)).is_ok());
        std::fs::write(&peer, "{}").unwrap();
        assert!(registered_resources(&[], root.path(), std::slice::from_ref(&peer)).is_err());
        std::fs::write(&peer, "broken").unwrap();
        assert!(registered_resources(&[], root.path(), std::slice::from_ref(&peer)).is_err());
        std::fs::remove_file(&peer).unwrap();
        assert!(registered_resources(&[], root.path(), &[peer]).is_err());
    }

    #[test]
    fn individual_resources_require_no_inferred_folder_database_pair() {
        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("folder-only");
        std::fs::create_dir(&selected).unwrap();
        let resources = RegisteredResources {
            database_names: BTreeSet::from(["lucidos_owned".into()]),
            directories: vec![root.path().join("unavailable-peer-folder")],
        };
        assert!(resources.require_unowned_database("lucidos_owned").is_err());
        assert!(resources.require_unowned_database("lucidos_orphan").is_ok());
        assert!(resources.require_unowned_directory(&selected).is_ok());
        let resources = RegisteredResources {
            database_names: BTreeSet::from(["lucidos_owned".into()]),
            directories: vec![],
        };
        assert!(resources.require_unowned_directory(&selected).is_ok());
        assert!(resources
            .require_unowned_directory(&root.path().join("missing"))
            .is_err());
    }

    #[test]
    fn resource_ownership_rejects_shared_databases_aliases_and_nested_folders() {
        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("selected");
        let other = root.path().join("other");
        std::fs::create_dir_all(selected.join("nested")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let mut resources = RegisteredResources {
            database_names: BTreeSet::from(["lucidos_shared".into()]),
            directories: vec![other.clone()],
        };
        assert!(resources
            .require_unowned(&selected, "lucidos_shared")
            .is_err());
        assert!(resources
            .require_unowned(&selected, "lucidos_selected")
            .is_ok());
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&selected, &alias).unwrap();
        resources.directories = vec![alias];
        assert!(resources
            .require_unowned(&selected, "lucidos_selected")
            .is_err());
        resources.directories = vec![selected.join("nested")];
        assert!(resources
            .require_unowned(&selected, "lucidos_selected")
            .is_err());
        resources.directories = vec![root.path().to_owned()];
        assert!(resources
            .require_unowned(&selected, "lucidos_selected")
            .is_err());
        resources.directories = vec![other.join("missing")];
        assert!(resources
            .require_unowned(&selected, "lucidos_selected")
            .is_ok());
        assert!(resources
            .require_unowned_destination(&other.join("missing"))
            .is_err());
    }

    #[test]
    fn sibling_candidate_containing_a_registered_folder_is_protected() {
        let root = tempfile::tempdir().unwrap();
        let own = root.path().join("own");
        let sibling = root.path().join("sibling");
        let nested = sibling.join("nested");
        for dir in [&own, &sibling, &nested] {
            std::fs::create_dir_all(dir.join(".lucidos")).unwrap();
            std::fs::create_dir_all(dir.join("data")).unwrap();
        }
        assert!(
            unregistered_siblings(std::slice::from_ref(&own), &[own.clone(), nested])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn sibling_discovery_excludes_peer_folders_and_refuses_unreadable_registries() {
        let root = tempfile::tempdir().unwrap();
        let own_dir = root.path().join("own");
        let peer_dir = root.path().join("peer-workspace");
        let orphan_dir = root.path().join("orphan");
        for directory in [&own_dir, &peer_dir, &orphan_dir] {
            std::fs::create_dir_all(directory.join(".lucidos")).unwrap();
            std::fs::create_dir_all(directory.join("data")).unwrap();
        }
        let mut own = Workspace::gateway_provisioned("own".into(), "Own".into(), 9100);
        own.dir = own_dir.to_string_lossy().into_owned();
        let peer_data = root.path().join("peer-install");
        std::fs::create_dir_all(peer_data.join("config")).unwrap();
        // Relative registrations resolve against the owning install, and aliases
        // must protect the same folder as its canonical spelling.
        std::os::unix::fs::symlink(&peer_dir, peer_data.join("alias")).unwrap();
        let mut peer = Workspace::gateway_provisioned("peer".into(), "Peer".into(), 9101);
        peer.dir = "alias".into();
        let registry_path = peer_data.join("config/workspaces.json");
        let mut registry = Registry::default();
        registry.workspaces.push(peer);
        registry.save(&registry_path).unwrap();
        let report = scan(vec![own.clone()], root.path(), vec![registry_path.clone()]);
        let siblings: Vec<_> = report
            .rows
            .iter()
            .filter(|r| r.workspace.is_none())
            .collect();
        assert_eq!(siblings.len(), 1);
        assert_eq!(
            siblings[0].directory,
            std::fs::canonicalize(orphan_dir).unwrap()
        );
        // An absent registration reserves its own path. It does not establish
        // an alias to another existing sibling after the symlink disappears.
        std::fs::remove_file(peer_data.join("alias")).unwrap();
        let report = scan(vec![own.clone()], root.path(), vec![registry_path.clone()]);
        assert_eq!(
            report.rows.iter().filter(|r| r.workspace.is_none()).count(),
            2
        );
        assert!(!report.warnings.iter().any(|w| w.contains("Sibling")));
        std::fs::write(&registry_path, "broken").unwrap();
        let report = scan(vec![own], root.path(), vec![registry_path]);
        assert!(report.rows.iter().all(|r| r.workspace.is_some()));
        assert!(report.registered_databases.is_none());
    }

    #[test]
    fn discovery_only_visits_workspace_shaped_siblings_and_skips_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let own = root.path().join("own");
        let sibling = root.path().join("sibling");
        let ordinary = root.path().join("ordinary");
        for dir in [&own, &sibling] {
            std::fs::create_dir_all(dir.join(".lucidos")).unwrap();
            std::fs::create_dir_all(dir.join("data")).unwrap();
        }
        std::fs::create_dir_all(&ordinary).unwrap();
        std::os::unix::fs::symlink(&sibling, root.path().join("alias")).unwrap();
        let found = sibling_workspaces(&[own]).unwrap();
        assert_eq!(found, vec![std::fs::canonicalize(sibling).unwrap()]);
    }

    #[test]
    fn aggregate_size_counts_cross_entry_hardlinks_once() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let payload = first.join("payload");
        std::fs::write(&payload, vec![42; 65536]).unwrap();
        std::fs::hard_link(&payload, second.join("linked")).unwrap();
        let separately = allocated_bytes(&first).unwrap() + allocated_bytes(&second).unwrap();
        let shared = std::fs::metadata(payload).unwrap().blocks() * 512;
        assert!(shared > 0);
        assert_eq!(
            allocated_bytes_many([first.as_path(), second.as_path()]).unwrap(),
            separately - shared
        );
        assert_eq!(
            allocated_bytes_many([first.as_path(), first.as_path()]).unwrap(),
            allocated_bytes(&first).unwrap()
        );
    }

    #[test]
    fn sizes_do_not_follow_links_outside_workspace() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("large"), vec![0; 65536]).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
        let first = allocated_bytes(root.path()).unwrap();
        std::fs::write(outside.path().join("larger"), vec![0; 131072]).unwrap();
        assert_eq!(allocated_bytes(root.path()).unwrap(), first);
    }
}
