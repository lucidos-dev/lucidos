//! Offline runner preparation. Retain Session (and both locks) across all steps.
use super::Session;
use crate::core::snapshot::{
    file_swap::{FileSwap, FrozenPolicy},
    journal::{Journal, Phase, PreparedFiles},
    restore_files, BoxError,
};
use std::path::Path;
use uuid::Uuid;

impl Session {
    /// Read-only admission checks while the engine still owns the workspace.
    /// The offline worker repeats them after shutdown to close the mutation race.
    pub fn preflight_restore(
        &self,
        handle: &super::Handle,
        database_url: &str,
        workspace: &Path,
    ) -> Result<(), BoxError> {
        (|| {
            if self
                .store
                .journals()?
                .iter()
                .any(|journal| !journal.phase().is_terminal() || journal.phase().blocks_engine())
            {
                return Err("snapshot_restore_already_pending".into());
            }
            let manifest = self.check(handle, database_url)?;
            self.preflight_restore_contract(database_url, &manifest)?;
            FrozenPolicy::capture(workspace)?;
            Ok(())
        })()
        .map_err(|_: BoxError| "snapshot_restore_preflight_failed".into())
    }

    pub fn admit_restore(&self, journal: &Journal) -> Result<(), BoxError> {
        if journal.phase() != Phase::Requested
            || self
                .store
                .journals()?
                .iter()
                .any(|existing| !existing.phase().is_terminal() || existing.phase().blocks_engine())
        {
            return Err("snapshot_restore_admission_failed".into());
        }
        self.store
            .save_journal(journal)
            .map_err(|_| "snapshot_restore_admission_failed".into())
    }

    /// Settle an admitted restore the gateway provably never took. A lost
    /// delivery must not call this: the offline run may already own the journal.
    pub fn refuse_restore(&self, restore_id: Uuid) -> Result<(), BoxError> {
        let mut journal = self.restore_journal(restore_id)?;
        journal.advance(Phase::Refused)?;
        self.store.save_journal(&journal)
    }

    /// Enable the offline command's machine-readable, fixed-vocabulary protocol.
    pub fn with_restore_reporting(mut self) -> Self {
        self.report_restore = true;
        self
    }

    fn report(&self, phase: &'static str) {
        if self.report_restore {
            // CLI protocol consumed by the gateway. Never format errors, paths,
            // caller labels, database URLs or credentials into these lines.
            use std::io::Write;
            // A disconnected observer cannot cancel a restore. The external
            // journal is authoritative; a broken output pipe is unrecoverable.
            let _ = writeln!(std::io::stdout().lock(), "{phase}");
        }
    }

    pub(super) fn fault(&self, _boundary: &'static str) -> Result<(), BoxError> {
        #[cfg(test)]
        if self.faults.lock().unwrap().remove(_boundary) {
            return Err("snapshot_injected_failure".into());
        }
        Ok(())
    }

    /// Prepare either forward installation or recovery with the same durable
    /// preservation policy. No live file or database mutation occurs here.
    /// On return, every retained staging/trash root has been fsynced into the
    /// external journal. Only the offline runner may use the returned swap.
    pub fn prepare_restore_files(
        &self,
        restore_id: Uuid,
        workspace: &Path,
    ) -> Result<FileSwap, BoxError> {
        self.prepare_restore_files_inner(restore_id, workspace)
            .map_err(|_| "snapshot_restore_preparation_failed".into())
    }

