//! The credential boundary shared by snapshot callers. Detailed errors from
//! archive primitives never leave here, including on join errors. Each one is
//! logged redacted, and callers see only a slug.
use super::{
    check,
    handle::Handle,
    manifest::{
        AuthenticatedManifest, Component, ComponentName, Consistency, Files, Kind, Manifest,
        Verification,
    },
    store::{self, Store, StoreLock},
    ArchiveJobGuard, ArchiveJobKind, ArchiveJobSlot, BoxError,
};
use crate::{
    core::{
        backup,
        secret_store::{key_store::KeyStore, SecretKey},
    },
    engine::thread_events::MessageOrigin,
};
use chrono::Utc;
use sqlx::PgPool;
use std::path::Path;

/// Log a failed step's cause, redacted, and return the slug callers see.
fn step_failed<E: std::fmt::Display>(
    step: &'static str,
    slug: &'static str,
) -> impl FnOnce(E) -> BoxError {
    move |error| {
        crate::log!(
            "[Snapshots] {step} failed: {}",
            crate::core::redact_postgres_secrets(&error.to_string())
        );
        slug.into()
    }
}

/// A floor refusal tells the user how much to free. Any other error stays opaque.
fn capture_failure(error: &BoxError) -> super::jobs::Failure {
    match error.downcast_ref::<store::DiskFloorRefused>() {
        Some(refusal) => super::jobs::Failure::DiskFloor {
            needed_bytes: refusal.needed_bytes,
        },
        None => super::jobs::Failure::CaptureFailed,
    }
}

/// Own both locks for the entire operation, including MAC key creation.
/// Capture tasks retain ownership if their caller stops waiting.
pub struct Session {
    store: Store,
    key: SecretKey,
    database: String,
    events: Option<super::events::JobEvents>,
    report_restore: bool,
    restore_generation: Option<uuid::Uuid>,
    #[cfg(test)]
    faults: std::sync::Mutex<std::collections::BTreeSet<&'static str>>,
    _lock: StoreLock,
    _slot: ArchiveJobGuard,
}

#[path = "restore_runner.rs"]
mod restore_runner;

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;

pub struct CaptureRequest {
    pub handle: Handle,
    pub workspace_name: String,
    pub actor: MessageOrigin,
    pub label: Option<String>,
    pub keep: bool,
    pub running_threads: u64,
}

/// A request whose target identity and display name came from the gateway registry.
/// The offline command must also verify that registry's directory/database binding.
pub struct OfflineRestoreRequest {
    pub restore_id: uuid::Uuid,
    pub gateway_slug: String,
    pub workspace_name: String,
}

/// Only the offline restore runner may request this, after stopping the engine.
/// Handles are minted internally so recovery cannot overwrite a caller's snapshot.
pub struct RecoveryRequest {
    pub restore_id: uuid::Uuid,
    pub workspace_name: String,
    pub actor: MessageOrigin,
}

#[derive(Clone, Copy)]
enum CaptureMode {
    Manual,
    Recovery(uuid::Uuid),
}

impl Session {
    /// Attach announcements and retention policy before sharing the session.
    pub fn with_events(mut self, events: super::events::JobEvents) -> Self {
        self.events = Some(events);
        self
    }

    /// Bind online job creation and restart discovery to the durable log identity.
    pub(crate) fn with_restore_generation(mut self, generation: Option<uuid::Uuid>) -> Self {
        self.restore_generation = generation;
        self
    }

    async fn complete_job(
        self: &std::sync::Arc<Self>,
        id: uuid::Uuid,
        state: super::jobs::State,
    ) -> Result<super::jobs::Job, BoxError> {
        let worker = self.clone();
        let job = tokio::task::spawn_blocking(move || worker.finish_job(id, state))
            .await
            .map_err(|_| "snapshot_job_worker_failed")??;
        if let Some(events) = &self.events {
            events.outcome(&job).await?;
            if job.action == super::jobs::Action::Capture
                && matches!(job.state, super::jobs::State::Completed { .. })
            {
                let bus = events.bus.clone();
                let workspace_id = job.workspace_id.clone();
                let actor = job.actor.clone();
                self.clone()
                    .prune(
                        events.manual_retention,
                        events.recovery_retention,
                        move |handle| {
                            let bus = bus.clone();
                            let workspace_id = workspace_id.clone();
                            let actor = actor.clone();
                            async move {
                                bus.emit(crate::engine::event_bus::BusEvent::System(
                                    crate::engine::event_bus::SystemEvent::SnapshotDeleted {
                                        workspace_id,
                                        handle,
                                        actor: Some(actor),
                                        reason: super::events::DeletionReason::Retention,
                                    },
                                ))
                                .await
                                .map_err(|_| "snapshot_event_failed")?;
                                Ok(())
                            }
                        },
                    )
                    .await?;
            }
        }
        Ok(job)
    }

