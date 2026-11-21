use super::*;
use crate::core::secret_store::key_store::TestKeyStore;
use crate::core::snapshot::jobs::{self, Action, Failure, State};
use uuid::Uuid;

/// Reconcile once the owner lock is free. A concurrent test's `fork` copies
/// this process's lock descriptors, so a released lock stays held until that
/// child execs (`lucidos-owner-lock` header). The refusal is transient. The
/// deadline bounds only the wait: a call that took the lock runs to the end.
async fn reconcile_when_owner_free(
    session: &std::sync::Arc<Session>,
    restore_id: Uuid,
    workspace: &std::path::Path,
    database_url: &str,
) -> Result<(), BoxError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match session
            .clone()
            .reconcile_interrupted_restore(restore_id, workspace, database_url)
            .await
        {
            Err(error) if error.to_string() == "snapshot_workspace_still_running" => {
                if std::time::Instant::now() >= deadline {
                    return Err("owner lock never freed".into());
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            settled => return settled,
        }
    }
}

#[tokio::test]
async fn reconcile_waits_for_a_lock_a_forked_child_still_holds() {
    use super::super::journal::{Journal, Phase};
    let workspace = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let session = std::sync::Arc::new(
        Session::open(
            Store::at(root.path().into(), "demo-workspace").unwrap(),
            &ArchiveJobSlot::default(),
            ArchiveJobKind::SnapshotRestore,
            &TestKeyStore,
            "snapshot_reconcile_wait_test",
        )
        .unwrap(),
    );
    let journal = Journal::new(
        "demo-workspace",
        Handle::generate(),
        MessageOrigin::Device {
            device_id: "test-device".into(),
        },
        None,
    );
    session.store.save_journal(&journal).unwrap();
    let held = lucidos_owner_lock::OwnerLock::try_acquire(&lucidos_owner_lock::engine::lock_path(
        workspace.path(),
    ))
    .unwrap()
    .unwrap();
    let url = "invalid-url";
    let immediate = session
        .clone()
        .reconcile_interrupted_restore(journal.restore_id, workspace.path(), url)
        .await;
    assert_eq!(
        immediate.unwrap_err().to_string(),
        "snapshot_workspace_still_running"
    );
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        drop(held);
    });
    reconcile_when_owner_free(&session, journal.restore_id, workspace.path(), url)
        .await
        .unwrap();
    releaser.join().unwrap();
    assert_eq!(session.store.journals().unwrap()[0].phase(), Phase::Refused);
}

#[tokio::test]
async fn recovery_capture_is_offline_verified_and_bound_to_restore() {
    use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
    let (pool, database) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("note"), b"before restore").unwrap();
    let root = tempfile::tempdir().unwrap();
    let session = std::sync::Arc::new(
        Session::open(
            Store::at(root.path().into(), "demo-workspace").unwrap(),
            &ArchiveJobSlot::default(),
            ArchiveJobKind::SnapshotRestore,
            &TestKeyStore,
            &database,
        )
        .unwrap(),
    );
    let restore_id = Uuid::new_v4();
    let actor = MessageOrigin::Device {
        device_id: "test-device".into(),
    };
    let mut journal = super::super::journal::Journal::new(
        "demo-workspace",
        Handle::generate(),
        actor.clone(),
        None,
    );
    journal.restore_id = restore_id;
    session.store.save_journal(&journal).unwrap();
    assert!(session
        .clone()
        .prepare_recovery(
            &pool,
            &test_db_url(&database),
            workspace.path(),
            restore_id,
            "demo".into(),
        )
        .await
        .is_err());
    assert!(session.list().unwrap().is_empty());
    journal
        .advance(super::super::journal::Phase::RecoveryCapture)
        .unwrap();
    session.store.save_journal(&journal).unwrap();
    let error = session
        .clone()
        .prepare_recovery(
            &pool,
            "invalid-database-url-with-sentinel",
            workspace.path(),
            restore_id,
            "demo".into(),
        )
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("sentinel"));
    let unchanged = session.store.journals().unwrap().remove(0);
    assert_eq!(
        unchanged.phase(),
        super::super::journal::Phase::RecoveryCapture
    );
    assert!(unchanged.recovery.is_none());
    let manifest = session
        .clone()
        .prepare_recovery(
            &pool,
            &test_db_url(&database),
            workspace.path(),
            restore_id,
            "demo".into(),
        )
        .await
        .unwrap();
    let reopened = Store::existing_at(root.path().into(), "demo-workspace".into()).unwrap();
    let durable = reopened.journals().unwrap().remove(0);
    assert_eq!(durable.phase(), super::super::journal::Phase::Staging);
    assert_eq!(durable.recovery.as_ref(), Some(&manifest.handle));
    assert!(reopened.referenced().unwrap().contains(&manifest.handle));
    assert!(session
        .clone()
        .prepare_recovery(
            &pool,
            &test_db_url(&database),
            workspace.path(),
            restore_id,
            "demo".into(),
        )
        .await
        .is_err());
    assert_eq!(session.list().unwrap().len(), 1);
    assert_eq!(manifest.kind, Kind::Recovery);
    assert_eq!(manifest.restore_id, Some(restore_id));
    assert_eq!(manifest.captured_by, actor);
    assert!(!manifest.consistency.live);
    assert_eq!(manifest.consistency.running_threads, 0);
    assert_eq!(manifest.consistency.files_changed_during_read, 0);
    assert!(manifest.verified.as_ref().unwrap().mac);
    assert_eq!(manifest.files.count, 1);
    let stored = session
        .check(&manifest.handle, &test_db_url(&database))
        .unwrap();
    assert_eq!(stored.restore_id, Some(restore_id));
    assert!(
        session
            .delete(
                &manifest.handle,
                &MessageOrigin::Device {
                    device_id: "test-device".into(),
                }
            )
            .is_err(),
        "newest recovery remains protected"
    );
    drop(session);
    pool.close().await;
    teardown_test_db(&database).await;
}