    fn prepare_restore_files_inner(
        &self,
        restore_id: Uuid,
        workspace: &Path,
    ) -> Result<FileSwap, BoxError> {
        let mut journal = self.restore_journal(restore_id)?;
        if !matches!(journal.phase(), Phase::Staging | Phase::RollingBack) {
            return Err("snapshot_restore_phase_refused".into());
        }
        let recovery = journal
            .recovery
            .as_ref()
            .ok_or("snapshot recovery missing")?;
        // Refuse changed recovery identity even if the caller supplied a valid
        // but unrelated recovery handle.
        let recovery_manifest = self.store.get(recovery, &self.key)?;
        if recovery_manifest.restore_id != Some(restore_id)
            || recovery_manifest.kind != crate::core::snapshot::manifest::Kind::Recovery
        {
            return Err("snapshot recovery identity mismatch".into());
        }
        if journal.files.is_none() {
            if journal.phase() != Phase::Staging {
                return Err("snapshot original preservation policy missing".into());
            }
            journal.files = Some(PreparedFiles {
                policy: FrozenPolicy::capture(workspace)?,
                staging_roots: Vec::new(),
            });
            // Persist before even preparing replacement files. A crash never
            // falls back to the archive's .backupignore during rollback.
            self.store.save_journal(&journal)?;
        }
        let handle = if journal.phase() == Phase::RollingBack {
            recovery
        } else {
            &journal.snapshot
        };
        let manifest = self.store.get(handle, &self.key)?;
        let archive = self.store.snapshot_dir(handle);
        let root = tempfile::Builder::new()
            .prefix(super::store::RESTORE_STAGING_PREFIX)
            .tempdir_in(self.store.root().join(".partial"))?
            .keep();
        let files = journal
            .files
            .as_mut()
            .ok_or("snapshot preservation policy missing")?;
        files.staging_roots.push(root.clone());
        self.store.save_journal(&journal)?;
        let staged = root.join("files");
        restore_files::stage(&archive, &manifest, &staged)?;
        let files = journal
            .files
            .as_mut()
            .ok_or("snapshot preservation policy missing")?;
        let swap =
            FileSwap::prepare_with_policy(&archive, &manifest, &staged, workspace, &files.policy)?;
        files
            .staging_roots
            .extend(swap.staging_roots.iter().cloned());
        self.store.save_journal(&journal)?;
        Ok(swap)
    }

    fn restore_journal(&self, restore_id: Uuid) -> Result<Journal, BoxError> {
        self.store
            .journals()?
            .into_iter()
            .find(|journal| journal.restore_id == restore_id)
            .ok_or_else(|| "snapshot_restore_journal_unavailable".into())
    }
}