    /// Run before accepting archive requests. An unused store creates no key or directory.
    pub async fn recover_at_boot(
        database_url: &str,
        gateway_slug: Option<String>,
        slots: ArchiveJobSlot,
        bus: crate::engine::event_bus::EventBus,
    ) -> Result<(), BoxError> {
        let generation = super::boundary::read(bus.pool())
            .await?
            .map(|b| b.restore_id);
        let database_url = database_url.to_owned();
        let recovered = tokio::task::spawn_blocking(move || {
            let database = crate::core::secret_store::boot::database_name(&database_url)
                .map_err(|_| "snapshot_store_unavailable")?;
            let root = Store::database_root(&database).map_err(|_| "snapshot_store_unavailable")?;
            if !root
                .try_exists()
                .map_err(|_| "snapshot_store_unavailable")?
            {
                return Ok::<_, BoxError>(None);
            }
            let session = Self::open_for_database(
                &database_url,
                gateway_slug.as_deref(),
                &slots,
                ArchiveJobKind::SnapshotCapture,
            )?
            .with_restore_generation(generation);
            let jobs = session.recover_jobs()?;
            Ok(Some((session, jobs)))
        })
        .await
        .map_err(|_| "snapshot_job_recovery_failed")??;
        if let Some((_session, jobs)) = recovered {
            // Keep both locks through announcements, before accepting new work.
            // Boot only announces and never prunes, so retention is the default.
            let events = super::events::JobEvents {
                bus,
                manual_retention: crate::core::prefs::SNAPSHOT_RETENTION.default_number() as usize,
                recovery_retention: crate::core::prefs::SNAPSHOT_RECOVERY_RETENTION.default_number()
                    as usize,
            };
            for job in jobs {
                events.outcome(&job).await?;
            }
        }
        Ok(())
    }

    /// Shared initialization for engine callers. Call on a blocking worker:
    /// store setup and key resolution may touch the filesystem or OS key store.
    pub fn open_for_database(
        database_url: &str,
        gateway_slug: Option<&str>,
        slots: &ArchiveJobSlot,
        kind: ArchiveJobKind,
    ) -> Result<Self, BoxError> {
        let database = crate::core::secret_store::boot::database_name(database_url)
            .map_err(|_| "snapshot_store_unavailable")?;
        let identity = super::identity::workspace_id(gateway_slug, &database)?;
        let store =
            Store::for_database(&database, identity).map_err(|_| "snapshot_store_unavailable")?;
        let keys = crate::core::secret_store::key_store::resolve()
            .map_err(|_| "snapshot_key_unavailable")?;
        Self::open(store, slots, kind, keys.as_ref(), &database)
    }

    /// Run an already-persisted job in an owned worker. Canceling the waiter
    /// retains both locks through capture and the final result write.
    /// The persisted job supplies the authoritative handle and actor.
    pub async fn run_capture_job(
        self: std::sync::Arc<Self>,
        id: uuid::Uuid,
        pool: &PgPool,
        database_url: &str,
        workspace: &Path,
        mut request: CaptureRequest,
    ) -> Result<super::jobs::Job, BoxError> {
        let pool = pool.clone();
        let database_url = database_url.to_owned();
        let workspace = workspace.to_owned();
        tokio::spawn(async move {
            let reader = self.clone();
            let job = tokio::task::spawn_blocking(move || reader.job(id))
                .await
                .map_err(|_| "snapshot_job_worker_failed")??;
            if job.action != super::jobs::Action::Capture
                || !matches!(job.state, super::jobs::State::Running)
            {
                return Err("snapshot_job_action_refused".into());
            }
            let request_handle = job.handle.clone();
            request.handle = job.handle.clone();
            request.actor = job.actor.clone();
            if let Some(events) = &self.events {
                events.progress(&job);
            }
            let result = self
                .clone()
                .capture(&pool, &database_url, &workspace, request)
                .await;
            let state = match result {
                Ok(manifest) => super::jobs::State::Completed {
                    manifest: Box::new(manifest),
                },
                Err(error) => {
                    let reason = capture_failure(&error);
                    crate::log!(
                        "[Snapshots] Capture job {id} for {} failed: {reason:?} ({})",
                        request_handle.as_str(),
                        crate::core::redact_postgres_secrets(&error.to_string())
                    );
                    super::jobs::State::Failed { reason }
                }
            };
            self.complete_job(id, state).await
        })
        .await
        .map_err(|_| "snapshot_job_worker_failed")?
    }