#[test]
fn initialization_refuses_unsafe_identity_and_sanitizes_database_errors_before_store_access() {
    let slots = ArchiveJobSlot::default();
    for (url, slug, expected) in [
        ("not-a-url-with-secret", None, "snapshot_store_unavailable"),
        (
            "postgres://demo:sentinel@localhost/demo_db",
            Some("../bad"),
            "snapshot_workspace_identity_unavailable",
        ),
    ] {
        let error = Session::open_for_database(url, slug, &slots, ArchiveJobKind::SnapshotCapture)
            .err()
            .expect("initialization must fail");
        assert_eq!(error.to_string(), expected);
        assert!(!slots.is_busy());
    }
}

fn retention_session() -> (tempfile::TempDir, std::sync::Arc<Session>, Vec<Manifest>) {
    let root = tempfile::tempdir().unwrap();
    let session = std::sync::Arc::new(
        Session::open(
            Store::at(root.path().into(), "demo-workspace").unwrap(),
            &ArchiveJobSlot::default(),
            ArchiveJobKind::SnapshotCapture,
            &TestKeyStore,
            "demo",
        )
        .unwrap(),
    );
    let mut manifests = Vec::new();
    for i in 0..5 {
        let mut manifest = super::super::manifest::fixture();
        manifest.workspace_id = session.store.workspace_id.clone();
        manifest.created_at = Utc::now() - chrono::Duration::days(i);
        if i >= 2 {
            manifest.kind = Kind::Recovery;
            manifest.restore_id = Some(Uuid::new_v4());
        }
        let dir = session.store.snapshot_dir(&manifest.handle);
        store::private_dir(&dir).unwrap();
        store::write_json(
            &dir.join("manifest.json"),
            &AuthenticatedManifest::seal(manifest.clone(), &session.key).unwrap(),
        )
        .unwrap();
        manifests.push(manifest);
    }
    (root, session, manifests)
}

#[tokio::test]
async fn retention_announces_each_deletion_before_the_next_and_preserves_protections() {
    let (_root, session, manifests) = retention_session();
    session.set_keep(&manifests[0].handle, true).unwrap();
    let mut announced = Vec::new();
    let observer = session.clone();
    session
        .clone()
        .prune(0, 0, |handle| {
            assert!(observer.get(&handle).is_err());
            // The next eligible victim must still exist at notification time.
            if announced.is_empty() {
                assert!(observer.get(&manifests[3].handle).is_ok());
            }
            announced.push(handle);
            std::future::ready(Ok(()))
        })
        .await
        .unwrap();
    assert_eq!(
        announced,
        vec![
            manifests[1].handle.clone(),
            manifests[3].handle.clone(),
            manifests[4].handle.clone()
        ]
    );
    assert_eq!(session.list().unwrap().len(), 2);
    assert!(session.get(&manifests[0].handle).is_ok());
    assert!(session.get(&manifests[2].handle).is_ok());
}

#[tokio::test]
async fn retention_stops_on_notification_failure_without_leaking_callback_errors() {
    let (_root, session, manifests) = retention_session();
    let error = session
        .clone()
        .prune(0, 0, |_| async { Err("private callback details".into()) })
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "snapshot_retention_announcement_failed");
    assert!(session.get(&manifests[0].handle).is_err());
    assert!(session.get(&manifests[1].handle).is_ok());
    assert_eq!(session.list().unwrap().len(), 4);
}

#[tokio::test]
async fn retention_preserves_open_restore_references() {
    let (_root, session, manifests) = retention_session();
    let mut journal = super::super::journal::Journal::new(
        session.store.workspace_id.clone(),
        manifests[1].handle.clone(),
        manifests[1].captured_by.clone(),
        None,
    );
    journal.recovery = Some(manifests[4].handle.clone());
    session.store.save_journal(&journal).unwrap();
    session
        .clone()
        .prune(0, 0, |_| async { Ok(()) })
        .await
        .unwrap();
    assert!(session.get(&manifests[1].handle).is_ok());
    assert!(session.get(&manifests[4].handle).is_ok());
    assert!(session.get(&manifests[2].handle).is_ok());
    assert_eq!(session.list().unwrap().len(), 3);
}

#[tokio::test]
async fn retention_stops_at_a_failed_delete_without_announcing_it() {
    let (_root, session, manifests) = retention_session();
    let observer = session.clone();
    let mut announced = Vec::new();
    let error = session
        .clone()
        .prune(0, 0, |handle| {
            announced.push(handle);
            // Simulate a protection changing between selection and deletion.
            observer.set_keep(&manifests[1].handle, true).unwrap();
            std::future::ready(Ok(()))
        })
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "snapshot_retention_failed");
    assert_eq!(announced, vec![manifests[0].handle.clone()]);
    assert_eq!(session.list().unwrap().len(), 4);
}

#[test]
fn session_holds_both_locks_and_refuses_backups() {
    let root = tempfile::tempdir().unwrap();
    let id = "demo-workspace";
    let slots = ArchiveJobSlot::default();
    let session = Session::open(
        Store::at(root.path().into(), id).unwrap(),
        &slots,
        ArchiveJobKind::SnapshotCapture,
        &TestKeyStore,
        "demo",
    )
    .unwrap();
    assert!(slots.try_acquire(ArchiveJobKind::Backup).is_none());
    let other_slots = ArchiveJobSlot::default();
    let refused = Session::open(
        Store::at(root.path().into(), id).unwrap(),
        &other_slots,
        ArchiveJobKind::SnapshotCheck,
        &TestKeyStore,
        "demo",
    );
    assert_eq!(refused.err().unwrap().to_string(), "archive_job_busy");
    assert!(!other_slots.is_busy());
    assert_eq!(
        session.get(&Handle::generate()).unwrap_err().to_string(),
        "snapshot_unavailable"
    );
    drop(session);
    assert!(!slots.is_busy());
    assert!(Session::open(
        Store::at(root.path().into(), id).unwrap(),
        &slots,
        ArchiveJobKind::SnapshotCheck,
        &TestKeyStore,
        "demo"
    )
    .is_ok());
}

