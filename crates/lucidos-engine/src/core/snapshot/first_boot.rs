//! Restricted restore boot: no Engine, HTTP, scheduler, recovery or dispatch.
//! Forward verification deliberately does not unblock normal boot: Phase 6
//! must durably settle the restored log and install the restore generation first.
use super::{restore_database, BoxError};
use crate::core::secret_store::{boot, key_store::KeyStore};

/// Restricted first-boot capability. The borrow prevents releasing workspace
/// ownership while verification or restored-log settlement still holds the
/// archive lock. Dropping this capability never completes the journal.
pub struct RestoreBoot<'owner> {
    store: super::store::Store,
    _archive_lock: super::store::StoreLock,
    _owner: &'owner lucidos_owner_lock::OwnerLock,
    workspace: std::path::PathBuf,
    database: String,
}

/// Verified capability retained through construction and restored settlement.
/// Its private fields prevent a caller from bypassing restricted verification.
pub struct VerifiedRestoreBoot<'owner> {
    guard: RestoreBoot<'owner>,
    boundary: super::boundary::Boundary,
}

impl VerifiedRestoreBoot<'_> {
    pub(crate) fn validate_binding(
        &self,
        workspace: &std::path::Path,
        database_url: &str,
    ) -> Result<(), BoxError> {
        if workspace != self.guard.workspace
            || boot::database_name(database_url)? != self.guard.database
        {
            return Err("snapshot_restore_boot_binding_mismatch".into());
        }
        Ok(())
    }

    /// Called only after restored-log settlement, while dispatch is closed.
    /// This deliberately does not complete the journal or release dispatch.
    pub(crate) async fn announce_restored(
        &self,
        bus: &crate::engine::event_bus::EventBus,
    ) -> Result<(), BoxError> {
        self.validate_boundary(bus.pool()).await?;
        let (_, mut journal) = pending_restore(&self.guard.workspace, &self.guard.store)?
            .ok_or("snapshot_restore_marker_required")?;
        let recovery = journal
            .recovery
            .clone()
            .ok_or("snapshot_recovery_required")?;
        if journal.announcement.is_none() {
            let paths =
                super::worktree_exemptions::read(self.guard.store.root(), &self.guard.workspace)?;
            let retained_worktrees = paths
                .iter()
                .map(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned)
                        .ok_or_else(|| BoxError::from("snapshot_worktree_name_invalid"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            journal.announcement = Some(super::journal::RestoreAnnouncement {
                notification_id: uuid::Uuid::new_v4(),
                retained_worktrees,
                actor_label: Some(restore_actor_label(bus.pool(), &journal.actor).await?),
            });
            self.guard.store.save_journal(&journal)?;
        }
        let announcement = journal
            .announcement
            .ok_or("snapshot_announcement_required")?;
        bus.emit_system_once(
            journal.restore_id,
            crate::engine::event_bus::SystemEvent::WorkspaceRestored {
                workspace_id: journal.workspace_id.clone(),
                handle: journal.snapshot.clone(),
                recovery: recovery.clone(),
                restore_id: journal.restore_id,
                retained_worktrees: announcement.retained_worktrees.clone(),
                actor: journal.actor.clone(),
            },
        )
        .await
        .map_err(|_| "snapshot_event_failed")?;
        bus.emit_system_once(
            announcement.notification_id,
            crate::engine::event_bus::SystemEvent::NotificationCreated {
                id: announcement.notification_id.to_string(),
                title: "Workspace restored".into(),
                message: format!(
                    "Restored snapshot `{}`. Recovery snapshot: `{}`. Retained worktrees: {}.{}",
                    journal.snapshot.as_str(),
                    recovery.as_str(),
                    if announcement.retained_worktrees.is_empty() {
                        "none".into()
                    } else {
                        announcement.retained_worktrees.join(", ")
                    },
                    announcement
                        .actor_label
                        .as_ref()
                        .map(|label| format!(" Requested by {label}."))
                        .unwrap_or_default(),
                ),
                task_id: None,
                app_id: None,
                thread_id: None,
                event_id: None,
                tap: crate::scheduler::notifications::Tap::RestoreSnapshot {
                    workspace_id: journal.workspace_id,
                    handle: recovery,
                },
                actor: Some(journal.actor),
                audience: None,
                push_group: None,
            },
        )
        .await
        .map_err(|_| "snapshot_notification_failed")?;
        Ok(())
    }

    /// Consume restricted ownership only after both announcements are durable.
    /// A crash between the external save and marker removal retries Completed
    /// without repeating events. Returning releases the archive lock before
    /// normal archive-job recovery attempts to acquire it.
    pub(crate) async fn complete(
        self,
        bus: &crate::engine::event_bus::EventBus,
    ) -> Result<(), BoxError> {
        self.announce_restored(bus).await?;
        let (_, mut journal) = pending_restore(&self.guard.workspace, &self.guard.store)?
            .ok_or("snapshot_restore_marker_required")?;
        if journal.phase() != super::journal::Phase::Completed {
            journal.advance(super::journal::Phase::Completed)?;
            self.guard.store.save_journal(&journal)?;
        }
        remove_marker(&self.guard.workspace)?;
        Ok(())
    }

    pub(crate) async fn validate_boundary(&self, pool: &sqlx::PgPool) -> Result<(), BoxError> {
        if super::boundary::read(pool).await?.as_ref() != Some(&self.boundary) {
            return Err("snapshot_restore_boot_boundary_mismatch".into());
        }
        Ok(())
    }
}

/// Only the two independently authenticated restore origins may announce a
/// restore. Freeze display metadata in external evidence, never re-resolve it
/// after a lost append (the restored database may contain a different name).
async fn restore_actor_label(
    pool: &sqlx::PgPool,
    actor: &crate::engine::thread_events::MessageOrigin,
) -> Result<String, BoxError> {
    use crate::engine::thread_events::MessageOrigin;
    match actor {
        MessageOrigin::Device { device_id } => Ok(format!(
            "owner device {}",
            crate::core::devices::DeviceStore::friendly_name(pool, device_id).await
        )),
        MessageOrigin::Workspace {
            workspace, mode, ..
        } => Ok(format!("{} from workspace `{workspace}`", mode.as_str())),
        _ => Err("snapshot_restore_actor_invalid".into()),
    }
}

impl<'owner> RestoreBoot<'owner> {
    /// The gateway registry binds all three identities before a database is
    /// opened. A bare engine cannot use a restored marker as authority.
    pub fn acquire(
        owner: &'owner lucidos_owner_lock::OwnerLock,
        workspace: &std::path::Path,
        database_url: &str,
    ) -> Result<Self, BoxError> {
        let gateway_slug =
            crate::api::base_path::workspace_id().ok_or("snapshot_restore_binding_unavailable")?;
        let app_data =
            lucidos_home::gateway_data_dir().ok_or("snapshot_restore_binding_unavailable")?;
        super::registry_binding::resolve(
            &app_data,
            workspace,
            database_url,
            &gateway_slug,
            uuid::Uuid::nil(),
        )?;
        let database = boot::database_name(database_url)?;
        let store = super::store::Store::existing_for_database(&database, gateway_slug)?;
        Self::acquire_at(owner, workspace, database, store)
    }

    fn acquire_at(
        owner: &'owner lucidos_owner_lock::OwnerLock,
        workspace: &std::path::Path,
        database: String,
        store: super::store::Store,
    ) -> Result<Self, BoxError> {
        let archive_lock = store.try_lock()?;
        pending_restore(workspace, &store)?.ok_or("snapshot_restore_marker_required")?;
        Ok(Self {
            store,
            _archive_lock: archive_lock,
            _owner: owner,
            workspace: workspace.to_owned(),
            database,
        })
    }

    /// Migrations, current access and key verification precede the durable
    /// generation. This is still no permission to start a driver or HTTP.
    pub async fn verify(self, pool: sqlx::PgPool) -> Result<VerifiedRestoreBoot<'owner>, BoxError> {
        verify_owned(pool.clone(), self.database.clone()).await?;
        let boundary = self.install_boundary(&pool).await?;
        Ok(VerifiedRestoreBoot {
            guard: self,
            boundary,
        })
    }

    async fn install_boundary(
        &self,
        pool: &sqlx::PgPool,
    ) -> Result<super::boundary::Boundary, BoxError> {
        super::boundary::install(pool, &self.workspace, &self.store).await
    }
}

