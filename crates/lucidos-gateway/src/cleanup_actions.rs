//! Frozen cleanup selections. The persisted definition, never browser paths,
//! controls execution. Each member is re-observed under its operation lock.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Selection {
    pub id: String,
    pub fingerprint: String,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct FrozenMember {
    pub id: String,
    pub fingerprint: String,
    #[serde(flatten)]
    pub resource: FrozenResource,
    #[serde(default)]
    pub retained_branches: Vec<String>,
}
/// Persist only the resource observed by the gateway; folder names never
/// imply database ownership. Untagged keeps existing registered batches readable.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum FrozenResource {
    Registered { workspace: Workspace },
    Folder { directory: PathBuf },
    Database { database: String },
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Batch {
    pub id: String,
    pub phrase: String,
    pub members: Vec<FrozenMember>,
    pub results: Vec<MemberResult>,
    #[serde(default)]
    pub actor: Option<crate::cleanup_audit::Actor>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub audits: std::collections::BTreeMap<String, crate::cleanup_audit::Audit>,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct MemberResult {
    pub id: String,
    pub status: String,
    pub detail: Option<String>,
}
impl Batch {
    pub(crate) fn public(mut self) -> Self {
        self.audits.clear();
        for member in &mut self.members {
            if let FrozenResource::Registered { workspace } = &mut member.resource {
                workspace.database_url = None;
                workspace.cleanup.owner_token_digest = None;
            }
        }
        self
    }
}
fn recovered_result(id: &str) -> MemberResult {
    MemberResult {
        id: id.into(),
        status: "skipped".into(),
        detail: Some(
            "Deletion was canceled by explicit Trash recovery; review again to delete".into(),
        ),
    }
}

fn confirmation_phrase<'a>(count: usize, branches: impl Iterator<Item = &'a str>) -> String {
    confirmation_with_branches(format!("Delete {count} workspaces"), branches)
}
fn confirmation_with_branches<'a>(
    mut phrase: String,
    branches: impl Iterator<Item = &'a str>,
) -> String {
    let branches: std::collections::BTreeSet<_> = branches.collect();
    if !branches.is_empty() {
        phrase.push_str("; keep branches: ");
        phrase.push_str(&branches.into_iter().collect::<Vec<_>>().join(", "));
    }
    phrase
}
fn batch_confirmation_phrase(members: &[FrozenMember]) -> Result<String, ApiError> {
    let protected = members.iter().find_map(|member| match &member.resource {
        FrozenResource::Registered { workspace }
            if matches!(
                workspace.cleanup.origin,
                crate::registry::WorkspaceOrigin::User | crate::registry::WorkspaceOrigin::Unknown
            ) =>
        {
            Some(workspace.name.clone())
        }
        FrozenResource::Folder { directory } => Some(directory.display().to_string()),
        FrozenResource::Database { database } => Some(database.clone()),
        _ => None,
    });
    let branches = members
        .iter()
        .flat_map(|member| member.retained_branches.iter().map(String::as_str));
    match protected {
        Some(name) if members.len() == 1 => Ok(confirmation_with_branches(
            format!("Delete {name}"),
            branches,
        )),
        Some(_) => Err(ApiError::bad_request(
            "user and unknown workspaces require a separate named confirmation",
        )),
        None => Ok(confirmation_phrase(members.len(), branches)),
    }
}
fn batch_root(app_data: &Path, id: &str) -> Result<PathBuf, ApiError> {
    if !id.starts_with("batch-") || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(ApiError::bad_request("invalid batch"));
    }
    Ok(app_data.join("cleanup-batches").join(id))
}
fn save(batch: &Batch, root: &Path) -> Result<(), BoxError> {
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer(file.as_file_mut(), batch)?;
    file.as_file().sync_all()?;
    file.persist(root.join("batch.json"))?;
    std::fs::File::open(root)?.sync_all()?;
    Ok(())
}
/// External effects are injected so tests exercise the production journal
/// ordering without a running cluster or access to live workspace databases.
enum DatabaseDeleteStep {
    Observe,
    Dump(PathBuf),
    Mark(String),
    Drop,
}
async fn execute_database_cleanup<F, Fut>(
    parent: &Path,
    database: &str,
    audit: Option<&crate::cleanup_audit::Audit>,
    mut effect: F,
) -> Result<(), BoxError>
where
    F: FnMut(DatabaseDeleteStep) -> Fut,
    Fut: std::future::Future<Output = Result<(), BoxError>>,
{
    effect(DatabaseDeleteStep::Observe).await?;
    let (mut entry, root) = crate::trash::DatabaseEntry::prepare(parent, database)?;
    entry.audit = audit.cloned();
    entry.save(&root)?;
    effect(DatabaseDeleteStep::Dump(root.join("database.dump"))).await?;
    entry.commit_dump(&root)?;
    effect(DatabaseDeleteStep::Observe).await?;
    entry.phase = crate::trash::Phase::Moved;
    entry.save(&root)?;
    effect(DatabaseDeleteStep::Mark(entry.deletion_operation.clone())).await?;
    // A crash before this sync cannot have reached DROP, even if COMMENT ran.
    // A crash after it resolves through the operation marker or the dump.
    entry.drop_started = Some(true);
    entry.save(&root)?;
    effect(DatabaseDeleteStep::Drop).await?;
    entry.phase = crate::trash::Phase::Completed;
    entry.save(&root)
}
/// Explicit unlock matters for a short operation lease: a concurrently forked
/// child may still hold a copy of the file description until exec. That child
/// must not extend the lease after the parent's operation has finished.
pub(super) struct ResourcePublicationLock(std::fs::File);
impl Drop for ResourcePublicationLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}
impl GatewayState {
    /// Caller holds the resource publication lease and workspace operation lock.
    pub(super) async fn reconcile_cleanup_entry<F, Fut>(
        &self,
        entry: &mut crate::trash::Entry,
        journal_root: &Path,
        resource: &FrozenResource,
        database_exists: &mut F,
    ) -> Result<(), BoxError>
    where
        F: FnMut(String) -> Fut,
        Fut: std::future::Future<Output = Result<bool, BoxError>>,
    {
        let workspace = match resource {
            FrozenResource::Registered { workspace } => {
                if entry
                    .workspace
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()?
                    != Some(serde_json::to_value(workspace)?)
                    || entry.original_dir != workspace.resolve_dir(self.app_data())
                {
                    return Err::<(), BoxError>("cleanup journal resource mismatch".into());
                }
                Some(workspace)
            }
            FrozenResource::Folder { directory }
                if entry.workspace.is_none() && entry.original_dir == *directory =>
            {
                None
            }
            _ => return Err("cleanup journal resource mismatch".into()),
        };
        // The publication lease prevents participating gateways from adopting
        // either resource between these proofs and terminal publication.
        let others = {
            let registry = self.inner.registry.lock().unwrap();
            if let Some(ws) = workspace {
                if let Some(current) = registry.get(&ws.id) {
                    if serde_json::to_value(current)? != serde_json::to_value(ws)? {
                        return Err("workspace registration changed".into());
                    }
                }
            }
            registry
                .workspaces
                .iter()
                .filter(|ws| workspace.is_none_or(|original| ws.id != original.id))
                .cloned()
                .collect::<Vec<_>>()
        };
        let resources = crate::cleanup::registered_resources(
            &others,
            self.app_data(),
            &self.cleanup_peer_paths(),
        )?;
        resources.require_unowned_destination(&entry.original_dir)?;
        entry.reconcile_moved(journal_root)?;
        let _owner = lucidos_owner_lock::OwnerLock::try_acquire(
            &lucidos_owner_lock::engine::lock_path(&journal_root.join("workspace")),
        )?
        .ok_or("moved workspace engine is running")?;
        if let Some(ws) = workspace {
            let database = postgres::database_name(&ws.id)?;
            resources.require_unowned_database(&database)?;
            if database_exists(database).await? {
                return Err("database still exists; explicit Trash recovery required".into());
            }
            self.trash_snapshots(&ws.id, journal_root)?;
        }
        entry.phase = crate::trash::Phase::DatabaseDropped;
        entry.save(journal_root)?;
        entry.prune_worktrees(journal_root)?;
        if let Some(ws) = workspace {
            let mut registry = self.inner.registry.lock().unwrap();
            if registry.contains(&ws.id) {
                let mut updated = registry.clone();
                updated.remove(&ws.id)?;
                updated.save(&self.inner.registry_path)?;
                *registry = updated;
            }
        }
        entry.phase = crate::trash::Phase::Completed;
        entry.save(journal_root)?;
        if let Some(ws) = workspace {
            self.clear_runtime_state(&ws.id);
            self.clear_stop(&ws.id);
        }
        Ok(())
    }