impl Session {
    /// Offline entry point. `gateway_slug` must come from the gateway registry,
    /// whose workspace path and database binding the caller has already checked.
    /// An owned task retains the engine owner lock through recovery capture and
    /// installation, even when the requesting task is canceled.
    pub async fn run_restore(
        self: std::sync::Arc<Self>,
        pool: &sqlx::PgPool,
        database_url: &str,
        workspace: &Path,
        request: super::OfflineRestoreRequest,
    ) -> Result<(), BoxError> {
        let pool = pool.clone();
        let database_url = database_url.to_owned();
        let workspace = workspace.to_owned();
        let super::OfflineRestoreRequest {
            restore_id,
            gateway_slug,
            workspace_name,
        } = request;
        tokio::spawn(async move {
            let worker = self.clone();
            let target = workspace.clone();
            let url = database_url.clone();
            let (owner, expanded, incoming, current) = tokio::task::spawn_blocking(move || {
                if crate::core::snapshot::identity::workspace_id(
                    Some(&gateway_slug),
                    &worker.database,
                )? != worker.store.workspace_id
                    || crate::core::secret_store::boot::database_name(&url)? != worker.database
                {
                    return Err::<_, BoxError>("snapshot_restore_binding_mismatch".into());
                }
                let owner = lucidos_owner_lock::OwnerLock::try_acquire(
                    &lucidos_owner_lock::engine::lock_path(&target),
                )
                .map_err(|_| "snapshot_workspace_lock_failed")?
                .ok_or("snapshot_workspace_still_running")?;
                let mut journal = worker.restore_journal(restore_id)?;
                if journal.phase() == Phase::Requested {
                    journal.advance(Phase::RecoveryCapture)?;
                    worker.store.save_journal(&journal)?;
                }
                if journal.phase() != Phase::RecoveryCapture || journal.recovery.is_some() {
                    return Err("snapshot_restore_phase_refused".into());
                }
                let manifest = worker.store.get(&journal.snapshot, &worker.key)?;
                worker.preflight_restore_contract(&url, &manifest)?;
                worker.check(&journal.snapshot, &url)?;
                // Refuse unsafe filesystem bindings before creating recovery.
                FrozenPolicy::capture(&target)?;
                let current = crate::core::backup::snapshot_file_estimate(&target)?;
                let expanded = super::super::database_capacity::expanded_dump_bytes(
                    &worker.store.snapshot_dir(&journal.snapshot).join("db.dump"),
                    &url,
                )?;
                Ok((owner, expanded, manifest.files.bytes, current))
            })
            .await
            .map_err(|_| "snapshot_restore_worker_failed")??;
            let actual_database: String = sqlx::query_scalar("SELECT current_database()")
                .fetch_one(&pool)
                .await
                .map_err(|_| "snapshot_restore_database_unavailable")?;
            if actual_database != self.database {
                return Err("snapshot_restore_binding_mismatch".into());
            }
            let database_capacity =
                super::super::database_capacity::DatabaseCapacity::discover(&pool, expanded)
                    .await?;
            let worker = self.clone();
            let target = workspace.clone();
            let url = database_url.clone();
            tokio::task::spawn_blocking(move || {
                super::super::capacity::preflight_database(
                    worker.store.root(),
                    &target,
                    incoming,
                    current,
                    Some((&database_capacity, &url)),
                )
            })
            .await
            .map_err(|_| "snapshot_restore_worker_failed")??;
            self.report("recovery-capture");
            self.clone()
                .prepare_recovery(&pool, &database_url, &workspace, restore_id, workspace_name)
                .await?;
            let worker = self.clone();
            let target = workspace.clone();
            let url = database_url.clone();
            let (owner, installed) = tokio::task::spawn_blocking(move || {
                let installed = worker.install_restore_inner(restore_id, &target, &url);
                (owner, installed)
            })
            .await
            .map_err(|_| "snapshot_restore_worker_failed")?;
            // Keep the owner lock across the restricted boot, including rollback.
            let _owner = owner;
            if !matches!(
                self.restore_journal(restore_id)?.phase(),
                Phase::FilesRestored | Phase::RollingBack
            ) {
                return installed;
            }
            let recovering = self.restore_journal(restore_id)?.phase() == Phase::RollingBack;
            self.report("first-boot");
            let marker_valid = if recovering {
                true
            } else {
                let worker = self.clone();
                let target = workspace.clone();
                tokio::task::spawn_blocking(move || {
                    super::super::first_boot::pending_restore(&target, &worker.store).is_ok_and(
                        |pending| {
                            pending.is_some_and(|(marker, _)| marker.restore_id == restore_id)
                        },
                    )
                })
                .await
                .unwrap_or(false)
            };
            if self
                .fault(if recovering {
                    "rollback-boot"
                } else {
                    "forward-boot"
                })
                .is_err()
                || !marker_valid
                || super::super::first_boot::verify_owned(pool.clone(), self.database.clone())
                    .await
                    .is_err()
            {
                if recovering {
                    let mut journal = self.restore_journal(restore_id)?;
                    journal.advance(Phase::NeedsAttention)?;
                    self.store.save_journal(&journal)?;
                    self.report("needs-attention");
                    return Err("snapshot_restore_recovery_boot_failed".into());
                }
                let worker = self.clone();
                tokio::task::spawn_blocking(move || {
                    worker.rollback_restore_inner(restore_id, &workspace, &database_url)
                })
                .await
                .map_err(|_| "snapshot_restore_worker_failed")??;
                if self.fault("rollback-boot").is_err()
                    || super::super::first_boot::verify_owned(pool.clone(), self.database.clone())
                        .await
                        .is_err()
                {
                    let mut journal = self.restore_journal(restore_id)?;
                    journal.advance(Phase::NeedsAttention)?;
                    self.store.save_journal(&journal)?;
                    self.report("needs-attention");
                    return Err("snapshot_restore_recovery_boot_failed".into());
                }
                let mut journal = self.restore_journal(restore_id)?;
                journal.advance(Phase::RolledBack)?;
                self.store.save_journal(&journal)?;
                self.report("rolled-back");
                return Err("snapshot_restore_rolled_back".into());
            }
            let mut journal = self.restore_journal(restore_id)?;
            if journal.phase() == Phase::RollingBack {
                journal.advance(Phase::RolledBack)?;
                self.store.save_journal(&journal)?;
                self.report("rolled-back");
            } else {
                self.report("files-restored");
            }
            // Forward boot stays blocked until restored-log settlement is wired.
            installed
        })
        .await
        .map_err(|_| "snapshot_restore_worker_failed")?
        .map_err(|_| "snapshot_restore_failed".into())
    }