/// Durable handoff to restored-log settlement, never permission to boot normally.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    pub restore_id: uuid::Uuid,
    pub snapshot: super::handle::Handle,
    pub restored_at: chrono::DateTime<chrono::Utc>,
}

/// Read the handoff without following redirected paths or allocating from an
/// unbounded file. Absence is distinct from an unreadable or malformed marker.
pub fn read_marker(workspace: &std::path::Path) -> Result<Option<Marker>, BoxError> {
    use std::io::Read;
    let runtime = workspace.join(".lucidos");
    match std::fs::symlink_metadata(&runtime) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("snapshot_marker_path_refused".into()),
        Ok(meta) if !meta.is_dir() => return Err("snapshot_marker_path_refused".into()),
        Ok(_) => {}
    }
    let path = runtime.join("restored-from-snapshot");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("snapshot_marker_path_refused".into()),
        Ok(meta) if !meta.is_file() => return Err("snapshot_marker_path_refused".into()),
        Ok(_) => {}
    }
    const MAX_MARKER_BYTES: u64 = 4096;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(MAX_MARKER_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| "snapshot_marker_unreadable")?;
    if bytes.len() > MAX_MARKER_BYTES as usize {
        return Err("snapshot_marker_invalid".into());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "snapshot_marker_invalid".into())
}

/// Bind a first-boot handoff to the external journal. This is deliberately not
/// a boot permit: the caller still has to settle restored work durably before
/// changing the journal to Completed. Call under the workspace owner lock and
/// archive lock so the offline worker cannot change the handoff concurrently.
pub fn pending_restore(
    workspace: &std::path::Path,
    store: &super::store::Store,
) -> Result<Option<(Marker, super::journal::Journal)>, BoxError> {
    use super::journal::Phase;
    let marker = read_marker(workspace)?;
    let journals = store.journals()?;
    let Some(marker) = marker else {
        if journals
            .iter()
            .any(|journal| journal.phase().blocks_engine())
        {
            return Err("snapshot_restore_marker_required".into());
        }
        return Ok(None);
    };
    let journal = journals
        .iter()
        .find(|journal| journal.restore_id == marker.restore_id)
        .ok_or("snapshot_restore_marker_mismatch")?;
    if journal.snapshot != marker.snapshot
        || journal.workspace_id != store.workspace_id
        || !matches!(journal.phase(), Phase::FilesRestored | Phase::Completed)
        || journal.recovery.is_none()
        || marker.restored_at > journal.updated_at
        || journals
            .iter()
            .any(|other| other.restore_id != marker.restore_id && !other.phase().is_terminal())
        || journals
            .iter()
            .any(|other| other.restore_id != marker.restore_id && other.phase().blocks_engine())
    {
        return Err("snapshot_restore_marker_mismatch".into());
    }
    Ok(Some((marker, journal.clone())))
}

pub fn write_marker(
    workspace: &std::path::Path,
    journal: &super::journal::Journal,
) -> Result<(), BoxError> {
    let runtime = workspace.join(".lucidos");
    let meta = std::fs::symlink_metadata(&runtime)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("snapshot_marker_path_refused".into());
    }
    super::store::write_json(
        &runtime.join("restored-from-snapshot"),
        &Marker {
            restore_id: journal.restore_id,
            snapshot: journal.snapshot.clone(),
            restored_at: chrono::Utc::now(),
        },
    )
}

pub fn remove_marker(workspace: &std::path::Path) -> Result<(), BoxError> {
    let runtime = workspace.join(".lucidos");
    let meta = std::fs::symlink_metadata(&runtime)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("snapshot_marker_path_refused".into());
    }
    match std::fs::remove_file(runtime.join("restored-from-snapshot")) {
        Ok(()) => std::fs::File::open(runtime)?.sync_all().map_err(Into::into),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Run SQLx's migration future locally while the caller retains its owner lock.
pub async fn verify_owned(pool: sqlx::PgPool, database: String) -> Result<(), BoxError> {
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let keys = crate::core::secret_store::key_store::resolve()?;
        runtime.block_on(verify(&pool, &database, keys.as_ref()))
    })
    .await
    .map_err(|_| "snapshot_first_boot_failed")?
}

