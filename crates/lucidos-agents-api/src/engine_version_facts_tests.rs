use crate::test_support::{boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db};
use std::sync::{Arc, RwLock};

const SOURCE_COMMIT_FILE: &str = "source-commit";

#[tokio::test]
async fn engine_activation_fact_records_served_identity_per_boot() {
    if !in_engine_child(
        "engine_version_facts_tests::engine_activation_fact_records_served_identity_per_boot",
    )
    .await
    {
        return;
    }
    let (pool, db) = setup_test_db().await;
    let commit = "a".repeat(40);
    for with_frontend in [true, false] {
        let (engine, workspace) = boot_mock_engine(&db).await;
        if with_frontend {
            let dir = workspace.path().to_path_buf();
            std::fs::write(dir.join(SOURCE_COMMIT_FILE), &commit).unwrap();
            engine.init_served_frontend(Arc::new(RwLock::new(dir.clone())), dir);
        }
        engine.record_engine_version_activated().await;
    }
    let rows: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'data' FROM events WHERE event_type = 'EngineVersionActivated' ORDER BY sequence",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let identity = crate::build_identity();
    assert_eq!(
        rows,
        [Some(commit), None].map(|frontend_source_commit| {
            serde_json::json!({
                "build_id": identity.id,
                "engine_version": identity.engine_version,
                "release": identity.release,
                "release_dirty": identity.release_dirty,
                "frontend_source_commit": frontend_source_commit,
            })
        })
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn served_frontend_commit_fact_records_known_changes_only() {
    if !in_engine_child(
        "engine_version_facts_tests::served_frontend_commit_fact_records_known_changes_only",
    )
    .await
    {
        return;
    }
    let (pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let initial = workspace.path().join("initial");
    std::fs::create_dir(&initial).unwrap();
    let first = "a".repeat(40);
    let second = "b".repeat(40);
    std::fs::write(initial.join(SOURCE_COMMIT_FILE), &first).unwrap();
    let handle = Arc::new(RwLock::new(initial.clone()));
    engine.init_served_frontend(handle.clone(), initial.clone());
    // A boot fallback serves live dist, whose stamp can change before a swap.
    std::fs::write(initial.join(SOURCE_COMMIT_FILE), &second).unwrap();

    for (index, commit) in [second.as_str(), second.as_str(), "invalid"]
        .into_iter()
        .enumerate()
    {
        let generation = index as u64 + 1;
        let source = workspace.path().join(format!("dist-{generation}"));
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("index.html"), "<html></html>").unwrap();
        std::fs::write(source.join(SOURCE_COMMIT_FILE), commit).unwrap();
        engine.advance_served_snapshot_for_test(&source).await;
        assert_eq!(
            std::fs::read_to_string(handle.read().unwrap().join(SOURCE_COMMIT_FILE)).unwrap(),
            commit,
        );
    }

    let rows: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'data' FROM events WHERE event_type = 'ServedFrontendCommitChanged'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        [serde_json::json!({
            "previous_source_commit": first,
            "source_commit": second,
        })]
    );
    drop(engine);
    teardown_test_db(&db).await;
}