#[tokio::test]
async fn capture_publishes_actual_reports_and_check_detects_tampering() {
    use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
    let (pool, database) = setup_test_db().await;
    let secret = "snapshot-synthetic-plaintext-canary";
    crate::test_support::seed_credential(
        &pool,
        "snapshot-canary",
        "https://example.org",
        crate::core::AuthType::Bearer,
        secret,
    )
    .await;
    let (bus, _callbacks) = crate::engine::event_bus::EventBus::new(pool.clone());
    let mut receiver = bus.subscribe();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("note"), b"saved bytes").unwrap();
    let root = tempfile::tempdir().unwrap();
    let id = "demo-workspace";
    let key_root = tempfile::tempdir().unwrap();
    let keys = crate::core::secret_store::key_store::FileKeyStore::new(key_root.path().into());
    let session = Session::open(
        Store::at(root.path().into(), id).unwrap(),
        &ArchiveJobSlot::default(),
        ArchiveJobKind::SnapshotCapture,
        &keys,
        &database,
    )
    .unwrap();
    let secret_key = crate::core::secret_store::key().unwrap();
    assert!(session.key.id() != secret_key.id());
    let needles = [
        secret.as_bytes().to_vec(),
        secret_key.as_bytes().to_vec(),
        secret_key.to_base64().into_bytes(),
        session.key.as_bytes().to_vec(),
        session.key.to_base64().into_bytes(),
        test_db_url(&database).into_bytes(),
    ];
    let contains_secret = |bytes: &[u8]| {
        needles
            .iter()
            .any(|needle| bytes.windows(needle.len()).any(|window| window == needle))
    };
    // Check the detector without ever printing a needle in a failure message.
    assert!(contains_secret(secret.as_bytes()));
    std::fs::create_dir(workspace.path().join(".lucidos")).unwrap();
    std::fs::write(
        workspace.path().join(".lucidos/secrets.key"),
        secret_key.as_bytes(),
    )
    .unwrap();
    std::fs::write(
        workspace.path().join(".lucidos/snapshot-mac.key"),
        session.key.to_base64(),
    )
    .unwrap();
    let handle = Handle::generate();
    let session = std::sync::Arc::new(session.with_events(super::super::events::JobEvents {
        bus,
        manual_retention: 5,
        recovery_retention: 3,
    }));
    let capture_job = session
        .create_job(
            handle.clone(),
            Action::Capture,
            MessageOrigin::Device {
                device_id: "test-device".into(),
            },
        )
        .unwrap();
    let completed = session
        .clone()
        .run_capture_job(
            capture_job.id,
            &pool,
            &test_db_url(&database),
            workspace.path(),
            CaptureRequest {
                handle: handle.clone(),
                workspace_name: "demo".into(),
                actor: MessageOrigin::Device {
                    device_id: "test-device".into(),
                },
                label: None,
                keep: false,
                running_threads: 2,
            },
        )
        .await
        .unwrap();
    let State::Completed { manifest } = completed.state else {
        panic!("capture failed")
    };
    assert!(matches!(
        session.job(capture_job.id).unwrap().state,
        State::Completed { .. }
    ));
    assert_eq!(manifest.files.count, 1);
    assert_eq!(manifest.files.bytes, 11);
    assert_eq!(manifest.consistency.running_threads, 2);
    assert!(manifest.table_rows.contains_key("events"));
    assert_eq!(session.list().unwrap().len(), 1);
    let unrelated = session
        .create_job(
            Handle::generate(),
            Action::Capture,
            MessageOrigin::Device {
                device_id: "test-device".into(),
            },
        )
        .unwrap();
    assert!(session
        .finish_job(
            unrelated.id,
            State::Completed {
                manifest: manifest.clone()
            }
        )
        .is_err());
    let check_job = session
        .create_job(
            handle.clone(),
            Action::Check,
            MessageOrigin::Device {
                device_id: "test-device".into(),
            },
        )
        .unwrap();
    assert!(session
        .clone()
        .run_capture_job(
            check_job.id,
            &pool,
            &test_db_url(&database),
            workspace.path(),
            CaptureRequest {
                handle: handle.clone(),
                workspace_name: "demo".into(),
                actor: MessageOrigin::Device {
                    device_id: "test-device".into()
                },
                label: None,
                keep: false,
                running_threads: 0,
            }
        )
        .await
        .is_err());
    let checked = session
        .clone()
        .run_check_job(check_job.id, &test_db_url(&database))
        .await
        .unwrap();
    assert!(matches!(checked.state, State::Completed { .. }));
    let snapshot = root.path().join(handle.as_str());
    for entry in std::fs::read_dir(&snapshot).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(!contains_secret(&bytes), "secret in raw snapshot artifact");
    }
    for name in ["files.tar.zst", "files.index.jsonl.zst"] {
        let bytes = zstd::decode_all(std::fs::File::open(snapshot.join(name)).unwrap()).unwrap();
        // The whole tar stream includes entry headers, paths and contents.
        assert!(!contains_secret(&bytes), "secret in expanded file artifact");
    }
    let expanded = crate::core::backup::pg_tool_command("pg_restore", &test_db_url(&database))
        .unwrap()
        .arg("--file=-")
        .arg(snapshot.join("db.dump"))
        .output()
        .unwrap();
    assert!(
        expanded.status.success(),
        "could not expand captured database"
    );
    assert!(
        !contains_secret(&expanded.stdout),
        "secret in expanded database"
    );
    assert!(
        !contains_secret(&expanded.stderr),
        "secret in database tool output"
    );
    assert!(
        !contains_secret(&serde_json::to_vec(&checked).unwrap()),
        "secret in check result"
    );
    assert!(
        !contains_secret(&serde_json::to_vec(&manifest).unwrap()),
        "secret in capture result"
    );
    std::fs::write(
        root.path().join(handle.as_str()).join("db.dump"),
        format!("sensitive-error-sentinel{secret}"),
    )
    .unwrap();
    let error = session
        .check(&handle, &test_db_url(&database))
        .unwrap_err()
        .to_string();
    assert_eq!(error, "snapshot_check_failed");
    assert!(!error.contains("sensitive-error-sentinel"));
    let check_job = session
        .create_job(
            handle.clone(),
            Action::Check,
            MessageOrigin::Device {
                device_id: "test-device".into(),
            },
        )
        .unwrap();
    let failed = session
        .clone()
        .run_check_job(check_job.id, &test_db_url(&database))
        .await
        .unwrap();
    assert!(matches!(
        failed.state,
        State::Failed {
            reason: Failure::CheckFailed
        }
    ));
    assert!(
        !contains_secret(&serde_json::to_vec(&failed).unwrap()),
        "secret in failure result"
    );
    for job in [capture_job.id, check_job.id] {
        assert!(
            !contains_secret(&serde_json::to_vec(&session.job(job).unwrap()).unwrap()),
            "secret in durable job"
        );
    }
    assert!(!serde_json::to_string(&failed)
        .unwrap()
        .contains("sensitive-error-sentinel"));
    for expected in [
        "SnapshotProgress",
        "SnapshotCreated",
        "SnapshotProgress",
        "SnapshotChecked",
        "SnapshotProgress",
        "SnapshotChecked",
    ] {
        let event = receiver.try_recv().unwrap();
        let crate::engine::event_bus::BusEvent::System(system) = event.typed else {
            panic!("unexpected thread event");
        };
        assert_eq!(system.event_type(), expected);
        let value = serde_json::to_value(&system).unwrap();
        assert!(
            !contains_secret(&serde_json::to_vec(&system).unwrap()),
            "secret in event"
        );
        assert_eq!(value["data"]["actor"]["device_id"], "test-device");
        assert!(!value.to_string().contains("sensitive-error-sentinel"));
    }
    assert!(receiver.try_recv().is_err());
    pool.close().await;
    teardown_test_db(&database).await;
}