    /// Same host-wide lock for every gateway, including distinct installations
    /// sharing Docker or overlapping resource directories. Never wait while
    /// holding a per-workspace lock: a competing publication returns conflict.
    pub(super) fn resource_publication_lock(&self) -> Result<ResourcePublicationLock, ApiError> {
        let acquire = || -> Result<ResourcePublicationLock, BoxError> {
            use std::os::unix::fs::OpenOptionsExt;
            let root = &self.inner.snapshot_home;
            std::fs::create_dir_all(root)?;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(root.join("workspace-resources.lock"))?;
            fs2::FileExt::try_lock_exclusive(&file)?;
            Ok(ResourcePublicationLock(file))
        };
        acquire().map_err(|e| {
            ApiError::conflict(format!(
                "workspace resources are busy or unavailable; retry: {e}"
            ))
        })
    }

    pub(super) fn cleanup_peer_paths(&self) -> Vec<PathBuf> {
        crate::installs::inventory(std::env::current_exe().ok(), self.app_data())
            .installs
            .into_iter()
            .filter_map(|install| install.data_dir)
            .filter(|dir| dir != self.app_data())
            .map(|dir| dir.join("config/workspaces.json"))
            .collect()
    }

    fn database_ownership(&self, database: &str) -> Result<(), BoxError> {
        let registry = self.inner.registry.lock().unwrap();
        crate::cleanup::registered_resources(
            &registry.workspaces,
            self.app_data(),
            &self.cleanup_peer_paths(),
        )?
        .require_unowned_database(database)
    }

    async fn delete_frozen_database(
        &self,
        database: &str,
        fingerprint: &str,
        audit: &crate::cleanup_audit::Audit,
    ) -> Result<(), BoxError> {
        execute_database_cleanup(
            &self.app_data().join("deleted"),
            database,
            Some(audit),
            |step| async move {
                let backend = self.inner.pg_backend.clone();
                let app_data = self.app_data().clone();
                let target = database.to_owned();
                match step {
                    DatabaseDeleteStep::Observe => {
                        self.database_ownership(database)?;
                        let fingerprint = fingerprint.to_owned();
                        tokio::task::spawn_blocking(move || -> Result<(), BoxError> {
                            let observations = postgres::cleanup_observations(&backend, &app_data)?;
                            let observed = observations
                                .iter()
                                .find(|db| db.name == target)
                                .ok_or("database unavailable")?;
                            if crate::cleanup::database_fingerprint(observed).as_deref()
                                != Some(&fingerprint)
                            {
                                return Err("database changed since review; skipped".into());
                            }
                            Ok(())
                        })
                        .await??;
                        self.database_ownership(database)
                    }
                    DatabaseDeleteStep::Dump(path) => {
                        tokio::task::spawn_blocking(move || {
                            postgres::cleanup_dump_named(&backend, &app_data, &target, &path)
                        })
                        .await?
                    }
                    DatabaseDeleteStep::Mark(operation) => {
                        tokio::task::spawn_blocking(move || {
                            postgres::cleanup_mark_named_original(
                                &backend, &app_data, &target, &operation,
                            )
                        })
                        .await?
                    }
                    DatabaseDeleteStep::Drop => {
                        self.database_ownership(database)?;
                        postgres::teardown_named_database(&backend, &target, &app_data).await
                    }
                }
            },
        )
        .await
    }

    /// Paired-device action: leave the occupied staging database untouched and
    /// durably record its name before ordinary restore allocates another one.
    pub(crate) async fn rotate_trash_restore(&self, id: &str) -> Result<(), ApiError> {
        let backend = self.inner.pg_backend.clone();
        let home = self.app_data().clone();
        self.rotate_trash_restore_with(id, move |target, old, new| {
            postgres::cleanup_rotate_restore(&backend, &home, target, old, new)
        })
        .await
    }