    pub async fn run_check_job(
        self: std::sync::Arc<Self>,
        id: uuid::Uuid,
        database_url: &str,
    ) -> Result<super::jobs::Job, BoxError> {
        let database_url = database_url.to_owned();
        tokio::spawn(async move {
            let reader = self.clone();
            let job = tokio::task::spawn_blocking(move || reader.job(id))
                .await
                .map_err(|_| "snapshot_job_worker_failed")??;
            if job.action != super::jobs::Action::Check
                || !matches!(job.state, super::jobs::State::Running)
            {
                return Err("snapshot_job_action_refused".into());
            }
            if let Some(events) = &self.events {
                events.progress(&job);
            }
            let worker = self.clone();
            let result =
                tokio::task::spawn_blocking(move || worker.check(&job.handle, &database_url)).await;
            let state = match result {
                Ok(Ok(manifest)) => super::jobs::State::Completed {
                    manifest: Box::new(manifest),
                },
                Ok(Err(_)) => super::jobs::State::Failed {
                    reason: super::jobs::Failure::CheckFailed,
                },
                Err(error) => {
                    crate::log!("[Snapshots] Check job {id} worker failed: {error}");
                    super::jobs::State::Failed {
                        reason: super::jobs::Failure::CheckFailed,
                    }
                }
            };
            self.complete_job(id, state).await
        })
        .await
        .map_err(|_| "snapshot_job_worker_failed")?
    }

    pub fn create_job(
        &self,
        handle: Handle,
        action: super::jobs::Action,
        actor: MessageOrigin,
    ) -> Result<super::jobs::Job, BoxError> {
        super::jobs::create(
            self.store.root(),
            &self.store.workspace_id,
            self.restore_generation,
            handle,
            action,
            actor,
        )
        .map_err(|_| "snapshot_job_write_failed".into())
    }

    pub fn job(&self, id: uuid::Uuid) -> Result<super::jobs::Job, BoxError> {
        super::jobs::get(self.store.root(), &self.store.workspace_id, id)
            .map_err(|_| "snapshot_job_unavailable".into())
    }

    pub fn finish_job(
        &self,
        id: uuid::Uuid,
        state: super::jobs::State,
    ) -> Result<super::jobs::Job, BoxError> {
        super::jobs::finish(self.store.root(), &self.store.workspace_id, id, state)
            .map_err(|_| "snapshot_job_update_failed".into())
    }

    /// Boot recovery must run before accepting new archive requests.
    pub fn recover_jobs(&self) -> Result<Vec<super::jobs::Job>, BoxError> {
        let settled = super::jobs::settle(
            self.store.root(),
            &self.store.workspace_id,
            self.restore_generation,
        )
        .map_err(|_| "snapshot_job_recovery_failed")?;
        // Leftover trees cost disk, never correctness, so they never stop boot.
        if let Err(error) = self.store.clean_partial(Utc::now(), &self._lock) {
            crate::log!("[Snapshots] Partial capture cleanup failed: {error}");
        }
        if let Err(error) = self.store.discard_settled_staging(&self._lock) {
            crate::log!("[Snapshots] Restore staging cleanup failed: {error}");
        }
        Ok(settled)
    }

    pub fn open(
        store: Store,
        slots: &ArchiveJobSlot,
        kind: ArchiveJobKind,
        keys: &dyn KeyStore,
        database: &str,
    ) -> Result<Self, BoxError> {
        let slot = slots.try_acquire(kind).ok_or("archive_job_busy")?;
        let lock = store.try_lock().map_err(|_| "archive_job_busy")?;
        let key = store::mac_key(keys, database).map_err(|_| "snapshot_key_unavailable")?;
        Ok(Self {
            store,
            key,
            database: database.to_owned(),
            events: None,
            report_restore: false,
            restore_generation: None,
            #[cfg(test)]
            faults: Default::default(),
            _lock: lock,
            _slot: slot,
        })
    }