#[tokio::test]
async fn canceled_caller_retains_locks_until_capture_finishes() {
    use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
    use std::{sync::Arc, time::Duration};
    let (setup_pool, database) = setup_test_db().await;
    let (bus, _callbacks) = crate::engine::event_bus::EventBus::new(setup_pool.clone());
    let mut receiver = bus.subscribe();
    let url = test_db_url(&database);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let held_connection = pool.acquire().await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let slots = ArchiveJobSlot::default();
    let workspace_id = "demo-workspace";
    let session = Arc::new(
        Session::open(
            Store::at(root.path().into(), workspace_id).unwrap(),
            &slots,
            ArchiveJobKind::SnapshotCapture,
            &TestKeyStore,
            &database,
        )
        .unwrap()
        .with_events(super::super::events::JobEvents {
            bus,
            manual_retention: 5,
            recovery_retention: 3,
        }),
    );
    let job = session
        .create_job(
            Handle::generate(),
            Action::Capture,
            MessageOrigin::Device {
                device_id: "test-device".into(),
            },
        )
        .unwrap();
    let caller_session = session.clone();
    let caller_pool = pool.clone();
    let path = workspace.path().to_owned();
    let caller = tokio::spawn(async move {
        caller_session
            .run_capture_job(
                job.id,
                &caller_pool,
                &url,
                &path,
                CaptureRequest {
                    handle: Handle::generate(),
                    workspace_name: "demo".into(),
                    actor: MessageOrigin::Device {
                        device_id: "test-device".into(),
                    },
                    label: None,
                    keep: false,
                    running_threads: 0,
                },
            )
            .await
    });
    // Three owners means the outer caller has entered the job and spawned
    // the worker, which cannot finish while we hold the sole connection.
    tokio::time::timeout(Duration::from_secs(10), async {
        while Arc::strong_count(&session) < 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    drop(session);
    assert!(slots.is_busy());
    let status_store = Store::at(root.path().into(), workspace_id).unwrap();
    assert!(matches!(
        jobs::read(&status_store, job.id).unwrap().state,
        State::Running
    ));
    drop(held_connection);
    tokio::time::timeout(Duration::from_secs(60), async {
        while slots.is_busy() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let reopened = Session::open(
        Store::at(root.path().into(), workspace_id).unwrap(),
        &slots,
        ArchiveJobKind::SnapshotCheck,
        &TestKeyStore,
        &database,
    )
    .unwrap();
    assert!(matches!(
        reopened.job(job.id).unwrap().state,
        State::Completed { .. }
    ));
    let progress = receiver.try_recv().unwrap();
    let completed = receiver.try_recv().unwrap();
    assert!(matches!(
        progress.typed,
        crate::engine::event_bus::BusEvent::System(
            crate::engine::event_bus::SystemEvent::SnapshotProgress { .. }
        )
    ));
    assert!(matches!(
        completed.typed,
        crate::engine::event_bus::BusEvent::System(
            crate::engine::event_bus::SystemEvent::SnapshotCreated { .. }
        )
    ));
    assert!(completed.seq.is_some());
    drop(reopened);
    pool.close().await;
    setup_pool.close().await;
    teardown_test_db(&database).await;
}

#[tokio::test]
async fn completed_capture_announces_creation_and_each_retention_deletion() {
    use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, database) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let mut receiver = bus.subscribe();
    let (_root, session, manifests) = retention_session();
    let session = std::sync::Arc::new(
        std::sync::Arc::try_unwrap(session)
            .ok()
            .unwrap()
            .with_events(super::super::events::JobEvents {
                bus,
                manual_retention: 1,
                recovery_retention: 1,
            }),
    );
    let actor = MessageOrigin::Device {
        device_id: "test-device".into(),
    };
    let job = session
        .create_job(manifests[0].handle.clone(), Action::Capture, actor)
        .unwrap();
    session
        .complete_job(
            job.id,
            State::Completed {
                manifest: Box::new(manifests[0].clone()),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap().typed,
        BusEvent::System(SystemEvent::SnapshotCreated { .. })
    ));
    let mut deleted = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        let BusEvent::System(SystemEvent::SnapshotDeleted {
            handle,
            reason: super::super::events::DeletionReason::Retention,
            actor: Some(MessageOrigin::Device { device_id }),
            ..
        }) = event.typed
        else {
            panic!("unexpected event");
        };
        assert_eq!(device_id, "test-device");
        assert!(event.seq.is_some());
        assert!(session.get(&handle).is_err());
        deleted.push(handle);
    }
    assert_eq!(
        deleted,
        vec![
            manifests[1].handle.clone(),
            manifests[3].handle.clone(),
            manifests[4].handle.clone()
        ]
    );
    assert!(session.get(&manifests[0].handle).is_ok());
    assert!(session.get(&manifests[2].handle).is_ok());
    pool.close().await;
    teardown_test_db(&database).await;
}

#[tokio::test]
async fn canceled_delete_waiter_keeps_locks_until_the_event_is_persisted() {
    use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
    use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
    use std::{sync::Arc, time::Duration};
    let (setup_pool, database) = setup_test_db().await;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&test_db_url(&database))
        .await
        .unwrap();
    let held = pool.acquire().await.unwrap();
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let mut receiver = bus.subscribe();
    let (_root, session, manifests) = retention_session();
    let session = Arc::new(Arc::try_unwrap(session).ok().unwrap().with_events(
        super::super::events::JobEvents {
            bus,
            manual_retention: 5,
            recovery_retention: 3,
        },
    ));
    let handle = manifests[0].handle.clone();
    let target = handle.clone();
    let caller_session = session.clone();
    let caller = tokio::spawn(async move {
        caller_session
            .delete_announced(
                target,
                MessageOrigin::Device {
                    device_id: "test-device".into(),
                },
            )
            .await
    });
    // The delete has happened but the event cannot reach the database yet.
    tokio::time::timeout(Duration::from_secs(10), async {
        while session.store.snapshot_dir(&handle).exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(
        Arc::strong_count(&session) > 1,
        "owned announcement retains both locks"
    );
    drop(held);
    let event = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(event.seq.is_some());
    assert!(
        matches!(event.typed, BusEvent::System(SystemEvent::SnapshotDeleted {handle: deleted, reason: super::super::events::DeletionReason::Manual, actor: Some(MessageOrigin::Device {..}), ..}) if deleted == handle)
    );
    drop(session);
    pool.close().await;
    setup_pool.close().await;
    teardown_test_db(&database).await;
}

#[tokio::test]
async fn runner_journals_original_policy_and_reuses_it_for_rollback() {
    use crate::core::snapshot::journal::{Journal, Phase};
    use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
    let (pool, database) = setup_test_db().await;
    sqlx::raw_sql("CREATE TABLE snapshot_runner_probe (value text); INSERT INTO snapshot_runner_probe VALUES ('saved');").execute(&pool).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("data")).unwrap();
    std::fs::write(workspace.path().join("data/note"), "saved").unwrap();
    std::fs::write(workspace.path().join("data/private"), "archived private").unwrap();
    let root = tempfile::tempdir().unwrap();
    let session = std::sync::Arc::new(
        Session::open(
            Store::at(root.path().into(), "demo-workspace").unwrap(),
            &ArchiveJobSlot::default(),
            ArchiveJobKind::SnapshotRestore,
            &TestKeyStore,
            &database,
        )
        .unwrap(),
    );
    let actor = MessageOrigin::Device {
        device_id: "test-device".into(),
    };
    let saved = session
        .clone()
        .capture(
            &pool,
            &test_db_url(&database),
            workspace.path(),
            CaptureRequest {
                handle: Handle::generate(),
                workspace_name: "demo".into(),
                actor: actor.clone(),
                label: None,
                keep: false,
                running_threads: 0,
            },
        )
        .await
        .unwrap();
    std::fs::write(workspace.path().join("data/note"), "current").unwrap();
    std::fs::write(workspace.path().join("data/private"), "private current").unwrap();
    std::fs::write(
        workspace.path().join("data/.backupignore"),
        "data/private\n",
    )
    .unwrap();
    sqlx::query("UPDATE snapshot_runner_probe SET value = 'current'")
        .execute(&pool)
        .await
        .unwrap();
    // B is newer than A and lives outside the database being restored.
    let later = session
        .clone()
        .capture(
            &pool,
            &test_db_url(&database),
            workspace.path(),
            CaptureRequest {
                handle: Handle::generate(),
                workspace_name: "demo".into(),
                actor: actor.clone(),
                label: None,
                keep: false,
                running_threads: 0,
            },
        )
        .await
        .unwrap();
    let mut journal = Journal::new("demo-workspace", saved.handle, actor, None);
    session.store.save_journal(&journal).unwrap();
    journal.advance(Phase::RecoveryCapture).unwrap();
    session.store.save_journal(&journal).unwrap();
    let owner = lucidos_owner_lock::OwnerLock::try_acquire(&lucidos_owner_lock::engine::lock_path(
        workspace.path(),
    ))
    .unwrap()
    .unwrap();
    assert!(session
        .clone()
        .run_restore(
            &pool,
            &test_db_url(&database),
            workspace.path(),
            OfflineRestoreRequest {
                restore_id: journal.restore_id,
                gateway_slug: "demo-workspace".into(),
                workspace_name: "demo".into()
            }
        )
        .await
        .is_err());
    drop(owner);
    for (url, identity) in [
        (test_db_url("different_snapshot_database"), "demo-workspace"),
        (test_db_url(&database), "different-workspace"),
        (test_db_url(&database), "database:demo"),
    ] {
        assert!(session
            .clone()
            .run_restore(
                &pool,
                &url,
                workspace.path(),
                OfflineRestoreRequest {
                    restore_id: journal.restore_id,
                    gateway_slug: identity.into(),
                    workspace_name: "demo".into()
                }
            )
            .await
            .is_err());
        let unchanged = session.store.journals().unwrap().remove(0);
        assert_eq!(unchanged.phase(), Phase::RecoveryCapture);
        assert!(unchanged.recovery.is_none());
    }
    // Block the owned runner at its connected-database check. Canceling the
    // requesting task must neither release the owner lock nor cancel recovery.
    let single_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&test_db_url(&database))
        .await
        .unwrap();
    let held_connection = single_pool.acquire().await.unwrap();
    let worker = session.clone();
    let worker_pool = single_pool.clone();
    let url = test_db_url(&database);
    let target = workspace.path().to_owned();
    let restore_id = journal.restore_id;
    let caller = tokio::spawn(async move {
        worker
            .run_restore(
                &worker_pool,
                &url,
                &target,
                OfflineRestoreRequest {
                    restore_id,
                    gateway_slug: "demo-workspace".into(),
                    workspace_name: "demo".into(),
                },
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let probe = lucidos_owner_lock::OwnerLock::try_acquire(
                &lucidos_owner_lock::engine::lock_path(workspace.path()),
            )
            .unwrap();
            if probe.is_none() {
                break;
            }
            drop(probe);
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(
        lucidos_owner_lock::OwnerLock::try_acquire(&lucidos_owner_lock::engine::lock_path(
            workspace.path()
        ))
        .unwrap()
        .is_none()
    );
    drop(held_connection);
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let durable = session.store.journals().unwrap().remove(0);
            if durable.phase() == Phase::FilesRestored
                && lucidos_owner_lock::OwnerLock::try_acquire(
                    &lucidos_owner_lock::engine::lock_path(workspace.path()),
                )
                .unwrap()
                .is_some()
            {
                break;
            }
            assert!(!durable.phase().is_terminal(), "unexpected restore failure");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    single_pool.close().await;
    let journal = session.store.journals().unwrap().remove(0);
    assert_eq!(journal.phase(), Phase::FilesRestored);
    let marker: super::super::first_boot::Marker = serde_json::from_slice(
        &std::fs::read(workspace.path().join(".lucidos/restored-from-snapshot")).unwrap(),
    )
    .unwrap();
    assert_eq!(marker.restore_id, journal.restore_id);
    assert_eq!(marker.snapshot, journal.snapshot);
    assert!(journal.phase().blocks_engine());
    let original = journal.files.clone().unwrap();
    assert!(!original.staging_roots.is_empty());
    assert!(original.staging_roots.iter().all(|path| path.is_dir()));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT value FROM snapshot_runner_probe")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "saved"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("data/note")).unwrap(),
        "saved"
    );
    assert!(!workspace.path().join("data/.backupignore").exists());
    assert!(session
        .list()
        .unwrap()
        .iter()
        .any(|manifest| manifest.handle == later.handle));
    assert_eq!(
        session
            .check(&later.handle, &test_db_url(&database))
            .unwrap()
            .handle,
        later.handle
    );
    reconcile_when_owner_free(
        &session,
        journal.restore_id,
        workspace.path(),
        &test_db_url(&database),
    )
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT value FROM snapshot_runner_probe")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "current"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("data/note")).unwrap(),
        "current"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("data/private")).unwrap(),
        "private current"
    );
    let mut durable = session.store.journals().unwrap().remove(0);
    assert_eq!(durable.phase(), Phase::RolledBack);
    assert_eq!(durable.files.as_ref().unwrap().policy, original.policy);
    durable.files = None;
    assert!(session.store.save_journal(&durable).is_err());
    pool.close().await;
    teardown_test_db(&database).await;
}

#[tokio::test]
async fn incompatible_restore_contract_refuses_before_recovery_or_mutation() {
    use crate::core::snapshot::journal::{Journal, Phase};
    use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
    for newer_schema in [true, false] {
        let (pool, database) = setup_test_db().await;
        let url = test_db_url(&database);
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("note"), "saved").unwrap();
        let root = tempfile::tempdir().unwrap();
        let session = std::sync::Arc::new(
            Session::open(
                Store::at(root.path().into(), "demo-workspace").unwrap(),
                &ArchiveJobSlot::default(),
                ArchiveJobKind::SnapshotRestore,
                &TestKeyStore,
                &database,
            )
            .unwrap(),
        );
        let actor = MessageOrigin::Device {
            device_id: "test-device".into(),
        };
        if newer_schema {
            sqlx::raw_sql("INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (9223372036854775807, 'synthetic future migration', true, decode('00', 'hex'), 0)")
                .execute(&pool).await.unwrap();
        }
        let mut saved = session
            .clone()
            .capture(
                &pool,
                &url,
                workspace.path(),
                CaptureRequest {
                    handle: Handle::generate(),
                    workspace_name: "demo".into(),
                    actor: actor.clone(),
                    label: None,
                    keep: false,
                    running_threads: 0,
                },
            )
            .await
            .unwrap();
        if !newer_schema {
            saved.secret_key_ids = [crate::core::secret_store::SecretKey::generate().id()].into();
        }
        // Authenticate a genuinely incompatible contract, rather than testing
        // the unrelated MAC-tampering refusal.
        let signed =
            super::super::manifest::AuthenticatedManifest::seal(saved.clone(), &session.key)
                .unwrap();
        std::fs::write(
            session
                .store
                .snapshot_dir(&saved.handle)
                .join("manifest.json"),
            serde_json::to_vec(&signed).unwrap(),
        )
        .unwrap();
        std::fs::write(workspace.path().join("note"), "current").unwrap();
        crate::test_support::seed_preference(&pool, "restore-contract-probe", "current")
            .await
            .unwrap();
        let before_files = tempfile::tempdir().unwrap();
        let before =
            crate::core::backup::capture::capture_files(workspace.path(), before_files.path())
                .unwrap();
        let before_db = tempfile::tempdir().unwrap();
        let counts = crate::core::backup::capture::capture_database(
            &pool,
            &url,
            &before_db.path().join("db.dump"),
        )
        .await
        .unwrap()
        .table_rows;
        assert!(
            session
                .preflight_restore(&saved.handle, &url, workspace.path())
                .is_err(),
            "incompatible contract admitted"
        );
        let journal = Journal::new("demo-workspace", saved.handle, actor, None);
        session.store.save_journal(&journal).unwrap();
        assert!(session
            .clone()
            .run_restore(
                &pool,
                &url,
                workspace.path(),
                OfflineRestoreRequest {
                    restore_id: journal.restore_id,
                    gateway_slug: "demo-workspace".into(),
                    workspace_name: "demo".into(),
                }
            )
            .await
            .is_err());
        let durable = session.store.journals().unwrap().remove(0);
        assert_eq!(durable.phase(), Phase::RecoveryCapture);
        assert!(durable.recovery.is_none());
        assert_eq!(session.list().unwrap().len(), 1);
        let after_files = tempfile::tempdir().unwrap();
        let after =
            crate::core::backup::capture::capture_files(workspace.path(), after_files.path())
                .unwrap();
        assert_eq!(before.tree_sha256, after.tree_sha256);
        for (table, expected) in counts {
            let quoted = table.replace('"', "\"\"");
            let count: i64 =
                sqlx::query_scalar(&format!("SELECT count(*) FROM public.\"{quoted}\""))
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(count, expected, "changed row count: {table}");
        }
        let value: String = sqlx::query_scalar(
            "SELECT value FROM preferences WHERE key = 'restore-contract-probe'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(value, "current");
        pool.close().await;
        teardown_test_db(&database).await;
    }
}

#[tokio::test]
async fn mutation_and_boot_faults_recover_or_block_normal_boot() {
    use crate::core::snapshot::journal::{Journal, Phase};
    use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
    for faults in [
        vec!["recovery-capture"],
        vec!["recovery-verify"],
        vec!["before-database"],
        vec!["after-database"],
        vec!["after-files"],
        vec!["after-marker"],
        vec!["forward-boot"],
        vec!["forward-boot", "rollback-boot"],
        vec!["after-database", "rollback-boot"],
    ] {
        let (pool, database) = setup_test_db().await;
        sqlx::raw_sql("CREATE TABLE snapshot_fault_probe (value text); INSERT INTO snapshot_fault_probe VALUES ('saved')").execute(&pool).await.unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("data")).unwrap();
        std::fs::write(workspace.path().join("data/note"), "saved").unwrap();
        let root = tempfile::tempdir().unwrap();
        let session = std::sync::Arc::new(
            Session::open(
                Store::at(root.path().into(), "demo-workspace").unwrap(),
                &ArchiveJobSlot::default(),
                ArchiveJobKind::SnapshotRestore,
                &TestKeyStore,
                &database,
            )
            .unwrap(),
        );
        let actor = MessageOrigin::Device {
            device_id: "test-device".into(),
        };
        let saved = session
            .clone()
            .capture(
                &pool,
                &test_db_url(&database),
                workspace.path(),
                CaptureRequest {
                    handle: Handle::generate(),
                    workspace_name: "demo".into(),
                    actor: actor.clone(),
                    label: None,
                    keep: false,
                    running_threads: 0,
                },
            )
            .await
            .unwrap();
        std::fs::write(workspace.path().join("data/note"), "current").unwrap();
        sqlx::query("UPDATE snapshot_fault_probe SET value = 'current'")
            .execute(&pool)
            .await
            .unwrap();
        std::fs::create_dir(workspace.path().join("data/nested")).unwrap();
        std::fs::write(workspace.path().join("data/nested/new"), "post-snapshot").unwrap();
        sqlx::query("INSERT INTO snapshot_fault_probe VALUES ('post-snapshot')")
            .execute(&pool)
            .await
            .unwrap();
        let before_files = tempfile::tempdir().unwrap();
        let before =
            crate::core::backup::capture::capture_files(workspace.path(), before_files.path())
                .unwrap();
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY tablename",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let mut before_rows = std::collections::BTreeMap::new();
        for table in tables {
            let quoted = table.replace('"', "\"\"");
            let count: i64 =
                sqlx::query_scalar(&format!("SELECT count(*) FROM public.\"{quoted}\""))
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            before_rows.insert(table, count);
        }
        let preparation_failed =
            faults.contains(&"recovery-capture") || faults.contains(&"recovery-verify");
        let mut journal = Journal::new("demo-workspace", saved.handle, actor, None);
        session.store.save_journal(&journal).unwrap();
        journal.advance(Phase::RecoveryCapture).unwrap();
        session.store.save_journal(&journal).unwrap();
        session
            .faults
            .lock()
            .unwrap()
            .extend(faults.iter().copied());
        assert!(
            session
                .clone()
                .run_restore(
                    &pool,
                    &test_db_url(&database),
                    workspace.path(),
                    OfflineRestoreRequest {
                        restore_id: journal.restore_id,
                        gateway_slug: "demo-workspace".into(),
                        workspace_name: "demo".into(),
                    }
                )
                .await
                .is_err(),
            "{faults:?}"
        );
        let durable = session.store.journals().unwrap().remove(0);
        assert_eq!(
            durable.phase(),
            if preparation_failed {
                Phase::RecoveryCapture
            } else if faults.contains(&"rollback-boot") {
                Phase::NeedsAttention
            } else {
                Phase::RolledBack
            },
            "{faults:?}"
        );
        assert_eq!(
            durable.phase().blocks_engine(),
            faults.contains(&"rollback-boot")
        );
        if preparation_failed {
            assert!(durable.recovery.is_none());
            // Verification failure occurs after publication, but must never
            // authorize staging or installing even a valid recovery archive.
            assert_eq!(
                session
                    .list()
                    .unwrap()
                    .iter()
                    .filter(|m| m.kind == Kind::Recovery)
                    .count(),
                usize::from(faults.contains(&"recovery-verify")),
            );
        } else {
            let recovery = session.get(durable.recovery.as_ref().unwrap()).unwrap();
            assert_eq!(recovery.files.tree_sha256, before.tree_sha256);
            for (table, expected) in &recovery.table_rows {
                assert_eq!(
                    before_rows.get(table),
                    Some(expected),
                    "{table}, {faults:?}"
                );
            }
        }
        for (table, expected) in &before_rows {
            let quoted = table.replace('"', "\"\"");
            let count: i64 =
                sqlx::query_scalar(&format!("SELECT count(*) FROM public.\"{quoted}\""))
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(count, *expected, "changed row count: {table}, {faults:?}");
        }
        let after_files = tempfile::tempdir().unwrap();
        let after =
            crate::core::backup::capture::capture_files(workspace.path(), after_files.path())
                .unwrap();
        assert_eq!(after.tree_sha256, before.tree_sha256, "{faults:?}");
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT value FROM snapshot_fault_probe WHERE value = 'current'"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            "current",
            "{faults:?}"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("data/note")).unwrap(),
            "current",
            "{faults:?}"
        );
        assert!(!workspace
            .path()
            .join(".lucidos/restored-from-snapshot")
            .exists());
        assert!(lucidos_owner_lock::OwnerLock::try_acquire(
            &lucidos_owner_lock::engine::lock_path(workspace.path())
        )
        .unwrap()
        .is_some());
        pool.close().await;
        teardown_test_db(&database).await;
    }
}

