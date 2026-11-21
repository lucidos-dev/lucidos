use super::*;
use crate::engine::chat::recovery::ChatRecoveryExt;
use crate::{core::secret_store::key_store::TestKeyStore, test_support::*};

/// HOME is scoped to a new executable, never changed in the test runner.
/// This includes archive recovery and all startup wiring after completion.
#[tokio::test]
async fn full_constructor_completes_restore_in_isolated_process() {
    crate::core::secret_store::key_store::install_test_key_store();
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
        store::{install_roomy_volume_for_test, Store},
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
    install_roomy_volume_for_test(home.clone());
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
    let old_grant = crate::api::permission_requests::tests::seed_snapshot_grant(&fixture_bus).await;
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
    let verified = RestoreBoot::acquire_for_test(&owner, &workspace, database.clone(), store)
        .unwrap()
        .verify(pool.clone())
        .await
        .unwrap();
    let generation = verified.boundary_for_test().clone();
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
    let engine = crate::engine::LucidosEngine::new_with_restore(
        workspace.clone(),
        &url,
        ctx,
        std::sync::Arc::new(crate::engine::agent_recovery::EngineAgentHost),
        verified,
    )
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
    crate::api::permission_requests::tests::assert_snapshot_grant_expired(&fixture_bus, old_grant)
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
    let replayed: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE thread_id = $1 AND sequence > $2 AND event_type IN ('MessageReceived', 'TriggerStarted', 'PermissionSpent')")
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