    fn install_restore_inner(
        &self,
        restore_id: Uuid,
        workspace: &Path,
        database_url: &str,
    ) -> Result<(), BoxError> {
        let mut journal = self.restore_journal(restore_id)?;
        if journal.phase() != Phase::Staging {
            return Err("snapshot_restore_phase_refused".into());
        }
        let manifest = self.store.get(&journal.snapshot, &self.key)?;
        self.preflight_restore_contract(database_url, &manifest)?;
        crate::core::snapshot::check::check(
            &self.store,
            &journal.snapshot,
            &self.key,
            database_url,
        )?;
        let recovery = journal
            .recovery
            .as_ref()
            .ok_or("snapshot recovery missing")?;
        let recovery_manifest =
            crate::core::snapshot::check::check(&self.store, recovery, &self.key, database_url)?;
        self.preflight_restore_contract(database_url, &recovery_manifest)?;
        self.report("staging");
        let swap = self.prepare_restore_files(restore_id, workspace)?;
        // Preparation has appended retained roots and the frozen policy.
        journal = self.restore_journal(restore_id)?;
        crate::core::snapshot::worktree_exemptions::preserve(self.store.root(), workspace)?;
        journal.advance(Phase::DbRestore)?;
        self.store.save_journal(&journal)?;
        self.report("restore-archive");
        let result = (|| {
            self.fault("before-database")?;
            crate::core::snapshot::restore_database::restore(
                database_url,
                &self.store.snapshot_dir(&journal.snapshot).join("db.dump"),
                &manifest.table_rows,
            )?;
            self.fault("after-database")?;
            journal.advance(Phase::FileSwap)?;
            self.store.save_journal(&journal)?;
            self.report("file-swap");
            swap.apply()?;
            self.fault("after-files")?;
            super::super::first_boot::write_marker(workspace, &journal)?;
            self.fault("after-marker")?;
            journal.advance(Phase::FilesRestored)?;
            self.store.save_journal(&journal)
        })();
        if result.is_err() {
            // Even an uncertain transaction outcome must replay recovery; never
            // mark unchanged merely because pg_restore reported failure.
            self.rollback_restore_inner(restore_id, workspace, database_url)?;
            return Err("snapshot_restore_rolled_back".into());
        }
        Ok(())
    }

    /// Settle interrupted preparation without replay; a possibly committed
    /// installation always restores the verified recovery snapshot instead.
    pub async fn reconcile_interrupted_restore(
        self: std::sync::Arc<Self>,
        restore_id: Uuid,
        workspace: &Path,
        database_url: &str,
    ) -> Result<(), BoxError> {
        let worker = self.clone();
        let target = workspace.to_owned();
        let rollback = tokio::task::spawn_blocking(move || {
            let _owner = lucidos_owner_lock::OwnerLock::try_acquire(
                &lucidos_owner_lock::engine::lock_path(&target),
            )?
            .ok_or("snapshot_workspace_still_running")?;
            let mut journal = worker.restore_journal(restore_id)?;
            let next = match journal.phase() {
                Phase::Requested => Some(Phase::Refused),
                Phase::RecoveryCapture | Phase::Staging => Some(Phase::FailedUnchanged),
                Phase::DbRestore | Phase::FileSwap | Phase::FilesRestored | Phase::RollingBack => {
                    return Ok(true);
                }
                Phase::NeedsAttention => {
                    return Err::<_, BoxError>("snapshot_restore_needs_attention".into())
                }
                Phase::Refused | Phase::Completed | Phase::FailedUnchanged | Phase::RolledBack => {
                    None
                }
            };
            if let Some(next) = next {
                journal.advance(next)?;
                worker.store.save_journal(&journal)?;
            }
            Ok(false)
        })
        .await
        .map_err(|_| "snapshot_restore_worker_failed")??;
        if rollback {
            // The durable mutation phase still blocks every normal engine boot
            // while rollback acquires the owner lock for the complete replay.
            self.rollback_restore(restore_id, workspace, database_url)
                .await?;
        }
        Ok(())
    }