#[tokio::test]
async fn interrupted_preparation_settles_without_connecting_or_replaying() {
    use super::super::journal::{Journal, Phase};
    for phase in [Phase::Requested, Phase::RecoveryCapture, Phase::Staging] {
        let workspace = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let session = std::sync::Arc::new(
            Session::open(
                Store::at(root.path().into(), "demo-workspace").unwrap(),
                &ArchiveJobSlot::default(),
                ArchiveJobKind::SnapshotRestore,
                &TestKeyStore,
                "snapshot_reconcile_test",
            )
            .unwrap(),
        );
        let mut journal = Journal::new(
            "demo-workspace",
            Handle::generate(),
            MessageOrigin::Device {
                device_id: "test-device".into(),
            },
            None,
        );
        session.store.save_journal(&journal).unwrap();
        if phase != Phase::Requested {
            journal.advance(Phase::RecoveryCapture).unwrap();
            session.store.save_journal(&journal).unwrap();
        }
        if phase == Phase::Staging {
            journal.recovery = Some(Handle::generate());
            journal.advance(Phase::Staging).unwrap();
        }
        session.store.save_journal(&journal).unwrap();
        let owner = lucidos_owner_lock::OwnerLock::try_acquire(
            &lucidos_owner_lock::engine::lock_path(workspace.path()),
        )
        .unwrap()
        .unwrap();
        assert!(session
            .clone()
            .reconcile_interrupted_restore(journal.restore_id, workspace.path(), "invalid-url")
            .await
            .is_err());
        assert_eq!(session.store.journals().unwrap()[0].phase(), phase);
        drop(owner);
        reconcile_when_owner_free(
            &session,
            journal.restore_id,
            workspace.path(),
            "invalid-url",
        )
        .await
        .unwrap();
        let expected = if phase == Phase::Requested {
            Phase::Refused
        } else {
            Phase::FailedUnchanged
        };
        assert_eq!(session.store.journals().unwrap()[0].phase(), expected);
        // Restart recovery is idempotent and does not require any archived files.
        reconcile_when_owner_free(
            &session,
            journal.restore_id,
            workspace.path(),
            "invalid-url",
        )
        .await
        .unwrap();
        assert!(session.list().unwrap().is_empty());
    }
}

