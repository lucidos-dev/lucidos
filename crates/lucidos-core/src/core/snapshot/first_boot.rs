//! INVARIANT: Restricted restore boot: no Engine, HTTP, scheduler, recovery or dispatch.
//! Forward verification deliberately does not unblock normal boot: Phase 6
//! must durably settle the restored log and install the restore generation first.
use super::{restore_database, BoxError};
use crate::core::secret_store::{boot, key_store::KeyStore};

pub struct RestoreBoot<'owner> {
    store: super::store::Store,
    _archive_lock: super::store::StoreLock,
    _owner: &'owner lucidos_owner_lock::OwnerLock,
    workspace: std::path::PathBuf,
    database: String,
}

pub struct VerifiedRestoreBoot<'owner> {
    guard: RestoreBoot<'owner>,
    boundary: super::boundary::Boundary,
}

impl VerifiedRestoreBoot<'_> {
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn boundary_for_test(&self) -> &super::boundary::Boundary {
        &self.boundary
    }

    pub fn validate_binding(
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

    pub async fn announce_restored(
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
                tap: crate::engine::thread_events::notification::Tap::RestoreSnapshot {
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

    /// INVARIANT: Consume restricted ownership only after both announcements are durable.
    /// A crash between the external save and marker removal retries Completed
    /// without repeating events. Returning releases the archive lock before
    /// normal archive-job recovery attempts to acquire it.
    pub async fn complete(self, bus: &crate::engine::event_bus::EventBus) -> Result<(), BoxError> {
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

    pub async fn validate_boundary(&self, pool: &sqlx::PgPool) -> Result<(), BoxError> {
        if super::boundary::read(pool).await?.as_ref() != Some(&self.boundary) {
            return Err("snapshot_restore_boot_boundary_mismatch".into());
        }
        Ok(())
    }
}

/// INVARIANT: Only the two independently authenticated restore origins may announce a
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
    pub fn acquire(
        owner: &'owner lucidos_owner_lock::OwnerLock,
        workspace: &std::path::Path,
        database_url: &str,
    ) -> Result<Self, BoxError> {
        let gateway_slug =
            crate::core::base_path::workspace_id().ok_or("snapshot_restore_binding_unavailable")?;
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

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn acquire_for_test(
        owner: &'owner lucidos_owner_lock::OwnerLock,
        workspace: &std::path::Path,
        database: String,
        store: super::store::Store,
    ) -> Result<Self, BoxError> {
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

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    pub restore_id: uuid::Uuid,
    pub snapshot: super::handle::Handle,
    pub restored_at: chrono::DateTime<chrono::Utc>,
}

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

/// INVARIANT: Bind a first-boot handoff to the external journal. This is deliberately not
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
        crate::core::store::MIGRATOR.run(pool).await?;
        restore_database::finish_access_after_migrations(pool).await?;
        match boot::boot(pool, database, keys).await? {
            boot::Boot::Ready(_) => Ok(()),
            boot::Boot::Refused(_) => Err("secret verification refused".into()),
        }
    }
    .await
    .map_err(|_| "snapshot_first_boot_failed".into())
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn pending_fixture(
    workspace: &std::path::Path,
    store: &super::store::Store,
) -> super::journal::Journal {
    use super::{
        handle::Handle,
        journal::{Journal, Phase},
    };
    std::fs::create_dir_all(workspace.join(".lucidos")).unwrap();
    let fixture = super::manifest::fixture();
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

#[cfg(feature = "test-support")]
impl<'owner> RestoreBoot<'owner> {
    #[doc(hidden)]
    pub async fn install_boundary_for_test(
        self,
        pool: &sqlx::PgPool,
    ) -> Result<VerifiedRestoreBoot<'owner>, BoxError> {
        let boundary = self.install_boundary(pool).await?;
        Ok(VerifiedRestoreBoot {
            guard: self,
            boundary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{core::secret_store::key_store::TestKeyStore, test_support::*};

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
