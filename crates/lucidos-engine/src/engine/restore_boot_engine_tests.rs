#[test]
fn production_checks_precede_workspace_mutation() {
    let main = include_str!("../../../lucidos-engine-bin/src/main.rs");
    let run = &main[main.find("async fn run()").unwrap()..];
    let guard = run.find("snapshot::boot_guard::ensure_boot_entry").unwrap();
    assert!(guard < run.find("ensure_storage_root(&workspace_path)").unwrap());
    let owned = run.find("own_workspace(&workspace_path)").unwrap();
    let recheck = run[owned..]
        .find("snapshot::boot_guard::ensure_normal_boot")
        .unwrap()
        + owned;
    assert!(recheck < run.find("pin_cli_for_workspace(&workspace_path").unwrap());
    let acquire = run.find("first_boot::RestoreBoot::acquire").unwrap();
    let verify = run.find("guard.verify(pool.clone())").unwrap();
    assert!(owned < acquire && acquire < verify);
    assert!(
        verify
            < run
                .find("proxy_migration::migrate_apis_json_if_needed")
                .unwrap()
    );
    let construction = include_str!("engine_impl/construction.rs");
    let new = &construction[construction.find("pub async fn new(").unwrap()..];
    assert!(
        new.find("snapshot::boot_guard::ensure_normal_boot")
            .unwrap()
            < new.find("ArtifactManager::new(").unwrap()
    );
    let restored = &construction[construction.find("pub async fn new_with_restore(").unwrap()..];
    assert!(
        restored.find("restore_boot.validate_binding").unwrap()
            < restored.find("ArtifactManager::new(").unwrap()
    );
    let settlement = restored
        .find("restore_boot::settle_before_dispatch")
        .unwrap();
    let sequence = include_str!("restore_boot.rs");
    assert!(
        sequence.find("restore_boot.validate_boundary").unwrap()
            < sequence.find("settle_restored_threads").unwrap()
    );
    assert!(
        sequence.find("settle_restored_checks").unwrap()
            < sequence.find("announce_restored").unwrap()
    );
    let completion = restored
        .find("restore_boot::complete_before_dispatch")
        .unwrap();
    assert!(settlement < completion);
    assert!(completion < restored.find("Session::recover_at_boot").unwrap());
}

use crate::core::secret_store::key_store::TestKeyStore;
use crate::core::snapshot::first_boot::{pending_fixture, read_marker, verify, RestoreBoot};
use crate::test_support::{refuse_event_type, setup_test_db, teardown_test_db};

#[tokio::test]
async fn fresh_boot_retries_keep_the_floor_and_announce_without_rebroadcast() {
    use crate::core::snapshot::{boundary, journal::Phase, store::Store, worktree_exemptions};
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
    let owner = lucidos_owner_lock::OwnerLock::acquire(&tree.path().join("owner.lock")).unwrap();
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
            RestoreBoot::acquire_for_test(&owner, &workspace, database.clone(), store).unwrap();
        verify(&pool, &database, &TestKeyStore).await.unwrap();
        let verified = guard.install_boundary_for_test(&pool).await.unwrap();
        let floor = verified.boundary_for_test().clone();
        assert_eq!(floor.restore_id, journal.restore_id);
        assert_eq!(original_floor.get_or_insert_with(|| floor.clone()), &floor);
        let (bus, _) = EventBus::new(pool.clone());
        let mut live = bus.subscribe();
        if attempt == 0 {
            refuse_event_type(&pool, "ApplyAllBatchCompleted").await;
            assert!(settle_before_dispatch(&bus, Some(&verified)).await.is_err());
            assert!(Store::existing_at(root.clone(), "demo".into())
                .unwrap()
                .journals()
                .unwrap()[0]
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
                Store::existing_at(root.clone(), "demo".into())
                    .unwrap()
                    .journals()
                    .unwrap()[0]
                    .phase(),
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
        let mut saved = Store::existing_at(root.clone(), "demo".into())
            .unwrap()
            .journals()
            .unwrap()
            .remove(0);
        let id = saved.announcement.as_ref().unwrap().notification_id;
        assert_eq!(*notification_id.get_or_insert(id), id);
        if attempt == 3 {
            saved.advance(Phase::Completed).unwrap();
            Store::existing_at(root.clone(), "demo".into())
                .unwrap()
                .save_journal(&saved)
                .unwrap();
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
    use crate::core::snapshot::{journal::Phase, store::Store, worktree_exemptions};
    use crate::engine::{event_bus::EventBus, restore_boot::complete_before_dispatch};
    let (pool, database) = setup_test_db().await;
    let tree = tempfile::tempdir().unwrap();
    let workspace = tree.path().join("workspace");
    let root = tree.path().join("store");
    let store = Store::at(root.clone(), "demo").unwrap();
    pending_fixture(&workspace, &store);
    worktree_exemptions::preserve(store.root(), &workspace).unwrap();
    let owner = lucidos_owner_lock::OwnerLock::acquire(&tree.path().join("owner.lock")).unwrap();
    let (bus, _) = EventBus::new(pool.clone());
    for event in ["WorkspaceRestored", "NotificationCreated", ""] {
        let store = Store::existing_at(root.clone(), "demo".into()).unwrap();
        let guard =
            RestoreBoot::acquire_for_test(&owner, &workspace, database.clone(), store).unwrap();
        let verified = guard.install_boundary_for_test(&pool).await.unwrap();
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
    use crate::core::snapshot::{boundary, journal::Phase, store::Store, worktree_exemptions};
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
            RestoreBoot::acquire_for_test(&owner, &workspace, database.clone(), store).unwrap();
        let verified = guard.install_boundary_for_test(&pool).await.unwrap();
        let floor = verified.boundary_for_test().clone();
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
            RestoreBoot::acquire_for_test(&owner, &workspace, database.clone(), store).unwrap();
        let verified = guard.install_boundary_for_test(&pool).await.unwrap();
        let retried_floor = verified.boundary_for_test().clone();
        assert_eq!(retried_floor, floor);
        complete_before_dispatch(&bus, Some(verified))
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