    /// Gateway crash recovery uses this same path. It never recaptures policy
    /// or recovery from a partly restored workspace.
    pub async fn rollback_restore(
        self: std::sync::Arc<Self>,
        restore_id: Uuid,
        workspace: &Path,
        database_url: &str,
    ) -> Result<(), BoxError> {
        let workspace = workspace.to_owned();
        let database_url = database_url.to_owned();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let _owner = lucidos_owner_lock::OwnerLock::try_acquire(
                &lucidos_owner_lock::engine::lock_path(&workspace),
            )
            .map_err(|_| "snapshot_workspace_lock_failed")?
            .ok_or("snapshot_workspace_still_running")?;
            self.rollback_restore_inner(restore_id, &workspace, &database_url)?;
            let verified = runtime.block_on(async {
                let pool = sqlx::postgres::PgPoolOptions::new()
                    .max_connections(2)
                    .connect(&database_url)
                    .await?;
                let keys = crate::core::secret_store::key_store::resolve()?;
                let result =
                    super::super::first_boot::verify(&pool, &self.database, keys.as_ref()).await;
                pool.close().await;
                result
            });
            let mut journal = self.restore_journal(restore_id)?;
            journal.advance(if verified.is_ok() {
                Phase::RolledBack
            } else {
                Phase::NeedsAttention
            })?;
            self.store.save_journal(&journal)?;
            verified.map_err(|_| "snapshot_restore_rollback_failed".into())
        })
        .await
        .map_err(|_| "snapshot_restore_worker_failed")?
    }

    fn rollback_restore_inner(
        &self,
        restore_id: Uuid,
        workspace: &Path,
        database_url: &str,
    ) -> Result<(), BoxError> {
        self.report("rolling-back");
        let mut journal = self.restore_journal(restore_id)?;
        if journal.phase() != Phase::RollingBack {
            journal.advance(Phase::RollingBack)?;
            self.store.save_journal(&journal)?;
        }
        let result = (|| {
            let handle = journal
                .recovery
                .as_ref()
                .ok_or("snapshot recovery missing")?;
            let manifest = self.store.get(handle, &self.key)?;
            self.preflight_restore_contract(database_url, &manifest)?;
            crate::core::snapshot::check::check(&self.store, handle, &self.key, database_url)?;
            let swap = self.prepare_restore_files(restore_id, workspace)?;
            crate::core::snapshot::restore_database::restore(
                database_url,
                &self.store.snapshot_dir(handle).join("db.dump"),
                &manifest.table_rows,
            )?;
            swap.apply()?;
            super::super::first_boot::remove_marker(workspace)?;
            Ok::<_, BoxError>(())
        })();
        // Re-read roots added by rollback preparation before saving status.
        journal = self.restore_journal(restore_id)?;
        // Access staging still needs restricted boot verification. Keep the engine blocked
        // until that step verifies equality; do not claim RolledBack here.
        if result.is_err() {
            journal.advance(Phase::NeedsAttention)?;
            self.store.save_journal(&journal)?;
        }
        result
    }

    fn preflight_restore_contract(
        &self,
        database_url: &str,
        manifest: &crate::core::snapshot::manifest::Manifest,
    ) -> Result<(), BoxError> {
        use crate::core::secret_store::{boot::database_name, key_store};
        if database_name(database_url)? != self.database {
            return Err("snapshot_database_binding_mismatch".into());
        }
        let latest_schema = sqlx::migrate!()
            .iter()
            .map(|migration| migration.version)
            .max()
            .ok_or("snapshot_schema_unavailable")?;
        if manifest.schema_version > latest_schema {
            return Err("snapshot_schema_newer_than_engine".into());
        }
        if manifest.secret_key_ids.len() > 1 {
            return Err("snapshot secret key contract invalid".into());
        }
        if let Some(id) = manifest.secret_key_ids.iter().next() {
            let name = key_store::secrets_key_name(&database_name(database_url)?);
            let keys = key_store::resolve()?;
            let active = keys.get(&name)?;
            if active.as_ref().is_none_or(|key| key.id() != *id) {
                let parked = keys.get(&key_store::parked_key_name(&name, id))?;
                if parked.as_ref().is_none_or(|key| key.id() != *id) {
                    return Err("snapshot_secret_key_unavailable".into());
                }
            }
        }
        Ok(())
    }
}