#[test]
fn restore_admission_is_durable_exclusive_and_refuses_unknown_archives() {
    use crate::core::snapshot::journal::{Journal, Phase};
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session = Session::open(
        Store::at(root.path().join("store"), "target").unwrap(),
        &ArchiveJobSlot::default(),
        ArchiveJobKind::SnapshotRestore,
        &TestKeyStore,
        "snapshot_admission_test",
    )
    .unwrap();
    let journal = Journal::new(
        "target",
        Handle::generate(),
        MessageOrigin::Device {
            device_id: "test-device".into(),
        },
        None,
    );
    assert!(session
        .preflight_restore(&journal.snapshot, "invalid-private-url", workspace.path())
        .is_err());
    assert!(session.store.journals().unwrap().is_empty());
    session.admit_restore(&journal).unwrap();
    let reopened = Store::existing_at(session.store.root().to_owned(), "target".into()).unwrap();
    let durable = reopened.journals().unwrap().remove(0);
    assert_eq!(durable.restore_id, journal.restore_id);
    assert_eq!(durable.phase(), Phase::Requested);
    assert_eq!(durable.actor, journal.actor);
    assert!(session.admit_restore(&journal).is_err());
    let other = Journal::new("target", Handle::generate(), journal.actor.clone(), None);
    assert!(session.admit_restore(&other).is_err());
    assert_eq!(reopened.journals().unwrap().len(), 1);

    // A restore the gateway never took settles, and stops blocking a new one.
    session.refuse_restore(journal.restore_id).unwrap();
    assert_eq!(reopened.journals().unwrap()[0].phase(), Phase::Refused);
    assert!(session.refuse_restore(journal.restore_id).is_err());
    session.admit_restore(&other).unwrap();
}