pub async fn verify(
    pool: &sqlx::PgPool,
    database: &str,
    keys: &dyn KeyStore,
) -> Result<(), BoxError> {
    async {
        let actual: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(pool)
            .await?;
        if actual != database {
            return Err::<_, BoxError>("database binding changed".into());
        }
        sqlx::migrate!().run(pool).await?;
        restore_database::finish_access_after_migrations(pool).await?;
        match boot::boot(pool, database, keys).await? {
            boot::Boot::Ready(_) => Ok(()),
            boot::Boot::Refused(_) => Err("secret verification refused".into()),
        }
    }
    .await
    .map_err(|_| "snapshot_first_boot_failed".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{core::secret_store::key_store::TestKeyStore, test_support::*};

    fn pending_fixture(
        workspace: &std::path::Path,
        store: &super::super::store::Store,
    ) -> super::super::journal::Journal {
        use super::super::{
            handle::Handle,
            journal::{Journal, Phase},
        };
        std::fs::create_dir_all(workspace.join(".lucidos")).unwrap();
        let fixture = super::super::manifest::fixture();
        let mut journal = Journal::new(
            store.workspace_id.clone(),
            fixture.handle,
            fixture.captured_by,
            None,
        );
        store.save_journal(&journal).unwrap();
        journal.advance(Phase::RecoveryCapture).unwrap();
        store.save_journal(&journal).unwrap();
        journal.recovery = Some(Handle::generate());
        for phase in [Phase::Staging, Phase::DbRestore, Phase::FileSwap] {
            journal.advance(phase).unwrap();
            store.save_journal(&journal).unwrap();
        }
        write_marker(workspace, &journal).unwrap();
        journal.advance(Phase::FilesRestored).unwrap();
        store.save_journal(&journal).unwrap();
        journal
    }

    /// HOME is scoped to a new executable, never changed in the test runner.
    /// This includes archive recovery and all startup wiring after completion.
    #[tokio::test]
    async fn full_constructor_completes_restore_in_isolated_process() {
        const CHILD: &str = "LUCIDOS_TEST_RESTORE_CONSTRUCTOR_CHILD";
        const CANARY: &str = "snapshot-constructor-synthetic-secret";
        fn assert_redacted(bytes: &[u8], extra: Option<&str>) {
            let key = crate::core::secret_store::key().unwrap();
            let mut needles = vec![
                CANARY.as_bytes().to_vec(),
                key.as_bytes().to_vec(),
                key.to_base64().into_bytes(),
            ];
            if let Some(value) = extra {
                needles.push(value.as_bytes().to_vec());
            }
            assert!(
                !needles
                    .iter()
                    .any(|needle| bytes.windows(needle.len()).any(|window| window == needle)),
                "secret in restore output"
            );
        }
        if std::env::var_os(CHILD).is_none() {
            let home = tempfile::tempdir().unwrap();
            let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
            child.args(["--exact",
                "core::snapshot::first_boot::tests::full_constructor_completes_restore_in_isolated_process",
                "--nocapture"]);
            child.env(CHILD, "1").env("HOME", home.path());
            child.env_remove("LUCIDOS_GATEWAY_DATA");
            child.env("LUCIDOS_WORKSPACE_ID", "constructor-workspace");
            child.kill_on_drop(true);
            let output = tokio::time::timeout(std::time::Duration::from_secs(120), child.output())
                .await
                .expect("constructor must finish without archive-lock deadlock")
                .expect("spawn isolated constructor");
            // Scan before formatting failed subprocess output into the test log.
            assert_redacted(&output.stdout, None);
            assert_redacted(&output.stderr, None);
            assert!(
                output.status.success(),
                "isolated constructor failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        use super::super::{
            handle::Handle,
            journal::{Journal, Phase},
            service::{CaptureRequest, OfflineRestoreRequest, Session},
            store::Store,
            worktree_exemptions, ArchiveJobKind, ArchiveJobSlot,
        };
        let (pool, database) = setup_test_db().await;
        let url = test_db_url(&database);
        crate::test_support::seed_credential(
            &pool,
            "restore-canary",
            "https://example.org",
            crate::core::AuthType::Bearer,
            CANARY,
        )
        .await;
        let home = lucidos_home::ensure().unwrap();
        let workspace = home.join("workspace");
        let root = Store::database_root(&database).unwrap();
        let identity = "constructor-workspace".to_owned();
        std::fs::create_dir_all(workspace.join("data")).unwrap();
        std::fs::write(workspace.join("data/note"), "snapshot A").unwrap();
        let session = std::sync::Arc::new(
            Session::open(
                Store::at(root.clone(), identity.clone()).unwrap(),
                &ArchiveJobSlot::default(),
                ArchiveJobKind::SnapshotRestore,
                &TestKeyStore,
                &database,
            )
            .unwrap(),
        );
        let actor = crate::engine::thread_events::MessageOrigin::Device {
            device_id: "constructor-owner".into(),
        };
        let capture = || CaptureRequest {
            handle: Handle::generate(),
            workspace_name: "Constructor workspace".into(),
            actor: actor.clone(),
            label: None,
            keep: false,
            running_threads: 0,
        };
        let (fixture_bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let old_approval =
            crate::api::owner_approval::tests::seed_snapshot_approval(&fixture_bus).await;
        let old_cut = crate::scheduler::cut_fires::tests::seed_snapshot_cut(&fixture_bus).await;
        let old_running = uuid::Uuid::new_v4();
        fixture_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id: old_running,
                event: crate::engine::thread_events::ThreadEvent::from_stored(
                    "MessageReceived",
                    serde_json::json!({"text":"Synthetic interrupted request","mode":"human"}),
                )
                .unwrap(),
                meta: crate::engine::thread_events::EventMeta::NONE,
            })
            .await
            .unwrap();
        crate::test_support::seed_device(&pool, "revoked-snapshot-device", None, None).await;
        let saved = session
            .clone()
            .capture(&pool, &url, &workspace, capture())
            .await
            .unwrap();
        std::fs::write(workspace.join("data/note"), "snapshot B").unwrap();
        let later = session
            .clone()
            .capture(&pool, &url, &workspace, capture())
            .await
            .unwrap();
        sqlx::query("DELETE FROM devices WHERE id = 'revoked-snapshot-device'")
            .execute(&pool)
            .await
            .unwrap();
        crate::test_support::seed_device(&pool, "recovery-test-device", Some("Mozilla/5.0"), None)
            .await;
        let current_access: Vec<serde_json::Value> =
            sqlx::query_scalar("SELECT to_jsonb(d) FROM devices d ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        // This tree did not exist at capture time and has uncommitted work.
        let dirty_tree = workspace
            .join(crate::paths::WORKTREES_SUBPATH)
            .join("thread-post-snapshot");
        std::fs::create_dir_all(&dirty_tree).unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&dirty_tree)
            .status()
            .unwrap();
        assert!(init.success());
        std::fs::write(dirty_tree.join("unsaved.txt"), "post-snapshot draft").unwrap();
        let mut journal = Journal::new(identity.clone(), saved.handle, actor, None);
        session.admit_restore(&journal).unwrap();
        journal.advance(Phase::RecoveryCapture).unwrap();
        Store::existing_at(root.clone(), identity.clone())
            .unwrap()
            .save_journal(&journal)
            .unwrap();
        session
            .clone()
            .run_restore(
                &pool,
                &url,
                &workspace,
                OfflineRestoreRequest {
                    restore_id: journal.restore_id,
                    gateway_slug: identity.clone(),
                    workspace_name: "Constructor workspace".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            Store::existing_at(root.clone(), identity.clone())
                .unwrap()
                .journals()
                .unwrap()[0]
                .phase(),
            Phase::FilesRestored
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("data/note")).unwrap(),
            "snapshot A"
        );
        drop(session);
        let store = Store::existing_at(root.clone(), identity.clone()).unwrap();
        assert!(worktree_exemptions::read(&root, &workspace)
            .unwrap()
            .contains(&dirty_tree.canonicalize().unwrap()));
        let owner = lucidos_owner_lock::OwnerLock::acquire(&home.join("owner.lock")).unwrap();
        let verified = RestoreBoot::acquire_at(&owner, &workspace, database.clone(), store)
            .unwrap()
            .verify(pool.clone())
            .await
            .unwrap();
        let generation = verified.boundary.clone();
        let ctx = crate::llm::ProviderBuildContext {
            default_model: "mock".into(),
            model_is_mock: true,
            vertex_project_id: String::new(),
            vertex_location: crate::llm::vertex::location_handle(String::new()),
            vertex_token_cache: None,
            model_registry: Default::default(),
            not_served: Default::default(),
            claude_gateway_access: None,
            chatgpt_plan_access: None,
        };
        let engine =
            crate::engine::LucidosEngine::new_with_restore(workspace.clone(), &url, ctx, verified)
                .await
                .expect("full restored engine construction");
        assert!(read_marker(&workspace).unwrap().is_none());
        let store = Store::existing_at(root.clone(), identity.clone()).unwrap();
        assert_eq!(store.journals().unwrap()[0].phase(), Phase::Completed);
        let released = store.try_lock().expect("completion released archive lock");
        drop(released);
        let session = Session::open(
            store,
            &ArchiveJobSlot::default(),
            ArchiveJobKind::SnapshotCheck,
            &TestKeyStore,
            &database,
        )
        .unwrap();
        assert!(session
            .list()
            .unwrap()
            .iter()
            .any(|entry| entry.handle == later.handle));
        assert_eq!(
            session.check(&later.handle, &url).unwrap().handle,
            later.handle
        );
        assert_eq!(
            std::fs::read_to_string(dirty_tree.join("unsaved.txt")).unwrap(),
            "post-snapshot draft"
        );
        let status = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&dirty_tree)
            .output()
            .unwrap();
        assert!(status.status.success());
        assert!(String::from_utf8(status.stdout)
            .unwrap()
            .contains("?? unsaved.txt"));
        assert_redacted(
            &serde_json::to_vec(&session.list().unwrap()).unwrap(),
            Some(&url),
        );
        for entry in std::fs::read_dir(root.join("journal")).unwrap() {
            assert_redacted(&std::fs::read(entry.unwrap().path()).unwrap(), Some(&url));
        }
        let events: Vec<serde_json::Value> = sqlx::query_scalar("SELECT payload FROM events")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_redacted(&serde_json::to_vec(&events).unwrap(), Some(&url));
        drop(session);
        assert_eq!(
            super::super::boundary::read(&pool).await.unwrap().unwrap(),
            generation
        );
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE event_type = 'WorkspaceRestored' AND payload->'data'->>'restore_id' = $1"
        ).bind(journal.restore_id.to_string()).fetch_one(&pool).await.unwrap();
        assert_eq!(count, 1);
        let restored_access: Vec<serde_json::Value> =
            sqlx::query_scalar("SELECT to_jsonb(d) FROM devices d ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(restored_access, current_access);
        crate::api::owner_approval::tests::assert_snapshot_approval_expired(
            &fixture_bus,
            old_approval,
        )
        .await;
        crate::scheduler::cut_fires::tests::assert_snapshot_cut_excluded(&pool, old_cut).await;
        let boundary = super::super::boundary::read(&pool).await.unwrap().unwrap();
        assert!(!super::super::boundary::permits_slot(
            &pool,
            boundary.restored_at - chrono::Duration::seconds(1)
        )
        .await
        .unwrap());
        let engine = std::sync::Arc::new(engine);
        engine.recover_orphaned_threads(&[]).await;
        crate::scheduler::assert_snapshot_catch_up_excluded(&engine).await;
        let replayed: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE thread_id = $1 AND sequence > $2 AND event_type IN ('MessageReceived', 'TriggerStarted', 'OwnerApprovalSpent')")
            .bind(old_running).bind(boundary.event_sequence).fetch_one(&pool).await.unwrap();
        assert_eq!(replayed, 0);
        let running: String =
            sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
                .bind(old_running)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_ne!(running, "running");
        super::recovered_compose_http_tests::verify(engine, generation.restore_id).await;
        pool.close().await;
    }

    #[tokio::test]
    async fn fresh_boot_retries_keep_the_floor_and_announce_without_rebroadcast() {
        use super::super::{boundary, journal::Phase, store::Store, worktree_exemptions};
        use crate::engine::{
            event_bus::{BusEvent, EventBus, SystemEvent},
            restore_boot::settle_before_dispatch,
        };
        let (pool, database) = setup_test_db().await;
        let tree = tempfile::tempdir().unwrap();
        let workspace = tree.path().join("workspace");
        let root = tree.path().join("store");
        let store = Store::at(root.clone(), "demo").unwrap();
        let journal = pending_fixture(&workspace, &store);
        worktree_exemptions::preserve(store.root(), &workspace).unwrap();
        let owner =
            lucidos_owner_lock::OwnerLock::acquire(&tree.path().join("owner.lock")).unwrap();
        // Seed archived authority through the production EventBus before the
        // first floor is installed. Construction must end it before announcing.
        let (seed_bus, _) = EventBus::new(pool.clone());
        let batch = uuid::Uuid::new_v4();
        seed_bus
            .emit(BusEvent::System(SystemEvent::ApplyAllBatchStarted {
                batch_id: batch,
                change_ids: vec![],
                actor: None,
            }))
            .await
            .unwrap();
        sqlx::query("INSERT INTO apply_all_batches (batch_id, change_ids) VALUES ($1, $2)")
            .bind(batch)
            .bind(Vec::<uuid::Uuid>::new())
            .execute(&pool)
            .await
            .unwrap();
        seed_bus
            .emit(BusEvent::System(SystemEvent::MainCheckStarted {
                check_id: uuid::Uuid::new_v4(),
                from_sha: "before".into(),
                to_sha: "after".into(),
            }))
            .await
            .unwrap();
        drop(seed_bus);
        let mut original_floor = None;
        let mut notification_id = None;
        // Each attempt drops every process-local capability. The fourth models
        // losing the successful reply; the fifth models a completed journal
        // whose marker removal was interrupted.
        for attempt in 0..5 {
            let store = Store::existing_at(root.clone(), "demo".into()).unwrap();
            let guard =
                RestoreBoot::acquire_at(&owner, &workspace, database.clone(), store).unwrap();
            verify(&pool, &database, &TestKeyStore).await.unwrap();
            let floor = guard.install_boundary(&pool).await.unwrap();
            assert_eq!(floor.restore_id, journal.restore_id);
            assert_eq!(original_floor.get_or_insert_with(|| floor.clone()), &floor);
            let verified = VerifiedRestoreBoot {
                guard,
                boundary: floor,
            };
            let (bus, _) = EventBus::new(pool.clone());
            let mut live = bus.subscribe();
            if attempt == 0 {
                refuse_event_type(&pool, "ApplyAllBatchCompleted").await;
                assert!(settle_before_dispatch(&bus, Some(&verified)).await.is_err());
                assert!(verified.guard.store.journals().unwrap()[0]
                    .announcement
                    .is_none());
                let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type IN ('WorkspaceRestored', 'NotificationCreated', 'MainCheckAbandoned')")
                    .fetch_one(&pool).await.unwrap();
                assert_eq!(count, 0, "settlement failure must stop the boot sequence");
                sqlx::query("DROP TRIGGER refuse_event ON events")
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query("DROP FUNCTION refuse_event()")
                    .execute(&pool)
                    .await
                    .unwrap();
                drop(verified);
                continue;
            }
            if attempt == 1 {
                refuse_event_type(&pool, "NotificationCreated").await;
                assert!(settle_before_dispatch(&bus, Some(&verified)).await.is_err());
                assert_eq!(
                    verified.guard.store.journals().unwrap()[0].phase(),
                    Phase::FilesRestored
                );
                sqlx::query("DROP TRIGGER refuse_event ON events")
                    .execute(&pool)
                    .await
                    .unwrap();
            } else {
                settle_before_dispatch(&bus, Some(&verified)).await.unwrap();
                if attempt >= 3 {
                    assert!(
                        live.try_recv().is_err(),
                        "durable retries must not wake subscribers"
                    );
                }
            }
            let mut saved = verified.guard.store.journals().unwrap().remove(0);
            let id = saved.announcement.as_ref().unwrap().notification_id;
            assert_eq!(*notification_id.get_or_insert(id), id);
            if attempt == 3 {
                saved.advance(Phase::Completed).unwrap();
                verified.guard.store.save_journal(&saved).unwrap();
            }
            assert!(read_marker(&workspace).unwrap().is_some());
            if attempt == 4 {
                crate::engine::restore_boot::complete_before_dispatch(&bus, Some(verified))
                    .await
                    .unwrap();
                assert!(read_marker(&workspace).unwrap().is_none());
                assert_eq!(
                    Store::existing_at(root.clone(), "demo".into())
                        .unwrap()
                        .journals()
                        .unwrap()[0]
                        .phase(),
                    Phase::Completed
                );
                assert!(live.try_recv().is_err(), "completion must not rebroadcast");
            } else {
                drop(verified);
            }
            let contender = Store::existing_at(root.clone(), "demo".into()).unwrap();
            assert!(
                contender.try_lock().is_ok(),
                "boot drop must release archive ownership"
            );
            assert_eq!(boundary::read(&pool).await.unwrap(), original_floor);
        }
        let counts: Vec<(String, i64)> = sqlx::query_as(
            "SELECT event_type, count(*) FROM events WHERE event_type IN ('WorkspaceRestored', 'NotificationCreated') GROUP BY event_type ORDER BY event_type",
        ).fetch_all(&pool).await.unwrap();
        assert_eq!(
            counts,
            vec![
                ("NotificationCreated".into(), 1),
                ("WorkspaceRestored".into(), 1)
            ]
        );
        let settlements: Vec<(String, i64)> = sqlx::query_as(
            "SELECT event_type, count(*) FROM events WHERE event_type IN ('ApplyAllBatchCompleted', 'MainCheckAbandoned') GROUP BY event_type ORDER BY event_type"
        ).fetch_all(&pool).await.unwrap();
        assert_eq!(
            settlements,
            vec![
                ("ApplyAllBatchCompleted".into(), 1),
                ("MainCheckAbandoned".into(), 1)
            ]
        );
        let ordered: bool = sqlx::query_scalar(
            "SELECT (SELECT max(sequence) FROM events WHERE event_type IN ('ApplyAllBatchCompleted', 'MainCheckAbandoned')) < (SELECT min(sequence) FROM events WHERE event_type = 'WorkspaceRestored')"
        ).fetch_one(&pool).await.unwrap();
        assert!(
            ordered,
            "announcement must follow all restored authority settlement"
        );
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM apply_all_batches")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0);
        pool.close().await;
        teardown_test_db(&database).await;
    }

    #[tokio::test]
    async fn completion_failure_keeps_marker_and_releases_archive_lock_for_retry() {
        use super::super::{journal::Phase, store::Store, worktree_exemptions};
        use crate::engine::{event_bus::EventBus, restore_boot::complete_before_dispatch};
        let (pool, database) = setup_test_db().await;
        let tree = tempfile::tempdir().unwrap();
        let workspace = tree.path().join("workspace");
        let root = tree.path().join("store");
        let store = Store::at(root.clone(), "demo").unwrap();
        pending_fixture(&workspace, &store);
        worktree_exemptions::preserve(store.root(), &workspace).unwrap();
        let owner =
            lucidos_owner_lock::OwnerLock::acquire(&tree.path().join("owner.lock")).unwrap();
        let (bus, _) = EventBus::new(pool.clone());
        for event in ["WorkspaceRestored", "NotificationCreated", ""] {
            let store = Store::existing_at(root.clone(), "demo".into()).unwrap();
            let guard =
                RestoreBoot::acquire_at(&owner, &workspace, database.clone(), store).unwrap();
            let boundary = guard.install_boundary(&pool).await.unwrap();
            let verified = VerifiedRestoreBoot { guard, boundary };
            if !event.is_empty() {
                refuse_event_type(&pool, event).await;
            }
            let result = complete_before_dispatch(&bus, Some(verified)).await;
            let store = Store::existing_at(root.clone(), "demo".into()).unwrap();
            let lock = store
                .try_lock()
                .expect("consuming completion must release archive ownership");
            if event.is_empty() {
                result.unwrap();
                assert_eq!(store.journals().unwrap()[0].phase(), Phase::Completed);
                assert!(read_marker(&workspace).unwrap().is_none());
            } else {
                assert!(result.is_err());
                assert_eq!(store.journals().unwrap()[0].phase(), Phase::FilesRestored);
                assert!(read_marker(&workspace).unwrap().is_some());
                sqlx::query("DROP TRIGGER refuse_event ON events")
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query("DROP FUNCTION refuse_event()")
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            drop(lock);
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type IN ('WorkspaceRestored', 'NotificationCreated')").fetch_one(&pool).await.unwrap();
        assert_eq!(count, 2);
        pool.close().await;
        teardown_test_db(&database).await;
    }

    #[tokio::test]
    async fn completion_filesystem_failures_preserve_retry_and_durable_announcements() {
        use super::super::{boundary, journal::Phase, store::Store, worktree_exemptions};
        use crate::engine::{event_bus::EventBus, restore_boot::complete_before_dispatch};
        use std::os::unix::fs::PermissionsExt;

        // Restore permissions even if an assertion unwinds, so the disposable
        // tree remains removable. Only this test's private directories change.
        struct ReadOnlyDirectory(std::path::PathBuf, std::fs::Permissions);
        impl Drop for ReadOnlyDirectory {
            fn drop(&mut self) {
                std::fs::set_permissions(&self.0, self.1.clone()).unwrap();
            }
        }
        for fail_journal in [true, false] {
            let (pool, database) = setup_test_db().await;
            let tree = tempfile::tempdir().unwrap();
            let workspace = tree.path().join("workspace");
            let root = tree.path().join("store");
            let store = Store::at(root.clone(), "demo").unwrap();
            pending_fixture(&workspace, &store);
            worktree_exemptions::preserve(store.root(), &workspace).unwrap();
            let owner =
                lucidos_owner_lock::OwnerLock::acquire(&tree.path().join("owner.lock")).unwrap();
            let guard =
                RestoreBoot::acquire_at(&owner, &workspace, database.clone(), store).unwrap();
            let floor = guard.install_boundary(&pool).await.unwrap();
            let verified = VerifiedRestoreBoot {
                guard,
                boundary: floor.clone(),
            };
            let (bus, mut live) = EventBus::new(pool.clone());
            verified.announce_restored(&bus).await.unwrap();
            while live.try_recv().is_ok() {}
            let blocked = if fail_journal {
                root.join("journal")
            } else {
                workspace.join(".lucidos")
            };
            let permissions = std::fs::metadata(&blocked).unwrap().permissions();
            let restore_permissions = ReadOnlyDirectory(blocked.clone(), permissions);
            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o500)).unwrap();
            assert!(complete_before_dispatch(&bus, Some(verified))
                .await
                .is_err());
            drop(restore_permissions);

            let store = Store::existing_at(root.clone(), "demo".into()).unwrap();
            let lock = store
                .try_lock()
                .expect("failed completion releases archive ownership");
            assert_eq!(
                store.journals().unwrap()[0].phase(),
                if fail_journal {
                    Phase::FilesRestored
                } else {
                    Phase::Completed
                }
            );
            assert!(read_marker(&workspace).unwrap().is_some());
            assert_eq!(boundary::read(&pool).await.unwrap(), Some(floor.clone()));
            assert!(
                live.try_recv().is_err(),
                "failed completion must not rebroadcast"
            );
            drop(lock);

            let guard =
                RestoreBoot::acquire_at(&owner, &workspace, database.clone(), store).unwrap();
            let retried_floor = guard.install_boundary(&pool).await.unwrap();
            assert_eq!(retried_floor, floor);
            complete_before_dispatch(
                &bus,
                Some(VerifiedRestoreBoot {
                    guard,
                    boundary: retried_floor,
                }),
            )
            .await
            .unwrap();
            let store = Store::existing_at(root, "demo".into()).unwrap();
            assert!(store.try_lock().is_ok());
            assert_eq!(store.journals().unwrap()[0].phase(), Phase::Completed);
            assert!(read_marker(&workspace).unwrap().is_none());
            assert!(
                live.try_recv().is_err(),
                "fresh-capability retry must not rebroadcast"
            );
            let counts: Vec<(String, i64)> = sqlx::query_as(
                "SELECT event_type, count(*) FROM events WHERE event_type IN ('WorkspaceRestored', 'NotificationCreated') GROUP BY event_type ORDER BY event_type",
            ).fetch_all(&pool).await.unwrap();
            assert_eq!(
                counts,
                vec![
                    ("NotificationCreated".into(), 1),
                    ("WorkspaceRestored".into(), 1)
                ]
            );
            pool.close().await;
            teardown_test_db(&database).await;
        }
    }

    #[tokio::test]
    async fn restore_announcement_retries_use_frozen_external_evidence() {
        use super::super::{journal::Phase, store::Store, worktree_exemptions};
        use crate::engine::event_bus::EventBus;
        let (pool, database) = setup_test_db().await;
        let tree = tempfile::tempdir().unwrap();
        let workspace = tree.path().join("workspace");
        let store = Store::at(tree.path().join("store"), "demo").unwrap();
        let journal = pending_fixture(&workspace, &store);
        sqlx::query("INSERT INTO devices (id, name) VALUES ('test-device', 'Test phone')")
            .execute(&pool)
            .await
            .unwrap();
        let trees = workspace.join(crate::paths::WORKTREES_SUBPATH);
        std::fs::create_dir_all(trees.join("retained-thread")).unwrap();
        worktree_exemptions::preserve(store.root(), &workspace).unwrap();
        let owner =
            lucidos_owner_lock::OwnerLock::acquire(&tree.path().join("owner.lock")).unwrap();
        let guard = RestoreBoot::acquire_at(&owner, &workspace, database.clone(), store).unwrap();
        let boundary = guard.install_boundary(&pool).await.unwrap();
        let verified = VerifiedRestoreBoot { guard, boundary };
        let (bus, _) = EventBus::new(pool.clone());
        crate::test_support::refuse_event_type(&pool, "WorkspaceRestored").await;
        assert!(verified.announce_restored(&bus).await.is_err());
        let evidence = verified.guard.store.journals().unwrap()[0]
            .announcement
            .clone()
            .unwrap();
        assert_eq!(evidence.retained_worktrees, vec!["retained-thread"]);
        assert_eq!(
            evidence.actor_label.as_deref(),
            Some("owner device Test phone")
        );
        sqlx::query("UPDATE devices SET name = 'Renamed phone' WHERE id = 'test-device'")
            .execute(&pool)
            .await
            .unwrap();
        // Changes to external exemptions after a lost append cannot change
        // the payload associated with this restore's stable event identity.
        std::fs::create_dir_all(trees.join("later-thread")).unwrap();
        worktree_exemptions::preserve(verified.guard.store.root(), &workspace).unwrap();
        sqlx::query("DROP TRIGGER refuse_event ON events")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DROP FUNCTION refuse_event()")
            .execute(&pool)
            .await
            .unwrap();
        crate::test_support::refuse_event_type(&pool, "NotificationCreated").await;
        assert!(verified.announce_restored(&bus).await.is_err());
        assert_eq!(
            verified.guard.store.journals().unwrap()[0].phase(),
            Phase::FilesRestored
        );
        sqlx::query("DROP TRIGGER refuse_event ON events")
            .execute(&pool)
            .await
            .unwrap();
        verified.announce_restored(&bus).await.unwrap();
        verified.announce_restored(&bus).await.unwrap();
        let notification: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM events WHERE id = $1 AND event_type = 'NotificationCreated'",
        )
        .bind(evidence.notification_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            notification["data"]["tap"],
            serde_json::json!({
                "kind": "restore-snapshot", "workspace_id": journal.workspace_id,
                "handle": journal.recovery.as_ref().unwrap(),
            })
        );
        assert_eq!(
            notification["data"]["id"],
            evidence.notification_id.to_string()
        );
        assert_eq!(
            notification["data"]["actor"],
            serde_json::to_value(&journal.actor).unwrap()
        );
        assert!(notification["data"]["message"]
            .as_str()
            .unwrap()
            .contains("Requested by owner device Test phone."));
        let payload: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM events WHERE id = $1 AND event_type = 'WorkspaceRestored'",
        )
        .bind(journal.restore_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            payload["data"]["retained_worktrees"],
            serde_json::json!(["retained-thread"])
        );
        assert_eq!(
            verified.guard.store.journals().unwrap()[0].phase(),
            Phase::FilesRestored
        );
        let mut changed = verified.guard.store.journals().unwrap().remove(0);
        changed
            .announcement
            .as_mut()
            .unwrap()
            .retained_worktrees
            .clear();
        assert!(verified.guard.store.save_journal(&changed).is_err());
        sqlx::query("UPDATE snapshot_restore_boundary SET snapshot = 'wrong'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(verified.announce_restored(&bus).await.is_err());
        drop(verified);
        pool.close().await;
        teardown_test_db(&database).await;
    }

    #[tokio::test]
    async fn legacy_restore_announcement_preserves_notification_identity() {
        use super::super::{journal::RestoreAnnouncement, store::Store};
        use crate::engine::event_bus::EventBus;
        let (pool, database) = setup_test_db().await;
        let tree = tempfile::tempdir().unwrap();
        let workspace = tree.path().join("workspace");
        let store = Store::at(tree.path().join("store"), "demo").unwrap();
        let mut journal = pending_fixture(&workspace, &store);
        // Decode the exact evidence shape written before visible attribution.
        let notification_id = uuid::Uuid::new_v4();
        journal.announcement = Some(
            serde_json::from_value::<RestoreAnnouncement>(
                serde_json::json!({ "notification_id": notification_id, "retained_worktrees": [] }),
            )
            .unwrap(),
        );
        store.save_journal(&journal).unwrap();
        let owner =
            lucidos_owner_lock::OwnerLock::acquire(&tree.path().join("owner.lock")).unwrap();
        let guard = RestoreBoot::acquire_at(&owner, &workspace, database.clone(), store).unwrap();
        let boundary = guard.install_boundary(&pool).await.unwrap();
        let verified = VerifiedRestoreBoot { guard, boundary };
        let (bus, _) = EventBus::new(pool.clone());
        verified.announce_restored(&bus).await.unwrap();
        verified.announce_restored(&bus).await.unwrap();
        let payload: serde_json::Value =
            sqlx::query_scalar("SELECT payload FROM events WHERE id = $1")
                .bind(notification_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            payload["data"]["message"],
            format!(
                "Restored snapshot `{}`. Recovery snapshot: `{}`. Retained worktrees: none.",
                journal.snapshot.as_str(),
                journal.recovery.unwrap().as_str(),
            )
        );
        assert!(verified.guard.store.journals().unwrap()[0]
            .announcement
            .as_ref()
            .unwrap()
            .actor_label
            .is_none());
        drop(verified);
        pool.close().await;
        teardown_test_db(&database).await;
    }

    #[tokio::test]
    async fn restricted_boot_holds_archive_lock_and_drop_never_completes_restore() {
        use super::super::{boundary, journal::Phase, store::Store};
        let (pool, database) = setup_test_db().await;
        let tree = tempfile::tempdir().unwrap();
        let workspace = tree.path().join("workspace");
        let store_root = tree.path().join("store");
        let store = Store::at(store_root.clone(), "demo").unwrap();
        let journal = pending_fixture(&workspace, &store);
        let owner =
            lucidos_owner_lock::OwnerLock::acquire(&tree.path().join("owner.lock")).unwrap();
        let guard = RestoreBoot::acquire_at(&owner, &workspace, database.clone(), store).unwrap();
        let competitor = Store::existing_at(store_root, "demo".into()).unwrap();
        assert!(competitor.try_lock().is_err());
        assert!(boundary::read(&pool).await.unwrap().is_none());
        // Exercise the same verification and boundary order with disposable
        // keys. No host keystore or live workspace is involved.
        verify(&pool, &database, &TestKeyStore).await.unwrap();
        let first = guard.install_boundary(&pool).await.unwrap();
        assert_eq!(first.restore_id, journal.restore_id);
        assert_eq!(guard.install_boundary(&pool).await.unwrap(), first);
        assert!(competitor.try_lock().is_err());
        let verified = VerifiedRestoreBoot {
            guard,
            boundary: first,
        };
        let url = format!("postgres://localhost/{database}");
        verified.validate_binding(&workspace, &url).unwrap();
        assert!(verified.validate_binding(tree.path(), &url).is_err());
        assert!(verified
            .validate_binding(&workspace, "postgres://localhost/another")
            .is_err());
        verified.validate_boundary(&pool).await.unwrap();
        sqlx::query("UPDATE snapshot_restore_boundary SET snapshot = 'different'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(verified.validate_boundary(&pool).await.is_err());
        assert!(competitor.try_lock().is_err());
        drop(verified);
        assert!(competitor.try_lock().is_ok());
        assert_eq!(
            competitor.journals().unwrap()[0].phase(),
            Phase::FilesRestored
        );
        pool.close().await;
        teardown_test_db(&database).await;
    }

    #[test]
    fn restricted_boot_refuses_absent_marker_and_competing_archive_lock() {
        use super::super::store::Store;
        let tree = tempfile::tempdir().unwrap();
        let workspace = tree.path().join("workspace");
        let owner =
            lucidos_owner_lock::OwnerLock::acquire(&tree.path().join("owner.lock")).unwrap();
        let root = tree.path().join("store");
        let store = Store::at(root.clone(), "demo").unwrap();
        assert!(RestoreBoot::acquire_at(&owner, &workspace, "demo".into(), store).is_err());
        let store = Store::existing_at(root.clone(), "demo".into()).unwrap();
        pending_fixture(&workspace, &store);
        let lock = store.try_lock().unwrap();
        let competing = Store::existing_at(root, "demo".into()).unwrap();
        assert!(RestoreBoot::acquire_at(&owner, &workspace, "demo".into(), competing).is_err());
        drop(lock);
    }

    #[tokio::test]
    async fn restore_boundary_survives_crash_without_consuming_new_work() {
        use super::super::{boundary, store::Store};
        use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
        let (pool, database) = setup_test_db().await;
        let tree = tempfile::tempdir().unwrap();
        let store = Store::at(tree.path().join("store"), "demo").unwrap();
        let workspace = tree.path().join("workspace");
        assert!(boundary::read(&pool).await.unwrap().is_none());
        assert!(boundary::permits_slot(&pool, chrono::Utc::now())
            .await
            .unwrap());
        assert!(boundary::install(&pool, &workspace, &store).await.is_err());
        let mut journal = pending_fixture(&workspace, &store);
        let (bus, _rx) = EventBus::new(pool.clone());
        let emit = || {
            BusEvent::System(SystemEvent::BackupCompleted {
                filename: "fixture.enc".into(),
                size_bytes: 0,
                started_at: chrono::Utc::now(),
                finished_at: chrono::Utc::now(),
            })
        };
        bus.emit(emit()).await.unwrap();
        let first = boundary::install(&pool, &workspace, &store).await.unwrap();
        assert!(first.event_sequence > 0);
        bus.emit(emit()).await.unwrap();
        assert_eq!(
            boundary::install(&pool, &workspace, &store).await.unwrap(),
            first
        );
        let path = workspace.join(".lucidos/restored-from-snapshot");
        let mut marker = read_marker(&workspace).unwrap().unwrap();
        marker.restored_at -= chrono::Duration::seconds(1);
        super::super::store::write_json(&path, &marker).unwrap();
        assert!(boundary::install(&pool, &workspace, &store).await.is_err());
        assert_eq!(boundary::read(&pool).await.unwrap(), Some(first.clone()));
        for slot in [
            first.restored_at - chrono::Duration::seconds(1),
            first.restored_at,
        ] {
            assert!(!boundary::permits_slot(&pool, slot).await.unwrap());
        }
        assert!(
            boundary::permits_slot(&pool, first.restored_at + chrono::Duration::seconds(1))
                .await
                .unwrap()
        );
        remove_marker(&workspace).unwrap();
        assert_eq!(boundary::read(&pool).await.unwrap(), Some(first.clone()));
        assert!(boundary::install(&pool, &workspace, &store).await.is_err());
        journal
            .advance(super::super::journal::Phase::Completed)
            .unwrap();
        store.save_journal(&journal).unwrap();
        pending_fixture(&workspace, &store);
        let next = boundary::install(&pool, &workspace, &store).await.unwrap();
        assert_ne!(next.restore_id, first.restore_id);
        assert!(next.event_sequence > first.event_sequence);
        pool.close().await;
        assert!(boundary::permits_slot(&pool, chrono::Utc::now())
            .await
            .is_err());
        teardown_test_db(&database).await;
    }

    #[test]
    fn first_boot_requires_matching_durable_handoff_and_preserves_it_on_refusal() {
        use super::super::{journal::Phase, store::Store};
        let tree = tempfile::tempdir().unwrap();
        let store = Store::at(tree.path().join("store"), "demo").unwrap();
        let workspace = tree.path().join("workspace");
        assert!(pending_restore(&workspace, &store).unwrap().is_none());
        let mut journal = pending_fixture(&workspace, &store);
        let (marker, found) = pending_restore(&workspace, &store).unwrap().unwrap();
        assert_eq!(found.restore_id, journal.restore_id);
        assert_eq!(marker.snapshot, journal.snapshot);
        // Completing the journal before removing the marker is crash-safe.
        journal.advance(Phase::Completed).unwrap();
        store.save_journal(&journal).unwrap();
        assert!(pending_restore(&workspace, &store).unwrap().is_some());
        let path = workspace.join(".lucidos/restored-from-snapshot");
        let mut value = serde_json::to_value(marker).unwrap();
        for field in ["restore_id", "snapshot", "restored_at"] {
            let original = value[field].clone();
            value[field] = match field {
                "restore_id" => serde_json::json!(uuid::Uuid::new_v4()),
                "snapshot" => serde_json::json!(super::super::handle::Handle::generate()),
                _ => serde_json::json!(chrono::Utc::now() + chrono::Duration::days(1)),
            };
            super::super::store::write_json(&path, &value).unwrap();
            assert!(pending_restore(&workspace, &store).is_err());
            assert!(path.exists());
            value[field] = original;
        }
        remove_marker(&workspace).unwrap();
        assert!(pending_restore(&workspace, &store).unwrap().is_none());
    }

    #[test]
    fn missing_marker_competing_restore_and_rollback_block_first_boot() {
        use super::super::{
            journal::{Journal, Phase},
            store::Store,
        };
        let tree = tempfile::tempdir().unwrap();
        let store = Store::at(tree.path().join("store"), "demo").unwrap();
        let workspace = tree.path().join("workspace");
        let mut journal = pending_fixture(&workspace, &store);
        let other = Journal::new(
            "demo",
            journal.snapshot.clone(),
            journal.actor.clone(),
            None,
        );
        store.save_journal(&other).unwrap();
        assert!(pending_restore(&workspace, &store).is_err());
        let mut other = other;
        other.advance(Phase::Refused).unwrap();
        store.save_journal(&other).unwrap();
        assert!(pending_restore(&workspace, &store).unwrap().is_some());
        remove_marker(&workspace).unwrap();
        assert!(pending_restore(&workspace, &store).is_err());
        write_marker(&workspace, &journal).unwrap();
        journal.advance(Phase::RollingBack).unwrap();
        store.save_journal(&journal).unwrap();
        assert!(pending_restore(&workspace, &store).is_err());
    }

    #[test]
    fn marker_reader_refuses_redirects_malformed_and_oversized_files() {
        let tree = tempfile::tempdir().unwrap();
        let runtime = tree.path().join(".lucidos");
        std::fs::create_dir(&runtime).unwrap();
        let path = runtime.join("restored-from-snapshot");
        for bytes in [b"invalid".to_vec(), vec![b' '; 4097]] {
            std::fs::write(&path, bytes).unwrap();
            assert!(read_marker(tree.path()).is_err());
        }
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(tree.path().join("absent"), &path).unwrap();
        assert!(read_marker(tree.path()).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&runtime).unwrap();
        std::os::unix::fs::symlink(tree.path().join("absent"), &runtime).unwrap();
        assert!(read_marker(tree.path()).is_err());
    }

    #[test]
    fn marker_is_durable_and_refuses_redirected_runtime() {
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let fixture = super::super::manifest::fixture();
        let journal = super::super::journal::Journal::new(
            fixture.workspace_id,
            fixture.handle,
            fixture.captured_by,
            None,
        );
        let runtime = workspace.path().join(".lucidos");
        std::os::unix::fs::symlink(external.path(), &runtime).unwrap();
        assert!(write_marker(workspace.path(), &journal).is_err());
        assert!(remove_marker(workspace.path()).is_err());
        assert_eq!(std::fs::read_dir(external.path()).unwrap().count(), 0);
        std::fs::remove_file(&runtime).unwrap();
        std::fs::create_dir(&runtime).unwrap();
        write_marker(workspace.path(), &journal).unwrap();
        let marker: Marker =
            serde_json::from_slice(&std::fs::read(runtime.join("restored-from-snapshot")).unwrap())
                .unwrap();
        assert_eq!(marker.restore_id, journal.restore_id);
        assert_eq!(marker.snapshot, journal.snapshot);
        remove_marker(workspace.path()).unwrap();
        remove_marker(workspace.path()).unwrap();
        assert!(!runtime.join("restored-from-snapshot").exists());
    }

    #[tokio::test]
    async fn verifies_without_engine_and_refuses_a_different_database() {
        let (pool, database) = setup_test_db().await;
        assert_eq!(
            verify(&pool, "wrong_database", &TestKeyStore)
                .await
                .unwrap_err()
                .to_string(),
            "snapshot_first_boot_failed"
        );
        verify(&pool, &database, &TestKeyStore).await.unwrap();
        // Repeating after access staging was consumed is safe.
        verify(&pool, &database, &TestKeyStore).await.unwrap();
        pool.close().await;
        teardown_test_db(&database).await;
    }
    #[tokio::test]
    async fn migration_failure_keeps_authoritative_access_staging_for_recovery() {
        let (pool, database) = setup_test_db().await;
        sqlx::raw_sql("CREATE SCHEMA snapshot_restore_access; CREATE TABLE snapshot_restore_access.rows (table_name text, value jsonb); INSERT INTO snapshot_restore_access.rows VALUES ('devices', '{\"id\":\"current-device\"}'); INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (99999999999999, 'injected newer migration', true, decode('00', 'hex'), 0);")
            .execute(&pool).await.unwrap();
        assert_eq!(
            verify(&pool, &database, &TestKeyStore)
                .await
                .unwrap_err()
                .to_string(),
            "snapshot_first_boot_failed"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM snapshot_restore_access.rows")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        pool.close().await;
        teardown_test_db(&database).await;
    }
}

#[cfg(test)]
#[path = "recovered_compose_http_tests.rs"]
mod recovered_compose_http_tests;