    pub fn list(&self) -> Result<Vec<Manifest>, BoxError> {
        self.store
            .list(&self.key)
            .map_err(|_| "snapshot_list_failed".into())
    }

    pub fn get(&self, handle: &Handle) -> Result<Manifest, BoxError> {
        self.store
            .get(handle, &self.key)
            .map_err(|_| "snapshot_unavailable".into())
    }

    pub fn check(&self, handle: &Handle, database_url: &str) -> Result<Manifest, BoxError> {
        check::check(&self.store, handle, &self.key, database_url)
            .map_err(step_failed("Snapshot check", "snapshot_check_failed"))
    }

    pub fn set_keep(&self, handle: &Handle, keep: bool) -> Result<Manifest, BoxError> {
        self.store
            .set_keep(handle, &self.key, keep)
            .map_err(|_| "snapshot_keep_failed".into())
    }

    pub fn delete(&self, handle: &Handle, actor: &MessageOrigin) -> Result<(), BoxError> {
        let manifest = self.get(handle)?;
        if manifest.kind == Kind::Recovery && !matches!(actor, MessageOrigin::Device { .. }) {
            return Err("snapshot_recovery_device_required".into());
        }
        self.store
            .delete(handle, &self.key)
            .map_err(|_| "snapshot_delete_refused".into())
    }