/// A capture that fails after it began writing must not leave its bytes: the
/// disk floor never counts them, and boot clears only hour-old partials.
#[tokio::test]
async fn a_failed_capture_leaves_no_partial_directory() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, database) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let session = std::sync::Arc::new(
        Session::open(
            Store::at(root.path().into(), "demo-workspace").unwrap(),
            &ArchiveJobSlot::default(),
            ArchiveJobKind::SnapshotCapture,
            &TestKeyStore,
            &database,
        )
        .unwrap(),
    );
    let handle = Handle::generate();
    let unreachable = "postgres://nobody:wrong@127.0.0.1:1/missing";
    assert!(session
        .clone()
        .capture(
            &pool,
            unreachable,
            workspace.path(),
            CaptureRequest {
                handle: handle.clone(),
                workspace_name: "demo".into(),
                actor: MessageOrigin::Device {
                    device_id: "test-device".into(),
                },
                label: None,
                keep: false,
                running_threads: 0,
            },
        )
        .await
        .is_err());
    assert!(!root.path().join(".partial").join(handle.as_str()).exists());
    assert_eq!(
        std::fs::read_dir(root.path().join(".partial"))
            .unwrap()
            .count(),
        0
    );
    drop(session);
    pool.close().await;
    teardown_test_db(&database).await;
}