    async fn rotate_trash_restore_with(
        &self,
        id: &str,
        observe: impl FnOnce(&str, &str, &str) -> Result<(), BoxError> + Send + 'static,
    ) -> Result<(), ApiError> {
        let failure = |e: BoxError| ApiError::conflict(e.to_string());
        let parent = self.app_data().join("deleted");
        let database_only = id.starts_with("database-");
        let (key, target) = if database_only {
            let (entry, _) = crate::trash::DatabaseEntry::load(&parent, id).map_err(failure)?;
            (format!("database:{}", entry.database), entry.database)
        } else {
            let (entry, _) = crate::trash::Entry::load(&parent, id).map_err(failure)?;
            let workspace = entry
                .workspace
                .ok_or_else(|| ApiError::conflict("folder has no database staging"))?;
            (
                workspace.id.clone(),
                postgres::database_name(&workspace.id).map_err(failure)?,
            )
        };
        let _operation = self.workspace_operation(&key).await;
        let _publication = self.resource_publication_lock()?;
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || -> Result<(), BoxError> {
            let replacement = lucidos_local_token::mint_hex(16)?;
            if database_only {
                let (mut entry, root) = crate::trash::DatabaseEntry::load(&parent, &id)?;
                if entry.database != target
                    || entry.phase != crate::trash::Phase::Restoring
                    || !entry.dump_ready
                {
                    return Err("Trash recovery identity changed or is not restoring".into());
                }
                let old = entry
                    .restore_operation
                    .as_ref()
                    .ok_or("missing recovery identity")?;
                observe(&target, old, &replacement)?;
                entry
                    .abandoned_staging
                    .push(format!("lucidos_restore_{old}"));
                entry.restore_operation = Some(replacement);
                entry.save(&root)
            } else {
                let (mut entry, root) = crate::trash::Entry::load(&parent, &id)?;
                if entry.workspace.as_ref().map(|ws| &ws.id) != Some(&key)
                    || entry.phase != crate::trash::Phase::Restoring
                    || !entry.dump_ready
                {
                    return Err("Trash recovery identity changed or is not restoring".into());
                }
                let recovery = entry.recovery.as_mut().ok_or("missing recovery identity")?;
                observe(&target, &recovery.operation, &replacement)?;
                recovery
                    .abandoned_staging
                    .push(format!("lucidos_restore_{}", recovery.operation));
                recovery.operation = replacement;
                entry.save(&root)
            }
        })
        .await
        .map_err(|e| ApiError::conflict(e.to_string()))?
        .map_err(failure)
    }

    pub(crate) async fn restore_database_trash(&self, id: &str) -> Result<(), ApiError> {
        let failure = |e: BoxError| ApiError::conflict(e.to_string());
        let parent = self.app_data().join("deleted");
        let (entry, _) = crate::trash::DatabaseEntry::load(&parent, id).map_err(failure)?;
        let _operation = self
            .workspace_operation(&format!("database:{}", entry.database))
            .await;
        let (mut entry, root) = crate::trash::DatabaseEntry::load(&parent, id).map_err(failure)?;
        let _publication = self.resource_publication_lock()?;
        self.database_ownership(&entry.database).map_err(failure)?;
        let backend = self.inner.pg_backend.clone();
        let app_data = self.app_data().clone();
        tokio::task::spawn_blocking(move || -> Result<(), BoxError> {
            if entry.can_cancel_without_database() {
                entry.phase = crate::trash::Phase::Restored;
                return entry.save(&root);
            }
            entry.begin_restore(&root)?;
            postgres::cleanup_restore_named(
                &backend,
                &app_data,
                &entry.database,
                entry
                    .restore_operation
                    .as_deref()
                    .ok_or("missing recovery operation")?,
                &root.join("database.dump"),
                Some(&entry.deletion_operation),
            )?;
            entry.phase = crate::trash::Phase::Restored;
            entry.save(&root)
        })
        .await
        .map_err(|e| ApiError::conflict(e.to_string()))?
        .map_err(failure)
    }

    fn folder_ownership(
        &self,
        directory: &Path,
        restoring: bool,
        peers: &[PathBuf],
    ) -> Result<(), BoxError> {
        let registry = self.inner.registry.lock().unwrap();
        let resources =
            crate::cleanup::registered_resources(&registry.workspaces, self.app_data(), peers)?;
        if restoring {
            resources.require_unowned_destination(directory)
        } else {
            resources.require_unowned_directory(directory)
        }
    }

    async fn delete_frozen_folder(
        &self,
        directory: &Path,
        fingerprint: &str,
        audit: &crate::cleanup_audit::Audit,
    ) -> Result<(), BoxError> {
        self.delete_frozen_folder_with_peers(
            directory,
            fingerprint,
            &self.cleanup_peer_paths(),
            Some(audit.clone()),
        )
        .await
    }

    async fn delete_frozen_folder_with_peers(
        &self,
        directory: &Path,
        fingerprint: &str,
        peers: &[PathBuf],
        audit: Option<crate::cleanup_audit::Audit>,
    ) -> Result<(), BoxError> {
        self.folder_ownership(directory, false, peers)?;
        // Do not stop an unregistered engine: a changed/running resource must survive.
        let lock_path = lucidos_owner_lock::engine::lock_path(directory);
        let _owner = lucidos_owner_lock::OwnerLock::try_acquire(&lock_path)?
            .ok_or("folder engine is running; skipped")?;
        self.folder_ownership(directory, false, peers)?;
        let bytes = crate::cleanup::allocated_bytes(directory)?;
        if crate::cleanup::folder_fingerprint(directory, false, bytes)? != fingerprint {
            return Err("folder changed since review; skipped".into());
        }
        let parent = self.app_data().join("deleted");
        let directory = directory.to_owned();
        tokio::task::spawn_blocking(move || -> Result<(), BoxError> {
            let (mut entry, root) = crate::trash::Entry::prepare_folder(&parent, directory)?;
            entry.audit = audit;
            entry.save(&root)?;
            entry.move_folder(&root)?;
            // No database belongs to this resource.
            entry.phase = crate::trash::Phase::DatabaseDropped;
            entry.save(&root)?;
            entry.prune_worktrees(&root)?;
            entry.phase = crate::trash::Phase::Completed;
            entry.save(&root)
        })
        .await?
    }

    pub(crate) async fn restore_folder_trash(
        &self,
        id: &str,
        directory: &Path,
    ) -> Result<(), ApiError> {
        self.restore_folder_trash_with_peers(id, directory, &self.cleanup_peer_paths())
            .await
    }

    async fn restore_folder_trash_with_peers(
        &self,
        id: &str,
        directory: &Path,
        peers: &[PathBuf],
    ) -> Result<(), ApiError> {
        let failure = |e: BoxError| ApiError::conflict(e.to_string());
        let key = crate::cleanup::folder_id(directory);
        let _operation = self.workspace_operation(&key).await;
        let _publication = self.resource_publication_lock()?;
        self.folder_ownership(directory, true, peers)
            .map_err(failure)?;
        let (mut entry, root) =
            crate::trash::Entry::load(&self.app_data().join("deleted"), id).map_err(failure)?;
        if entry.workspace.is_some() || entry.original_dir != directory {
            return Err(ApiError::conflict("Trash resource identity changed"));
        }
        tokio::task::spawn_blocking(move || -> Result<(), BoxError> {
            if entry.phase == crate::trash::Phase::Prepared {
                return entry.cancel_prepared(&root);
            }
            entry.begin_recovery(&root)?;
            entry.recover_folder(&root)?;
            entry.phase = crate::trash::Phase::Restored;
            entry.save(&root)
        })
        .await
        .map_err(|e| ApiError::conflict(e.to_string()))?
        .map_err(failure)
    }

    pub(crate) fn freeze_cleanup(&self, members: Vec<Selection>) -> Result<Batch, ApiError> {
        if members.is_empty() || members.len() > 1000 {
            return Err(ApiError::bad_request(
                "select between 1 and 1000 workspaces",
            ));
        }
        let report = self.inner.cleanup_review.lock().unwrap().report.clone();
        if report
            .scanned_at
            .is_none_or(|at| chrono::Utc::now().signed_duration_since(at).num_seconds() >= 60)
        {
            return Err(ApiError::conflict("refresh the workspace review first"));
        }
        let mut ids = HashSet::new();
        for member in &members {
            if !ids.insert(&member.id) {
                return Err(ApiError::bad_request("duplicate workspace"));
            }
            let row = report
                .rows
                .iter()
                .find(|r| r.id == member.id)
                .ok_or_else(|| ApiError::conflict("workspace is no longer in the review"))?;
            if row.fingerprint.as_deref() != Some(&member.fingerprint)
                || (row.workspace.is_some()
                    && row.database.as_ref().is_none_or(|d| d.warning.is_some()))
                || (row.workspace.is_none()
                    && !matches!(row.state, "not-registered" | "database-only"))
                || row.warning.is_some()
                || (row.state != "database-only" && row.running.is_none())
            {
                return Err(ApiError::conflict(
                    "workspace observations are unavailable or changed",
                ));
            }
        }
        let registry = self.inner.registry.lock().unwrap().clone();
        let members = members
            .into_iter()
            .map(|selection| {
                let row = report
                    .rows
                    .iter()
                    .find(|row| row.id == selection.id)
                    .ok_or_else(|| ApiError::conflict("resource was removed"))?;
                let resource = if let Some(workspace) = &row.workspace {
                    let current = registry
                        .get(&workspace.id)
                        .ok_or_else(|| ApiError::conflict("workspace was removed"))?;
                    FrozenResource::Registered {
                        workspace: current.clone(),
                    }
                } else if row.state == "database-only" {
                    FrozenResource::Database {
                        database: row
                            .database
                            .as_ref()
                            .ok_or_else(|| ApiError::conflict("database observation unavailable"))?
                            .name
                            .clone(),
                    }
                } else {
                    FrozenResource::Folder {
                        directory: row.directory.clone(),
                    }
                };
                Ok(FrozenMember {
                    id: selection.id,
                    fingerprint: selection.fingerprint,
                    retained_branches: if row.state == "database-only" {
                        vec![]
                    } else {
                        crate::trash::retained_branches(&row.directory).map_err(|e| {
                            ApiError::conflict(format!("branch observation unavailable: {e}"))
                        })?
                    },
                    resource,
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        let phrase = batch_confirmation_phrase(&members)?;
        let result = (|| -> Result<Batch, BoxError> {
            use std::os::unix::fs::DirBuilderExt;
            let parent = self.app_data().join("cleanup-batches");
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&parent)?;
            let root = tempfile::Builder::new()
                .prefix("batch-")
                .tempdir_in(&parent)?
                .keep();
            let id = root
                .file_name()
                .ok_or("batch name missing")?
                .to_string_lossy()
                .into_owned();
            let batch = Batch {
                id,
                phrase,
                members,
                results: vec![],
                actor: None,
                audits: Default::default(),
            };
            save(&batch, &root)?;
            std::fs::File::open(&parent)?.sync_all()?;
            Ok(batch)
        })();
        result.map_err(|e| ApiError::bad_request(e.to_string()))
    }

    async fn cleanup_current_fingerprint(
        &self,
        ws: &Workspace,
        retained_branches: Vec<String>,
    ) -> Result<String, BoxError> {
        let backend = self.inner.pg_backend.clone();
        let app_data = self.app_data().clone();
        let workspace = ws.clone();
        let other_workspaces = self
            .inner
            .registry
            .lock()
            .unwrap()
            .workspaces
            .iter()
            .filter(|other| other.id != ws.id)
            .cloned()
            .collect::<Vec<_>>();
        let peers = self.cleanup_peer_paths();
        tokio::task::spawn_blocking(move || {
            let dir = workspace.resolve_dir(&app_data);
            if crate::trash::retained_branches(&dir)? != retained_branches {
                return Err("retained branches changed since confirmation was frozen".into());
            }
            let name = postgres::database_name(&workspace.id)?;
            crate::cleanup::registered_resources(&other_workspaces, &app_data, &peers)?
                .require_unowned(&dir, &name)?;
            let databases = postgres::cleanup_observations(&backend, &app_data)?;
            let db = databases
                .iter()
                .find(|db| db.name == name)
                .ok_or("database unavailable")?;
            if db.warning.is_some() {
                return Err("activity unavailable".into());
            }
            let running =
                lucidos_owner_lock::is_held(&lucidos_owner_lock::engine::lock_path(&dir))?;
            crate::cleanup::observed_workspace_fingerprint(
                &workspace,
                &dir,
                Some(db),
                Some(running),
                Some(crate::cleanup::allocated_bytes(&dir)?),
            )
            .ok_or_else(|| "workspace observations unavailable".into())
        })
        .await?
    }

    pub(crate) async fn confirm_cleanup(
        &self,
        id: &str,
        phrase: &str,
        device_id: &str,
    ) -> Result<Batch, ApiError> {
        self.confirm_cleanup_with_observation(id, phrase, device_id, |database| async move {
            let backend = self.inner.pg_backend.clone();
            let app_data = self.app_data().to_owned();
            tokio::task::spawn_blocking(move || {
                postgres::cleanup_database_exists(&backend, &app_data, &database)
            })
            .await?
        })
        .await
    }

    async fn confirm_cleanup_with_observation<F, Fut>(
        &self,
        id: &str,
        phrase: &str,
        device_id: &str,
        mut database_exists: F,
    ) -> Result<Batch, ApiError>
    where
        F: FnMut(String) -> Fut,
        Fut: std::future::Future<Output = Result<bool, BoxError>>,
    {
        let _review = self.cleanup_review_mutation();
        let _batch_lock = self
            .workspace_operation(&format!("cleanup-batch:{id}"))
            .await;
        let root = batch_root(self.app_data(), id)?;
        let mut batch: Batch = std::fs::File::open(root.join("batch.json"))
            .map_err(|_| ApiError::not_found("batch not found"))
            .and_then(|f| {
                serde_json::from_reader(f).map_err(|_| ApiError::conflict("batch is unreadable"))
            })?;
        if batch.id != id
            || batch.phrase != phrase
            || batch_confirmation_phrase(&batch.members)? != phrase
        {
            return Err(ApiError::bad_request("confirmation does not match"));
        }
        if batch.actor.is_none() {
            batch.actor = Some(crate::cleanup_audit::Actor::Device {
                device_id: device_id.into(),
            });
            save(&batch, &root).map_err(|e| ApiError::conflict(e.to_string()))?;
        }
        let _publication = self.resource_publication_lock()?;
        let actor = batch
            .actor
            .as_ref()
            .ok_or_else(|| ApiError::conflict("missing cleanup actor"))?;
        for member in &batch.members {
            if batch.results.iter().any(|r| r.id == member.id) {
                continue;
            }
            if let Some(audit) = batch.audits.get(&member.id) {
                if !audit.matches(&member.id, actor) {
                    return Err(ApiError::conflict(
                        "cleanup audit does not match frozen member",
                    ));
                }
            } else {
                let audit =
                    crate::cleanup_audit::Audit::begin(self.app_data(), &member.id, actor.clone())
                        .map_err(|e| ApiError::conflict(e.to_string()))?;
                batch.audits.insert(member.id.clone(), audit);
            }
        }
        // Every pending member's identity is durable before the first external effect.
        save(&batch, &root).map_err(|e| ApiError::conflict(e.to_string()))?;
        // Terminal journals repair completion events before retry observes resources.
        crate::trash::list(self.app_data()).map_err(|e| ApiError::conflict(e.to_string()))?;
        for member in &batch.members {
            if batch.results.iter().any(|r| r.id == member.id) {
                continue;
            }
            let _operation = self.workspace_operation(&member.id).await;
            let audit = batch
                .audits
                .get(&member.id)
                .ok_or_else(|| ApiError::conflict("missing cleanup audit"))?
                .clone();
            if audit
                .is_completed(self.app_data())
                .map_err(|e| ApiError::conflict(e.to_string()))?
            {
                batch.results.push(MemberResult {
                    id: member.id.clone(),
                    status: "deleted".into(),
                    detail: None,
                });
                save(&batch, &root).map_err(|e| ApiError::conflict(e.to_string()))?;
                continue;
            }
            if let Some((mut entry, journal_root)) =
                crate::trash::Entry::find_audit(&self.app_data().join("deleted"), &audit)
                    .map_err(|e| ApiError::conflict(e.to_string()))?
            {
                if entry.phase == crate::trash::Phase::Restored {
                    batch.results.push(recovered_result(&member.id));
                    save(&batch, &root).map_err(|e| ApiError::conflict(e.to_string()))?;
                    continue;
                }
                let reconcile = self
                    .reconcile_cleanup_entry(
                        &mut entry,
                        &journal_root,
                        &member.resource,
                        &mut database_exists,
                    )
                    .await;
                reconcile.map_err(|e| {
                    ApiError::conflict(format!("cleanup interrupted; result remains pending: {e}"))
                })?;
                batch.results.push(MemberResult {
                    id: member.id.clone(),
                    status: "deleted".into(),
                    detail: None,
                });
                save(&batch, &root).map_err(|e| ApiError::conflict(e.to_string()))?;
                continue;
            }
            if let FrozenResource::Database { database } = &member.resource {
                if let Some((mut entry, journal_root)) = crate::trash::DatabaseEntry::find_audit(
                    &self.app_data().join("deleted"),
                    &audit,
                )
                .map_err(|e| ApiError::conflict(e.to_string()))?
                {
                    if entry.database != *database {
                        return Err(ApiError::conflict("cleanup journal resource mismatch"));
                    }
                    if entry.phase == crate::trash::Phase::Restored {
                        batch.results.push(recovered_result(&member.id));
                        save(&batch, &root).map_err(|e| ApiError::conflict(e.to_string()))?;
                        continue;
                    }
                    self.database_ownership(database)
                        .map_err(|e| ApiError::conflict(e.to_string()))?;
                    let exists = database_exists(database.clone())
                        .await
                        .map_err(|e| ApiError::conflict(e.to_string()))?;
                    entry
                        .finish_dropped(&journal_root, !exists)
                        .map_err(|e| ApiError::conflict(e.to_string()))?;
                    batch.results.push(MemberResult {
                        id: member.id.clone(),
                        status: "deleted".into(),
                        detail: None,
                    });
                    save(&batch, &root).map_err(|e| ApiError::conflict(e.to_string()))?;
                    continue;
                }
            }
            let ws = self.inner.registry.lock().unwrap().get(&member.id).cloned();
            let outcome = if let FrozenResource::Folder { directory } = &member.resource {
                self.delete_frozen_folder(directory, &member.fingerprint, &audit)
                    .await
                    .map_err(|e| e.to_string())
            } else if let FrozenResource::Database { database } = &member.resource {
                self.delete_frozen_database(database, &member.fingerprint, &audit)
                    .await
                    .map_err(|e| e.to_string())
            } else {
                match ws {
                    None => Err("workspace is no longer registered".to_owned()),
                    Some(ws)
                        if serde_json::to_value(&ws).ok()
                            != match &member.resource {
                                FrozenResource::Registered { workspace } => {
                                    serde_json::to_value(workspace).ok()
                                }
                                FrozenResource::Folder { .. } | FrozenResource::Database { .. } => {
                                    None
                                }
                            } =>
                    {
                        Err("workspace registration changed; skipped".into())
                    }
                    Some(ws) => match self
                        .cleanup_current_fingerprint(&ws, member.retained_branches.clone())
                        .await
                    {
                        Ok(current) if current == member.fingerprint => self
                            .delete_confirmed_workspace(&ws, &audit)
                            .await
                            .map_err(|e| e.to_string()),
                        Ok(_) => Err("workspace changed since review; skipped".into()),
                        Err(e) => Err(format!("observation unavailable; skipped: {e}")),
                    },
                }
            };
            if let Err(error) = &outcome {
                if crate::trash::DatabaseEntry::find_audit(&self.app_data().join("deleted"), &audit)
                    .map_err(|e| ApiError::conflict(e.to_string()))?
                    .is_some()
                    || crate::trash::Entry::find_audit(&self.app_data().join("deleted"), &audit)
                        .map_err(|e| ApiError::conflict(e.to_string()))?
                        .is_some()
                {
                    return Err(ApiError::conflict(format!(
                        "cleanup interrupted; result remains pending: {error}"
                    )));
                }
            }
            if outcome.is_ok() {
                audit
                    .completed(self.app_data())
                    .map_err(|e| ApiError::conflict(e.to_string()))?;
            }
            batch.results.push(MemberResult {
                id: member.id.clone(),
                status: if outcome.is_ok() {
                    "deleted"
                } else {
                    "skipped"
                }
                .into(),
                detail: outcome.err(),
            });
            save(&batch, &root).map_err(|e| ApiError::conflict(e.to_string()))?;
        }
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn staging_rotation_persists_history_only_after_successful_observation() {
        for database_only in [false, true] {
            for allowed in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
                let parent = state.app_data().join("deleted");
                let (root, old) = if database_only {
                    let (mut entry, root) =
                        crate::trash::DatabaseEntry::prepare(&parent, "lucidos_scratch").unwrap();
                    std::fs::write(root.join("database.dump"), "recovery").unwrap();
                    entry.dump_ready = true;
                    entry.phase = crate::trash::Phase::Completed;
                    entry.save(&root).unwrap();
                    entry.begin_restore(&root).unwrap();
                    (root, entry.restore_operation.unwrap())
                } else {
                    let source = temp.path().join("source");
                    std::fs::create_dir(&source).unwrap();
                    let mut ws =
                        Workspace::gateway_provisioned("scratch".into(), "Scratch".into(), 9100);
                    ws.dir = source.to_string_lossy().into_owned();
                    let (mut entry, root) =
                        crate::trash::Entry::prepare(&parent, &ws, source, |dump| {
                            std::fs::write(dump, "recovery")?;
                            Ok(())
                        })
                        .unwrap();
                    entry.move_folder(&root).unwrap();
                    entry.phase = crate::trash::Phase::Completed;
                    entry.save(&root).unwrap();
                    entry.begin_recovery(&root).unwrap();
                    (root, entry.recovery.unwrap().operation)
                };
                let id = root.file_name().unwrap().to_str().unwrap();
                let expected_old = old.clone();
                let result = state
                    .rotate_trash_restore_with(id, move |target, old, new| {
                        assert_eq!(target, "lucidos_scratch");
                        assert_eq!(old, expected_old);
                        assert_ne!(old, new);
                        if allowed {
                            Ok(())
                        } else {
                            Err("catalog unavailable or occupied".into())
                        }
                    })
                    .await;
                assert_eq!(result.is_ok(), allowed);
                let (operation, history) = if database_only {
                    let (entry, _) = crate::trash::DatabaseEntry::load(&parent, id).unwrap();
                    (entry.restore_operation.unwrap(), entry.abandoned_staging)
                } else {
                    let (entry, _) = crate::trash::Entry::load(&parent, id).unwrap();
                    let recovery = entry.recovery.unwrap();
                    (recovery.operation, recovery.abandoned_staging)
                };
                assert_eq!(operation == old, !allowed);
                assert_eq!(
                    history,
                    if allowed {
                        vec![format!("lucidos_restore_{old}")]
                    } else {
                        vec![]
                    }
                );
                assert_eq!(
                    std::fs::read_to_string(root.join("database.dump")).unwrap(),
                    "recovery"
                );
                let listing = crate::trash::list(state.app_data()).unwrap();
                assert_eq!(
                    listing[0]
                        .warning
                        .as_ref()
                        .is_some_and(|w| w.contains(&old)),
                    allowed
                );
            }
        }
    }

    #[tokio::test]
    async fn owner_retry_reuses_interrupted_journal_and_original_audit() {
        for observation in [Some(false), Some(true), None] {
            let temp = tempfile::tempdir().unwrap();
            let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
            let source = temp.path().join("source");
            std::fs::create_dir(&source).unwrap();
            std::fs::write(source.join("retained"), "unique work").unwrap();
            let mut workspace =
                Workspace::gateway_provisioned("scratch".into(), "Scratch".into(), 9100);
            workspace.dir = source.to_string_lossy().into_owned();
            workspace.cleanup.origin = crate::registry::WorkspaceOrigin::Eval;
            workspace.cleanup.ephemeral = true;
            let token = "owner-secret";
            workspace.cleanup.owner_token_digest = Some(crate::auth::digest(token));
            {
                let mut registry = state.inner.registry.lock().unwrap();
                registry.workspaces.push(workspace.clone());
                registry.save(&state.inner.registry_path).unwrap();
            }
            let audit = crate::cleanup_audit::Audit::begin(
                state.app_data(),
                "scratch",
                crate::cleanup_audit::Actor::Owner {
                    workspace_id: "scratch".into(),
                },
            )
            .unwrap();
            let (mut entry, journal) = crate::trash::Entry::prepare(
                &state.app_data().join("deleted"),
                &workspace,
                source.clone(),
                |_| Ok(()),
            )
            .unwrap();
            entry.audit = Some(audit.clone());
            entry.save(&journal).unwrap();
            crate::trash::rename_exclusive(&source, &journal.join("workspace")).unwrap();
            let result = state
                .delete_workspace_authorized_with_observation(
                    "scratch",
                    Some("Scratch"),
                    Some(token),
                    |database| {
                        assert_eq!(database, "lucidos_scratch");
                        std::future::ready(observation.ok_or_else(|| "catalog unavailable".into()))
                    },
                )
                .await;
            assert_eq!(result.is_ok(), observation == Some(false));
            assert_eq!(
                audit.is_completed(state.app_data()).unwrap(),
                observation == Some(false)
            );
            assert_eq!(
                state.inner.registry.lock().unwrap().contains("scratch"),
                observation != Some(false)
            );
            assert!(!source.exists());
            assert_eq!(
                std::fs::read_to_string(journal.join("workspace/retained")).unwrap(),
                "unique work"
            );
            assert_eq!(
                std::fs::read_dir(state.app_data().join("deleted"))
                    .unwrap()
                    .count(),
                1
            );
            assert_eq!(
                std::fs::read_dir(state.app_data().join("cleanup-events"))
                    .unwrap()
                    .count(),
                if observation == Some(false) { 2 } else { 1 }
            );
        }
    }

    #[tokio::test]
    async fn batch_retry_finishes_moved_folder_and_registered_journals() {
        for registered in [false, true] {
            for observation in [Some(false), Some(true), None] {
                let temp = tempfile::tempdir().unwrap();
                let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
                let source = temp.path().join("source");
                std::fs::create_dir(&source).unwrap();
                std::fs::write(source.join("retained"), "unique work").unwrap();
                let mut workspace =
                    Workspace::gateway_provisioned("scratch".into(), "Scratch".into(), 9100);
                workspace.dir = source.to_string_lossy().into_owned();
                let resource = if registered {
                    {
                        let mut registry = state.inner.registry.lock().unwrap();
                        registry.workspaces.push(workspace.clone());
                        registry.save(&state.inner.registry_path).unwrap();
                    }
                    FrozenResource::Registered {
                        workspace: workspace.clone(),
                    }
                } else {
                    FrozenResource::Folder {
                        directory: source.clone(),
                    }
                };
                let actor = crate::cleanup_audit::Actor::Device {
                    device_id: "first-device".into(),
                };
                let audit =
                    crate::cleanup_audit::Audit::begin(state.app_data(), "scratch", actor.clone())
                        .unwrap();
                let (mut entry, journal) = if registered {
                    crate::trash::Entry::prepare(
                        &state.app_data().join("deleted"),
                        &workspace,
                        source.clone(),
                        |path| {
                            std::fs::write(path, "recovery dump")?;
                            Ok(())
                        },
                    )
                    .unwrap()
                } else {
                    crate::trash::Entry::prepare_folder(
                        &state.app_data().join("deleted"),
                        source.clone(),
                    )
                    .unwrap()
                };
                entry.audit = Some(audit.clone());
                entry.save(&journal).unwrap();
                // The rename happened, but saving Moved did not.
                crate::trash::rename_exclusive(&source, &journal.join("workspace")).unwrap();
                let root = batch_root(state.app_data(), "batch-test").unwrap();
                std::fs::create_dir_all(&root).unwrap();
                let phrase = if registered {
                    "Delete Scratch".to_owned()
                } else {
                    format!("Delete {}", source.display())
                };
                let batch = Batch {
                    id: "batch-test".into(),
                    phrase: phrase.clone(),
                    members: vec![FrozenMember {
                        id: "scratch".into(),
                        fingerprint: "obsolete".into(),
                        resource,
                        retained_branches: vec![],
                    }],
                    results: vec![],
                    actor: Some(actor),
                    audits: [("scratch".into(), audit.clone())].into_iter().collect(),
                };
                save(&batch, &root).unwrap();
                let mut called = false;
                let result = state
                    .confirm_cleanup_with_observation(
                        "batch-test",
                        &phrase,
                        "another-device",
                        |database| {
                            called = true;
                            assert_eq!(database, "lucidos_scratch");
                            std::future::ready(
                                observation.ok_or_else(|| "catalog unavailable".into()),
                            )
                        },
                    )
                    .await;
                assert_eq!(called, registered);
                let stored: Batch =
                    serde_json::from_reader(std::fs::File::open(root.join("batch.json")).unwrap())
                        .unwrap();
                if !registered || observation == Some(false) {
                    assert_eq!(result.unwrap().results[0].status, "deleted");
                    assert!(audit.is_completed(state.app_data()).unwrap());
                    assert!(!state.inner.registry.lock().unwrap().contains("scratch"));
                } else {
                    assert!(result.is_err());
                    assert!(stored.results.is_empty());
                    assert!(state.inner.registry.lock().unwrap().contains("scratch"));
                    assert!(!audit.is_completed(state.app_data()).unwrap());
                }
                assert!(!source.exists());
                assert_eq!(
                    std::fs::read_to_string(journal.join("workspace/retained")).unwrap(),
                    "unique work"
                );
            }
        }
    }

    #[tokio::test]
    async fn batch_retry_reconciles_drop_and_preserves_pending_on_unknown_or_present() {
        for observation in [None, Some(true), Some(false)] {
            let temp = tempfile::tempdir().unwrap();
            let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
            let root = batch_root(state.app_data(), "batch-test").unwrap();
            std::fs::create_dir_all(&root).unwrap();
            let actor = crate::cleanup_audit::Actor::Device {
                device_id: "device".into(),
            };
            let audit = crate::cleanup_audit::Audit::begin(
                state.app_data(),
                "database:lucidos_scratch",
                actor.clone(),
            )
            .unwrap();
            let (mut entry, journal) = crate::trash::DatabaseEntry::prepare(
                &state.app_data().join("deleted"),
                "lucidos_scratch",
            )
            .unwrap();
            entry.audit = Some(audit.clone());
            std::fs::write(journal.join("database.dump"), b"dump").unwrap();
            entry.commit_dump(&journal).unwrap();
            entry.phase = crate::trash::Phase::Moved;
            entry.drop_started = Some(true);
            entry.save(&journal).unwrap();
            let batch = Batch {
                id: "batch-test".into(),
                phrase: "Delete lucidos_scratch".into(),
                members: vec![FrozenMember {
                    id: "database:lucidos_scratch".into(),
                    fingerprint: "obsolete".into(),
                    resource: FrozenResource::Database {
                        database: "lucidos_scratch".into(),
                    },
                    retained_branches: vec![],
                }],
                results: vec![],
                actor: Some(actor),
                audits: [("database:lucidos_scratch".into(), audit.clone())]
                    .into_iter()
                    .collect(),
            };
            save(&batch, &root).unwrap();
            let mut called = false;
            let result = state
                .confirm_cleanup_with_observation(
                    "batch-test",
                    "Delete lucidos_scratch",
                    "another-device",
                    |database| {
                        called = true;
                        assert_eq!(database, "lucidos_scratch");
                        std::future::ready(observation.ok_or_else(|| "catalog unavailable".into()))
                    },
                )
                .await;
            assert!(called, "catalog observer was not reached");
            let stored: Batch =
                serde_json::from_reader(std::fs::File::open(root.join("batch.json")).unwrap())
                    .unwrap();
            if observation == Some(false) {
                assert_eq!(result.unwrap().results[0].status, "deleted");
                assert_eq!(stored.results[0].status, "deleted");
                assert!(audit.is_completed(state.app_data()).unwrap());
            } else {
                assert!(result.is_err());
                assert!(stored.results.is_empty());
                assert!(!audit.is_completed(state.app_data()).unwrap());
            }
            assert_eq!(stored.audits["database:lucidos_scratch"], audit);
        }
    }

    #[tokio::test]
    async fn database_execution_persists_recovery_before_every_external_step() {
        for failure in 0..=5 {
            let temp = tempfile::tempdir().unwrap();
            let parent = temp.path().join("deleted");
            let audit = crate::cleanup_audit::Audit::begin(
                temp.path(),
                "database:lucidos_Legacy-name",
                crate::cleanup_audit::Actor::Device {
                    device_id: "test-device".into(),
                },
            )
            .unwrap();
            let mut calls = 0;
            let result =
                execute_database_cleanup(&parent, "lucidos_Legacy-name", Some(&audit), |step| {
                    let index = calls;
                    calls += 1;
                    let result = (|| -> Result<(), BoxError> {
                        if index > 0 {
                            let root = std::fs::read_dir(&parent)?.next().unwrap()?.path();
                            let (entry, _) = crate::trash::DatabaseEntry::load(
                                &parent,
                                root.file_name().unwrap().to_str().unwrap(),
                            )?;
                            assert_eq!(entry.database, "lucidos_Legacy-name");
                            assert_eq!(
                                serde_json::to_value(entry.audit.as_ref().unwrap())?,
                                serde_json::to_value(&audit)?
                            );
                            assert_eq!(
                                std::fs::read_dir(temp.path().join("cleanup-events"))?.count(),
                                1
                            );
                            if matches!(step, DatabaseDeleteStep::Mark(_)) {
                                assert!(entry.dump_ready);
                                assert_eq!(entry.phase, crate::trash::Phase::Moved);
                                assert_eq!(entry.drop_started, Some(false));
                            }
                            if matches!(step, DatabaseDeleteStep::Drop) {
                                assert!(entry.dump_ready);
                                assert_eq!(entry.drop_started, Some(true));
                            }
                        }
                        if index == failure {
                            return Err("injected failure".into());
                        }
                        if let DatabaseDeleteStep::Dump(path) = step {
                            std::fs::write(path, b"durable recovery")?;
                        }
                        Ok(())
                    })();
                    std::future::ready(result)
                })
                .await;
            assert_eq!(result.is_ok(), failure == 5);
            assert_eq!(calls, (failure + 1).min(5));
            if failure == 0 {
                assert!(!parent.exists());
                continue;
            }
            let root = std::fs::read_dir(&parent)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let (entry, _) = crate::trash::DatabaseEntry::load(
                &parent,
                root.file_name().unwrap().to_str().unwrap(),
            )
            .unwrap();
            assert_eq!(entry.drop_started, Some(failure >= 4));
            assert_eq!(entry.phase == crate::trash::Phase::Completed, failure == 5);
            assert_eq!(
                std::fs::read_dir(temp.path().join("cleanup-events"))
                    .unwrap()
                    .count(),
                if failure == 5 { 2 } else { 1 }
            );
            // Before the durable drop checkpoint even a marked original needs
            // no SQL recovery. Afterwards the dump and operation resolve it.
            assert_eq!(entry.can_cancel_without_database(), failure < 4);
        }
    }

    #[tokio::test]
    async fn database_restore_cancels_before_drop_without_cluster_access() {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let parent = state.app_data().join("deleted");
        let (mut entry, root) =
            crate::trash::DatabaseEntry::prepare(&parent, "lucidos_orphan").unwrap();
        // Model the crash after Moved was synced, before or after COMMENT.
        // No DROP can have occurred, and this test has no Postgres to contact.
        entry.phase = crate::trash::Phase::Moved;
        entry.save(&root).unwrap();
        let id = root.file_name().unwrap().to_str().unwrap();
        state.restore_database_trash(id).await.unwrap();
        let (restored, _) = crate::trash::DatabaseEntry::load(&parent, id).unwrap();
        assert_eq!(restored.phase, crate::trash::Phase::Restored);
        assert!(restored.restore_operation.is_none());
        assert!(!restored.dump_ready);
    }

    #[test]
    fn legacy_database_intent_does_not_claim_drop_was_never_attempted() {
        let temp = tempfile::tempdir().unwrap();
        let (mut entry, _) =
            crate::trash::DatabaseEntry::prepare(temp.path(), "lucidos_orphan").unwrap();
        entry.phase = crate::trash::Phase::Moved;
        let mut json = serde_json::to_value(entry).unwrap();
        json.as_object_mut().unwrap().remove("drop_started");
        let legacy: crate::trash::DatabaseEntry = serde_json::from_value(json).unwrap();
        assert_eq!(legacy.drop_started, None);
        assert!(!legacy.can_cancel_without_database());
    }

    #[tokio::test]
    async fn resource_publication_lock_serializes_gateways_before_registration() {
        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared-home");
        let mut first = GatewayState::for_tests_with_static_dir(Some(temp.path().join("first")));
        let mut second = GatewayState::for_tests_with_static_dir(Some(temp.path().join("second")));
        Arc::get_mut(&mut first.inner).unwrap().snapshot_home = shared.clone();
        Arc::get_mut(&mut second.inner).unwrap().snapshot_home = shared;
        let held = first.resource_publication_lock().unwrap();
        assert!(second.resource_publication_lock().is_err());
        let result = second.create_workspace("Claim during cleanup").await;
        assert!(result.is_err());
        assert!(second.inner.registry.lock().unwrap().workspaces.is_empty());
        let dir = temp.path().join("orphan");
        std::fs::create_dir(&dir).unwrap();
        let result = second
            .adopt_workspace_with_cleanup(&dir, None, None, None, &Default::default(), false)
            .await;
        assert!(result.is_err());
        assert!(second.inner.registry.lock().unwrap().workspaces.is_empty());
        assert!(!dir.join(".lucidos").exists());
        let inherited_descriptor = held.0.try_clone().unwrap();
        drop(held);
        let held = second.resource_publication_lock().unwrap();
        drop(inherited_descriptor);
        assert!(first.resource_publication_lock().is_err());
        drop(held);
        second
            .adopt_workspace_with_cleanup(&dir, None, None, None, &Default::default(), false)
            .await
            .unwrap();
        assert_eq!(second.inner.registry.lock().unwrap().workspaces.len(), 1);
    }

    #[test]
    fn freeze_database_selection_needs_no_folder_or_running_observation() {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let mut report = crate::cleanup::Review {
            scanned_at: Some(chrono::Utc::now()),
            registered_databases: Some(Default::default()),
            ..Default::default()
        };
        crate::cleanup::merge_databases(
            &mut report,
            vec![crate::cleanup::DatabaseObservation {
                name: "lucidos_Legacy-name".into(),
                bytes: 4096,
                last_event: None,
                warning: None,
            }],
        );
        let row = &report.rows[0];
        assert!(row.running.is_none());
        let selection = Selection {
            id: row.id.clone(),
            fingerprint: row.fingerprint.clone().unwrap(),
        };
        state.inner.cleanup_review.lock().unwrap().report = report;
        let batch = state.freeze_cleanup(vec![selection]).unwrap();
        assert!(
            matches!(&batch.members[0].resource, FrozenResource::Database { database } if database == "lucidos_Legacy-name")
        );
        assert!(batch.members[0].retained_branches.is_empty());
    }

    #[test]
    fn freeze_folder_selection_uses_gateway_path_and_preserves_legacy_batches() {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let anchor = temp.path().join("anchor");
        let folder = temp.path().join("unregistered");
        for path in [&anchor, &folder] {
            std::fs::create_dir_all(path.join("data")).unwrap();
            std::fs::create_dir_all(path.join(".lucidos")).unwrap();
        }
        let mut ws = Workspace::gateway_provisioned("anchor".into(), "Anchor".into(), 9100);
        ws.dir = anchor.to_str().unwrap().into();
        let report = crate::cleanup::scan(vec![ws], state.app_data(), vec![]);
        let row = report
            .rows
            .iter()
            .find(|row| row.workspace.is_none())
            .unwrap();
        let selection = Selection {
            id: row.id.clone(),
            fingerprint: row.fingerprint.clone().unwrap(),
        };
        state.inner.cleanup_review.lock().unwrap().report = report;
        let batch = state.freeze_cleanup(vec![selection]).unwrap();
        match &batch.members[0].resource {
            FrozenResource::Folder { directory } => assert_eq!(
                directory.canonicalize().unwrap(),
                folder.canonicalize().unwrap()
            ),
            _ => panic!("folder selection fabricated a registration"),
        }
        let legacy = serde_json::json!({
            "id": "anchor", "fingerprint": "old",
            "workspace": Workspace::gateway_provisioned("anchor".into(), "Anchor".into(), 9100)
        });
        let parsed: FrozenMember = serde_json::from_value(legacy).unwrap();
        assert!(matches!(parsed.resource, FrozenResource::Registered { .. }));
    }

    #[tokio::test]
    async fn folder_execution_and_restore_never_register_or_call_postgres() {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let folder = temp.path().join("unregistered");
        std::fs::create_dir_all(folder.join(".lucidos")).unwrap();
        std::fs::write(folder.join("retained"), "unique content").unwrap();
        let fingerprint = crate::cleanup::folder_fingerprint(
            &folder,
            false,
            crate::cleanup::allocated_bytes(&folder).unwrap(),
        )
        .unwrap();
        state
            .delete_frozen_folder_with_peers(&folder, &fingerprint, &[], None)
            .await
            .unwrap();
        assert!(!folder.exists());
        let listing = crate::trash::list(state.app_data()).unwrap();
        assert_eq!(listing.len(), 1);
        assert!(listing[0].restore_available);
        state
            .restore_folder_trash_with_peers(&listing[0].id, &folder, &[])
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(folder.join("retained")).unwrap(),
            "unique content"
        );
        assert!(state.inner.registry.lock().unwrap().workspaces.is_empty());
    }

    #[tokio::test]
    async fn changed_running_and_peer_owned_folders_survive() {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let folder = temp.path().join("unregistered");
        std::fs::create_dir_all(folder.join(".lucidos")).unwrap();
        let fingerprint = crate::cleanup::folder_fingerprint(
            &folder,
            false,
            crate::cleanup::allocated_bytes(&folder).unwrap(),
        )
        .unwrap();
        std::fs::write(folder.join("new-work"), "retained").unwrap();
        assert!(state
            .delete_frozen_folder_with_peers(&folder, &fingerprint, &[], None)
            .await
            .is_err());
        // Parallel process-spawning tests can briefly inherit the preceding
        // lock before exec closes it. Acquire the fixture lock before testing
        // the production nonblocking refusal.
        let lock =
            lucidos_owner_lock::OwnerLock::acquire(&lucidos_owner_lock::engine::lock_path(&folder))
                .unwrap();
        assert!(state
            .delete_frozen_folder_with_peers(&folder, &fingerprint, &[], None)
            .await
            .is_err());
        drop(lock);
        let missing = temp.path().join("missing-peer.json");
        assert!(state
            .delete_frozen_folder_with_peers(&folder, &fingerprint, &[missing], None)
            .await
            .is_err());
        assert!(folder.join("new-work").exists());
        assert!(!state.app_data().join("deleted").exists());
    }

    #[test]
    fn confirmation_names_retained_branches_in_stable_order() {
        assert_eq!(
            confirmation_phrase(2, ["z", "a", "z"].into_iter()),
            "Delete 2 workspaces; keep branches: a, z"
        );
        assert_eq!(
            confirmation_phrase(1, std::iter::empty()),
            "Delete 1 workspaces"
        );
    }

    #[test]
    fn unknown_resources_require_individual_named_confirmation() {
        for resource in [
            FrozenResource::Folder {
                directory: PathBuf::from("/sample/folder"),
            },
            FrozenResource::Database {
                database: "lucidos_sample".into(),
            },
        ] {
            let members = vec![
                FrozenMember {
                    id: "first".into(),
                    fingerprint: "observed".into(),
                    resource,
                    retained_branches: vec![],
                },
                FrozenMember {
                    id: "second".into(),
                    fingerprint: "observed".into(),
                    resource: FrozenResource::Folder {
                        directory: PathBuf::from("/sample/second"),
                    },
                    retained_branches: vec![],
                },
            ];
            assert!(batch_confirmation_phrase(&members).is_err());
            let phrase = batch_confirmation_phrase(&members[..1]).unwrap();
            assert!(phrase == "Delete /sample/folder" || phrase == "Delete lucidos_sample");
        }
    }

    #[tokio::test]
    async fn recovered_member_finishes_as_skipped_without_database_effects() {
        for database in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
            let actor = crate::cleanup_audit::Actor::Device {
                device_id: "device".into(),
            };
            let source = temp.path().join("source");
            std::fs::create_dir(&source).unwrap();
            let (id, resource) = if database {
                (
                    "database:lucidos_sample",
                    FrozenResource::Database {
                        database: "lucidos_sample".into(),
                    },
                )
            } else {
                (
                    "folder",
                    FrozenResource::Folder {
                        directory: source.clone(),
                    },
                )
            };
            let audit =
                crate::cleanup_audit::Audit::begin(state.app_data(), id, actor.clone()).unwrap();
            let parent = state.app_data().join("deleted");
            if database {
                let (mut entry, root) =
                    crate::trash::DatabaseEntry::prepare(&parent, "lucidos_sample").unwrap();
                entry.audit = Some(audit.clone());
                entry.phase = crate::trash::Phase::Restored;
                entry.save(&root).unwrap();
            } else {
                let (mut entry, root) =
                    crate::trash::Entry::prepare_folder(&parent, source).unwrap();
                entry.audit = Some(audit.clone());
                entry.phase = crate::trash::Phase::Restored;
                entry.save(&root).unwrap();
            }
            let members = vec![FrozenMember {
                id: id.into(),
                fingerprint: "old".into(),
                resource,
                retained_branches: vec![],
            }];
            let batch = Batch {
                id: "batch-test".into(),
                phrase: batch_confirmation_phrase(&members).unwrap(),
                members,
                results: vec![],
                actor: Some(actor),
                audits: [(id.into(), audit)].into_iter().collect(),
            };
            let root = batch_root(state.app_data(), &batch.id).unwrap();
            std::fs::create_dir_all(&root).unwrap();
            save(&batch, &root).unwrap();
            let result = state
                .confirm_cleanup_with_observation(&batch.id, &batch.phrase, "device", |_| {
                    std::future::ready(Err(
                        "recovered deletion must not observe the database".into()
                    ))
                })
                .await
                .unwrap();
            assert_eq!(result.results.len(), 1);
            assert_eq!(result.results[0].status, "skipped");
        }
    }

    #[test]
    fn protected_workspaces_require_separate_named_confirmation() {
        for origin in [
            crate::registry::WorkspaceOrigin::User,
            crate::registry::WorkspaceOrigin::Unknown,
        ] {
            let mut workspace =
                Workspace::gateway_provisioned("sample".into(), "My workspace".into(), 9100);
            workspace.cleanup.origin = origin;
            let member = FrozenMember {
                id: workspace.id.clone(),
                fingerprint: "observed".into(),
                resource: FrozenResource::Registered { workspace },
                retained_branches: vec!["saved".into()],
            };
            let mut members = vec![member];
            assert_eq!(
                batch_confirmation_phrase(&members).unwrap(),
                "Delete My workspace; keep branches: saved"
            );
            members.push(FrozenMember {
                id: "folder".into(),
                fingerprint: "observed".into(),
                resource: FrozenResource::Folder {
                    directory: PathBuf::from("folder"),
                },
                retained_branches: vec![],
            });
            assert!(batch_confirmation_phrase(&members).is_err());
        }
    }

    #[tokio::test]
    async fn generic_protected_confirmation_refuses_before_authorization_or_observation() {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let root = batch_root(state.app_data(), "batch-test").unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let batch = Batch {
            id: "batch-test".into(),
            phrase: "Delete 1 workspaces".into(),
            members: vec![FrozenMember {
                id: "sample".into(),
                fingerprint: "observed".into(),
                resource: FrozenResource::Registered {
                    workspace: Workspace::gateway_provisioned(
                        "sample".into(),
                        "Protected".into(),
                        9100,
                    ),
                },
                retained_branches: vec![],
            }],
            results: vec![],
            actor: None,
            audits: Default::default(),
        };
        save(&batch, &root).unwrap();
        let before = std::fs::read(root.join("batch.json")).unwrap();
        let mut observed = false;
        let error = state
            .confirm_cleanup_with_observation("batch-test", &batch.phrase, "device", |_| {
                observed = true;
                std::future::ready(Ok(false))
            })
            .await
            .err()
            .unwrap();
        assert_eq!(
            error.into_response().status(),
            axum::http::StatusCode::BAD_REQUEST
        );
        assert!(!observed);
        assert_eq!(std::fs::read(root.join("batch.json")).unwrap(), before);
        assert!(!state.app_data().join("cleanup-events").exists());
        assert!(!state.app_data().join("deleted").exists());
    }

    #[tokio::test]
    async fn changed_frozen_branch_names_are_refused_before_database_access() {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let mut ws = Workspace::gateway_provisioned("scratch".into(), "Scratch".into(), 9100);
        ws.dir = temp.path().to_str().unwrap().into();
        let error = state
            .cleanup_current_fingerprint(&ws, vec!["previous-branch".into()])
            .await
            .unwrap_err();
        assert!(error.to_string().contains("branches changed"));
    }

    #[tokio::test]
    async fn retry_recovers_completion_after_trash_and_batch_result_were_lost() {
        for completion_source in ["purged", "terminal-journal", "corrupt-event"] {
            let temp = tempfile::tempdir().unwrap();
            let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
            let root = batch_root(state.app_data(), "batch-test").unwrap();
            std::fs::create_dir_all(&root).unwrap();
            let actor = crate::cleanup_audit::Actor::Device {
                device_id: "first-device".into(),
            };
            let audit =
                crate::cleanup_audit::Audit::begin(state.app_data(), "scratch", actor.clone())
                    .unwrap();
            let batch = Batch {
                id: "batch-test".into(),
                phrase: "Delete Scratch".into(),
                members: vec![FrozenMember {
                    id: "scratch".into(),
                    fingerprint: "old".into(),
                    resource: FrozenResource::Registered {
                        workspace: Workspace::gateway_provisioned(
                            "scratch".into(),
                            "Scratch".into(),
                            9100,
                        ),
                    },
                    retained_branches: vec![],
                }],
                results: vec![],
                actor: Some(actor),
                audits: [("scratch".into(), audit.clone())].into_iter().collect(),
            };
            save(&batch, &root).unwrap();
            if completion_source == "terminal-journal" {
                let source = temp.path().join("source");
                std::fs::create_dir(&source).unwrap();
                let FrozenResource::Registered { workspace } = &batch.members[0].resource else {
                    panic!("registered fixture")
                };
                let (mut entry, journal_root) = crate::trash::Entry::prepare(
                    &state.app_data().join("deleted"),
                    workspace,
                    source,
                    |_| Ok(()),
                )
                .unwrap();
                entry.audit = Some(audit.clone());
                entry.move_folder(&journal_root).unwrap();
                entry.phase = crate::trash::Phase::Completed;
                // Simulate the crash between terminal journal sync and event publication.
                std::fs::write(
                    journal_root.join("entry.json"),
                    serde_json::to_vec(&entry).unwrap(),
                )
                .unwrap();
                assert!(!audit.is_completed(state.app_data()).unwrap());
            } else {
                audit.completed(state.app_data()).unwrap();
            }
            if completion_source == "corrupt-event" {
                for event in std::fs::read_dir(state.app_data().join("cleanup-events")).unwrap() {
                    let path = event.unwrap().path();
                    if path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .contains("WorkspaceDeleted")
                    {
                        std::fs::write(path, b"{}").unwrap();
                    }
                }
                assert!(state
                    .confirm_cleanup("batch-test", "Delete Scratch", "second-device")
                    .await
                    .is_err());
                let stored: Batch =
                    serde_json::from_reader(std::fs::File::open(root.join("batch.json")).unwrap())
                        .unwrap();
                assert!(stored.results.is_empty());
                assert_eq!(stored.audits["scratch"], audit);
                continue;
            }
            // No registration survives. Retry must use proven completion, not absence.
            let result = state
                .confirm_cleanup("batch-test", "Delete Scratch", "second-device")
                .await
                .unwrap();
            assert_eq!(result.results[0].status, "deleted");
            assert_eq!(result.audits["scratch"], audit);
            let stored: Batch =
                serde_json::from_reader(std::fs::File::open(root.join("batch.json")).unwrap())
                    .unwrap();
            assert_eq!(stored.results[0].status, "deleted");
            assert_eq!(stored.audits["scratch"], audit);
            assert!(serde_json::to_value(result.public())
                .unwrap()
                .get("audits")
                .is_none());
            assert_eq!(
                std::fs::read_dir(state.app_data().join("cleanup-events"))
                    .unwrap()
                    .count(),
                2
            );
        }
    }

    #[tokio::test]
    async fn changed_member_is_skipped_without_database_access_and_result_is_durable() {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let mut ws = Workspace::gateway_provisioned("scratch".into(), "Scratch".into(), 9100);
        let root = batch_root(state.app_data(), "batch-test").unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let batch = Batch {
            id: "batch-test".into(),
            phrase: "Delete Scratch".into(),
            members: vec![FrozenMember {
                id: ws.id.clone(),
                fingerprint: "old".into(),
                resource: FrozenResource::Registered {
                    workspace: ws.clone(),
                },
                retained_branches: vec![],
            }],
            results: vec![],
            actor: None,
            audits: Default::default(),
        };
        save(&batch, &root).unwrap();
        ws.name = "Renamed".into();
        state.inner.registry.lock().unwrap().workspaces.push(ws);
        let result = state
            .confirm_cleanup("batch-test", "Delete Scratch", "test-device")
            .await
            .unwrap();
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].status, "skipped");
        let saved: Batch =
            serde_json::from_reader(std::fs::File::open(root.join("batch.json")).unwrap()).unwrap();
        assert_eq!(saved.results[0].status, "skipped");
        assert_eq!(
            saved.actor,
            Some(crate::cleanup_audit::Actor::Device {
                device_id: "test-device".into(),
            })
        );
        let repeated = state
            .confirm_cleanup("batch-test", "Delete Scratch", "another-device")
            .await
            .unwrap();
        assert_eq!(repeated.actor, saved.actor);
        assert_eq!(repeated.results.len(), 1);
        let events: Vec<_> = std::fs::read_dir(state.app_data().join("cleanup-events"))
            .unwrap()
            .map(|entry| {
                serde_json::from_slice::<serde_json::Value>(
                    &std::fs::read(entry.unwrap().path()).unwrap(),
                )
                .unwrap()
            })
            .collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["event_type"], "WorkspaceDeletionAuthorized");
        assert_eq!(events[0]["actor"]["device_id"], "test-device");
        assert_eq!(state.inner.registry.lock().unwrap().workspaces.len(), 1);
    }
    #[tokio::test]
    async fn batch_routes_reject_local_and_agent_credentials_without_writes() {
        use tower::ServiceExt as _;
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        for uri in [
            "/cleanup/batches",
            "/cleanup/batches/batch-test/confirm",
            "/trash/workspace-test/restore",
            "/trash/empty-batches",
            "/trash/empty-batches/empty-test/confirm",
        ] {
            for agent in [false, true] {
                let mut request = axum::http::Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"members":[],"confirm":"Delete 0 workspaces"}"#,
                    ))
                    .unwrap();
                if agent {
                    request.extensions_mut().insert(auth::AuthenticatedDevice {
                        id: "test".into(),
                        label: "Test device".into(),
                    });
                    request.headers_mut().insert(
                        "x-lucidos-agent-origin-token",
                        HeaderValue::from_static("subprocess"),
                    );
                }
                let response = crate::control::router()
                    .with_state(state.clone())
                    .oneshot(request)
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::FORBIDDEN);
                assert!(!state.app_data().join("cleanup-batches").exists());
                assert!(!state.app_data().join("deleted").exists());
            }
        }
    }

    #[test]
    fn batch_paths_cannot_escape_storage() {
        for id in ["../batch-test", "batch-../x", "/batch-x", "batch-x.json"] {
            assert!(batch_root(Path::new("/tmp/example"), id).is_err());
        }
    }

    #[tokio::test]
    async fn paired_device_cannot_bypass_frozen_cleanup_with_direct_delete() {
        use tower::ServiceExt as _;
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let mut request = axum::http::Request::builder()
            .method("DELETE")
            .uri("/workspaces/sample")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"confirm":"Sample"}"#))
            .unwrap();
        request.extensions_mut().insert(auth::AuthenticatedDevice {
            id: "test".into(),
            label: "Test device".into(),
        });
        let response = crate::control::router()
            .with_state(state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("frozen cleanup batch"));
        assert!(!state.app_data().join("deleted").exists());
    }
}