    /// Own deletion and its announcement through caller cancellation.
    pub async fn delete_announced(
        self: std::sync::Arc<Self>,
        handle: Handle,
        actor: MessageOrigin,
    ) -> Result<(), BoxError> {
        let events = self.events.clone().ok_or("snapshot_events_required")?;
        tokio::spawn(async move {
            let worker = self.clone();
            let target = handle.clone();
            let author = actor.clone();
            tokio::task::spawn_blocking(move || worker.delete(&target, &author))
                .await
                .map_err(|_| "snapshot_delete_failed")??;
            events
                .bus
                .emit(crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::SnapshotDeleted {
                        workspace_id: self.store.workspace_id.clone(),
                        handle,
                        actor: Some(actor),
                        reason: super::events::DeletionReason::Manual,
                    },
                ))
                .await
                .map_err(|_| "snapshot_event_failed")?;
            drop(self);
            Ok(())
        })
        .await
        .map_err(|_| "snapshot_delete_failed")?
    }

    /// Internal retention operation. Store protections still apply, including
    /// the newest recovery snapshot. The caller persists SnapshotDeleted with
    /// reason `retention` before we attempt the next deletion. Run this from an
    /// owned job worker so caller cancellation cannot skip that announcement.
    pub async fn prune<F, Fut>(
        self: std::sync::Arc<Self>,
        manual: usize,
        recovery: usize,
        mut announce_deleted: F,
    ) -> Result<(), BoxError>
    where
        F: FnMut(Handle) -> Fut,
        Fut: std::future::Future<Output = Result<(), BoxError>>,
    {
        let worker = self.clone();
        let victims = tokio::task::spawn_blocking(move || {
            let manifests = worker.list()?;
            let referenced = worker
                .store
                .referenced()
                .map_err(|_| "snapshot_retention_failed")?;
            Ok::<_, BoxError>(store::retention_victims(
                &manifests,
                &referenced,
                manual,
                recovery,
            ))
        })
        .await
        .map_err(|_| "snapshot_retention_failed")??;
        for handle in victims {
            let worker = self.clone();
            let victim = handle.clone();
            tokio::task::spawn_blocking(move || {
                worker
                    .store
                    .delete(&victim, &worker.key)
                    .map_err(|_| "snapshot_retention_failed")
            })
            .await
            .map_err(|_| "snapshot_retention_failed")??;
            announce_deleted(handle)
                .await
                .map_err(|_| "snapshot_retention_announcement_failed")?;
        }
        Ok(())
    }

    pub async fn capture(
        self: std::sync::Arc<Self>,
        pool: &PgPool,
        database_url: &str,
        workspace: &Path,
        request: CaptureRequest,
    ) -> Result<Manifest, BoxError> {
        self.capture_with_mode(pool, database_url, workspace, request, CaptureMode::Manual)
            .await
    }

    /// Capture recovery and durably journal its handle before permitting staging.
    /// The persisted request supplies identity and attribution. An interrupted
    /// waiter retains both archive locks until the journal write finishes.
    pub async fn prepare_recovery(
        self: std::sync::Arc<Self>,
        pool: &PgPool,
        database_url: &str,
        workspace: &Path,
        restore_id: uuid::Uuid,
        workspace_name: String,
    ) -> Result<Manifest, BoxError> {
        let pool = pool.clone();
        let database_url = database_url.to_owned();
        let workspace = workspace.to_owned();
        tokio::spawn(async move {
            let reader = self.clone();
            let mut journal = tokio::task::spawn_blocking(move || {
                let journal = reader
                    .store
                    .journals()
                    .map_err(|_| "snapshot_restore_journal_failed")?
                    .into_iter()
                    .find(|journal| journal.restore_id == restore_id)
                    .ok_or("snapshot_restore_journal_unavailable")?;
                if journal.phase() != super::journal::Phase::RecoveryCapture
                    || journal.recovery.is_some()
                {
                    return Err::<_, BoxError>("snapshot_restore_phase_refused".into());
                }
                Ok(journal)
            })
            .await
            .map_err(|_| "snapshot_restore_journal_failed")??;
            let manifest = self
                .clone()
                .capture_recovery(
                    &pool,
                    &database_url,
                    &workspace,
                    RecoveryRequest {
                        restore_id,
                        workspace_name,
                        actor: journal.actor.clone(),
                    },
                )
                .await?;
            journal.recovery = Some(manifest.handle.clone());
            journal.advance(super::journal::Phase::Staging)?;
            tokio::task::spawn_blocking(move || self.store.save_journal(&journal))
                .await
                .map_err(|_| "snapshot_restore_journal_failed")?
                .map_err(|_| "snapshot_restore_journal_failed")?;
            Ok(manifest)
        })
        .await
        .map_err(|_| "snapshot_recovery_failed")?
    }

    /// Capture and fully verify the stopped workspace before any restore mutation.
    /// The caller must durably reference this handle in its journal before advancing.
    pub async fn capture_recovery(
        self: std::sync::Arc<Self>,
        pool: &PgPool,
        database_url: &str,
        workspace: &Path,
        request: RecoveryRequest,
    ) -> Result<Manifest, BoxError> {
        let capture = CaptureRequest {
            handle: Handle::generate(),
            workspace_name: request.workspace_name,
            actor: request.actor,
            label: None,
            keep: false,
            running_threads: 0,
        };
        self.capture_with_mode(
            pool,
            database_url,
            workspace,
            capture,
            CaptureMode::Recovery(request.restore_id),
        )
        .await
    }

    async fn capture_with_mode(
        self: std::sync::Arc<Self>,
        pool: &PgPool,
        database_url: &str,
        workspace: &Path,
        request: CaptureRequest,
        mode: CaptureMode,
    ) -> Result<Manifest, BoxError> {
        let pool = pool.clone();
        let database_url = database_url.to_owned();
        let workspace = workspace.to_owned();
        // Dropping a JoinHandle detaches rather than aborts. In particular,
        // pg_dump and spawn_blocking file capture must retain both locks when
        // an HTTP caller disconnects. The durable job worker waits for this.
        let worker = self.clone();
        let result = tokio::spawn(async move {
            #[cfg(test)]
            if matches!(mode, CaptureMode::Recovery(_)) {
                worker.fault("recovery-capture")?;
            }
            let manifest = worker
                .capture_inner(&pool, &database_url, &workspace, request, mode)
                .await?;
            if matches!(mode, CaptureMode::Recovery(_)) {
                // Keep both locks through verification even if the caller disconnects.
                #[cfg(test)]
                worker.fault("recovery-verify")?;
                tokio::task::spawn_blocking(move || worker.check(&manifest.handle, &database_url))
                    .await
                    .map_err(step_failed(
                        "Recovery check worker",
                        "snapshot_check_failed",
                    ))?
            } else {
                Ok(manifest)
            }
        })
        .await
        .map_err(step_failed("Capture worker", "snapshot_capture_failed"))?;
        drop(self);
        result
    }

    async fn capture_inner(
        &self,
        pool: &PgPool,
        database_url: &str,
        workspace: &Path,
        request: CaptureRequest,
        mode: CaptureMode,
    ) -> Result<Manifest, BoxError> {
        // Include the database in the disk estimate; file bytes alone can be
        // negligible for a workspace whose history is mostly in PostgreSQL.
        let db_bytes: i64 = sqlx::query_scalar("SELECT pg_database_size(current_database())")
            .fetch_one(pool)
            .await
            .map_err(step_failed("Database size query", "snapshot_size_failed"))?;
        let workspace_path = workspace.to_owned();
        let file_bytes =
            tokio::task::spawn_blocking(move || backup::snapshot_file_estimate(&workspace_path))
                .await
                .map_err(step_failed("File size worker", "snapshot_size_failed"))?
                .map_err(step_failed("File size estimate", "snapshot_size_failed"))?;
        self.store
            .preflight_space(file_bytes.saturating_add(db_bytes.max(0) as u64))
            .map_err(|error| {
                if error.is::<store::DiskFloorRefused>() {
                    error
                } else {
                    step_failed("Disk space query", "snapshot_disk_floor")(error)
                }
            })?;
        let partial = self.store.begin(&request.handle).map_err(step_failed(
            "Partial directory creation",
            "snapshot_capture_failed",
        ))?;
        let captured = self
            .capture_into(pool, database_url, workspace, &partial, request, mode)
            .await;
        if captured.is_err() {
            // The disk floor never counts a failed capture's bytes, and boot
            // clears only partials an hour old, so remove them now.
            match std::fs::remove_dir_all(&partial) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    crate::log!("[Snapshots] Failed capture cleanup: {error}");
                }
                _ => {}
            }
        }
        captured
    }

    async fn capture_into(
        &self,
        pool: &PgPool,
        database_url: &str,
        workspace: &Path,
        partial: &Path,
        request: CaptureRequest,
        mode: CaptureMode,
    ) -> Result<Manifest, BoxError> {
        let db = backup::capture::capture_database(pool, database_url, &partial.join("db.dump"))
            .await
            .map_err(step_failed("Database dump", "snapshot_capture_failed"))?;
        let workspace_path = workspace.to_owned();
        let output = partial.to_path_buf();
        let url = database_url.to_owned();
        let (files, components) = tokio::task::spawn_blocking(move || -> Result<_, BoxError> {
            check::verify_dump(&output.join("db.dump"), &url)?;
            let files = backup::capture::capture_files(&workspace_path, &output)?;
            let components = [
                ComponentName::Database,
                ComponentName::Files,
                ComponentName::Index,
            ]
            .into_iter()
            .map(|name| {
                let (bytes, sha256) = check::component_hash(&output.join(name.as_str()))?;
                Ok(Component {
                    name,
                    bytes,
                    sha256,
                })
            })
            .collect::<Result<Vec<_>, BoxError>>()?;
            Ok((files, components))
        })
        .await
        .map_err(step_failed(
            "File capture worker",
            "snapshot_capture_failed",
        ))?
        .map_err(step_failed("File capture", "snapshot_capture_failed"))?;
        if matches!(mode, CaptureMode::Recovery(_)) && files.files_changed_during_read != 0 {
            return Err("snapshot_recovery_files_changed".into());
        }
        let restore_id = match mode {
            CaptureMode::Manual => None,
            CaptureMode::Recovery(id) => Some(id),
        };
        let manifest = Manifest {
            format_version: 1,
            handle: request.handle,
            kind: if restore_id.is_some() {
                Kind::Recovery
            } else {
                Kind::Manual
            },
            label: request.label,
            keep: request.keep,
            workspace_id: self.store.workspace_id.clone(),
            workspace_name: request.workspace_name,
            created_at: Utc::now(),
            captured_by: request.actor,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            schema_version: db.schema_version,
            secret_key_ids: db.secret_key_ids,
            consistency: Consistency {
                db_captured_at: db.db_captured_at,
                files_started_at: files.files_started_at,
                files_finished_at: files.files_finished_at,
                live: restore_id.is_none(),
                running_threads: request.running_threads,
                files_changed_during_read: files.files_changed_during_read,
            },
            components,
            files: Files {
                count: files.count,
                bytes: files.bytes,
                tree_sha256: files.tree_sha256,
            },
            table_rows: db.table_rows,
            verified: Some(Verification {
                mac: true,
                at: Utc::now(),
            }),
            restore_id,
        };
        let signed = AuthenticatedManifest::seal(manifest.clone(), &self.key)
            .map_err(step_failed("Manifest signing", "snapshot_capture_failed"))?;
        self.store
            .publish(&signed, &self.key)
            .map_err(step_failed("Snapshot publish", "snapshot_capture_failed"))?;
        Ok(manifest)
    }
}