/// A workspace whose estimate exceeds the free disk fails with the shortfall
/// rather than a bare capture failure. A sparse file sets the estimate without
/// using disk.
#[tokio::test]
async fn a_capture_refused_by_the_disk_floor_names_the_space_to_free() {
    use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
    let (pool, database) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let huge = 1u64 << 41;
    std::fs::File::create(workspace.path().join("huge.bin"))
        .unwrap()
        .set_len(huge)
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let session = std::sync::Arc::new(
        Session::open(
            Store::at(root.path().into(), "demo-workspace").unwrap(),
            &ArchiveJobSlot::default(),
            ArchiveJobKind::SnapshotCapture,
            &TestKeyStore,
            &database,
        )
        .unwrap(),
    );
    let actor = MessageOrigin::Device {
        device_id: "test-device".into(),
    };
    let handle = Handle::generate();
    let job = session
        .create_job(handle.clone(), Action::Capture, actor.clone())
        .unwrap();
    let failed = session
        .clone()
        .run_capture_job(
            job.id,
            &pool,
            &test_db_url(&database),
            workspace.path(),
            CaptureRequest {
                handle,
                workspace_name: "demo".into(),
                actor,
                label: None,
                keep: false,
                running_threads: 0,
            },
        )
        .await
        .unwrap();
    let State::Failed {
        reason: Failure::DiskFloor { needed_bytes },
    } = failed.state
    else {
        panic!("expected a disk floor failure, got {:?}", failed.state);
    };
    assert!(needed_bytes > huge / 2, "{needed_bytes}");
    drop(session);
    pool.close().await;
    teardown_test_db(&database).await;
}
