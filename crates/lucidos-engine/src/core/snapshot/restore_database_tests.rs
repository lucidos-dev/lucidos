use super::*;
use crate::{
    core::{secret_store::key_store::TestKeyStore, snapshot::*},
    engine::thread_events::MessageOrigin,
    test_support::{setup_test_db, teardown_test_db, test_db_url},
};

#[test]
fn checks_require_counts_and_quote_identifiers() {
    let mut rows: BTreeMap<String, i64> = ACCESS_TABLES
        .into_iter()
        .map(|name| (name.to_owned(), 0))
        .collect();
    rows.insert("unusual\"table'".into(), 3);
    let (_, after) = transaction_checks(&rows).unwrap();
    assert!(after.contains("public.\"unusual\"\"table'\""));
    rows.insert("devices".into(), -1);
    assert!(transaction_checks(&rows).is_err());
    rows.remove("devices");
    assert!(transaction_checks(&rows).is_err());
    assert!(transaction_checks(&BTreeMap::new()).is_err());
}

#[tokio::test]
async fn replay_carries_current_access_and_failed_checks_roll_back_everything() {
    let (pool, database) = setup_test_db().await;
    let url = test_db_url(&database);
    // An archive from before additive access migrations must retain newer
    // access values through replay and the normal first-boot migration path.
    sqlx::raw_sql("ALTER TABLE devices DROP COLUMN pairing_label; ALTER TABLE push_subscriptions DROP COLUMN scope_url; DELETE FROM _sqlx_migrations WHERE version IN (20260926192138, 20260621193600)")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO devices (id) VALUES ('revoked-device')")
        .execute(&pool)
        .await
        .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let session = std::sync::Arc::new(
        service::Session::open(
            store::Store::at(root.path().into(), "demo").unwrap(),
            &ArchiveJobSlot::default(),
            ArchiveJobKind::SnapshotRestore,
            &TestKeyStore,
            &database,
        )
        .unwrap(),
    );
    let manifest = session
        .clone()
        .capture_recovery(
            &pool,
            &url,
            workspace.path(),
            service::RecoveryRequest {
                restore_id: uuid::Uuid::new_v4(),
                workspace_name: "demo".into(),
                actor: MessageOrigin::Device {
                    device_id: "current-device".into(),
                },
            },
        )
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    sqlx::raw_sql("DELETE FROM devices; INSERT INTO devices (id) VALUES ('current-device'); INSERT INTO device_presence (device_id) VALUES ('current-device'); INSERT INTO push_subscriptions (endpoint, p256dh, auth, device_id) VALUES ('https://push.example.test', 'test-public-key', 'lsec1:test-key:test-ciphertext', 'current-device'); INSERT INTO preferences (key,value) VALUES ('restore-test', 'current')")
        .execute(&pool).await.unwrap();
    sqlx::raw_sql("UPDATE devices SET pairing_label = 'current label'; UPDATE push_subscriptions SET scope_url = 'https://scope.example.test/'")
        .execute(&pool).await.unwrap();
    let dump = root.path().join(manifest.handle.as_str()).join("db.dump");
    let mut invalid = manifest.table_rows.clone();
    *invalid.get_mut("devices").unwrap() += 1;
    let error = restore(&url, &dump, &invalid).unwrap_err();
    assert_eq!(error.to_string(), "snapshot_database_restore_failed");
    let value: String =
        sqlx::query_scalar("SELECT value FROM preferences WHERE key='restore-test'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(value, "current");
    sqlx::raw_sql("CREATE TABLE post_snapshot_table (id integer); CREATE VIEW post_snapshot_view AS SELECT * FROM post_snapshot_table; CREATE FUNCTION public.post_snapshot_function() RETURNS integer LANGUAGE sql AS 'SELECT 1'")
        .execute(&pool)
        .await
        .unwrap();
    // Cleanup must roll back together with a failed row-count check.
    assert!(restore(&url, &dump, &invalid).is_err());
    let table: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('public.post_snapshot_table')::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(table.is_some());
    let value: String =
        sqlx::query_scalar("SELECT value FROM preferences WHERE key='restore-test'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(value, "current");
    restore(&url, &dump, &manifest.table_rows).unwrap();
    // No archived grant is exposed while the engine is still offline.
    let access_count: i64 = sqlx::query_scalar("SELECT count(*) FROM devices")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(access_count, 0);
    assert!(finish_access_after_migrations(&pool).await.is_err());
    // A rollback/retry after a crash must keep the authoritative staged rows,
    // even though public access tables were emptied by the first replay.
    restore(&url, &dump, &manifest.table_rows).unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    finish_access_after_migrations(&pool).await.unwrap();
    // First boot can crash after access installation, before marker removal.
    finish_access_after_migrations(&pool).await.unwrap();
    let label: String = sqlx::query_scalar("SELECT pairing_label FROM devices")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(label, "current label");
    let scope: String = sqlx::query_scalar("SELECT scope_url FROM push_subscriptions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(scope, "https://scope.example.test/");
    let table: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('public.post_snapshot_table')::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(table.is_none());
    let view: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('public.post_snapshot_view')::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(view.is_none());
    let function: Option<String> =
        sqlx::query_scalar("SELECT to_regprocedure('public.post_snapshot_function()')::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(function.is_none());
    let devices: Vec<String> = sqlx::query_scalar("SELECT id FROM devices")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(devices, ["current-device"]);
    let presence: String = sqlx::query_scalar("SELECT device_id FROM device_presence")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(presence, "current-device");
    let push: String = sqlx::query_scalar("SELECT device_id FROM push_subscriptions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(push, "current-device");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM preferences WHERE key='restore-test'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    drop(session);
    pool.close().await;
    teardown_test_db(&database).await;
}

/// The restore policy decides who may restore this workspace, so a restore
/// must never bring back an archived value (ADR 0612 decision 9).
#[tokio::test]
async fn replay_carries_the_current_restore_policy_forward() {
    let (pool, database) = setup_test_db().await;
    let url = test_db_url(&database);
    let policy = crate::core::prefs::SNAPSHOT_RESTORE_BY_AGENTS.key();
    sqlx::query("INSERT INTO preferences (key, value) VALUES ($1, 'allow')")
        .bind(policy)
        .execute(&pool)
        .await
        .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let session = std::sync::Arc::new(
        service::Session::open(
            store::Store::at(root.path().into(), "demo").unwrap(),
            &ArchiveJobSlot::default(),
            ArchiveJobKind::SnapshotRestore,
            &TestKeyStore,
            &database,
        )
        .unwrap(),
    );
    let manifest = session
        .clone()
        .capture_recovery(
            &pool,
            &url,
            workspace.path(),
            service::RecoveryRequest {
                restore_id: uuid::Uuid::new_v4(),
                workspace_name: "demo".into(),
                actor: MessageOrigin::Device {
                    device_id: "current-device".into(),
                },
            },
        )
        .await
        .unwrap();
    let dump = root.path().join(manifest.handle.as_str()).join("db.dump");
    let policy_rows = || async {
        sqlx::query_scalar::<_, String>("SELECT value FROM preferences WHERE key = $1")
            .bind(policy)
            .fetch_all(&pool)
            .await
            .unwrap()
    };

    sqlx::query("UPDATE preferences SET value = 'ask' WHERE key = $1")
        .bind(policy)
        .execute(&pool)
        .await
        .unwrap();
    restore(&url, &dump, &manifest.table_rows).unwrap();
    assert!(
        policy_rows().await.is_empty(),
        "the archived policy is live offline"
    );
    sqlx::migrate!().run(&pool).await.unwrap();
    finish_access_after_migrations(&pool).await.unwrap();
    assert_eq!(policy_rows().await, ["ask"]);

    // No current row means the default, never the archived 'allow'.
    sqlx::query("DELETE FROM preferences WHERE key = $1")
        .bind(policy)
        .execute(&pool)
        .await
        .unwrap();
    restore(&url, &dump, &manifest.table_rows).unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    finish_access_after_migrations(&pool).await.unwrap();
    assert!(policy_rows().await.is_empty());
    drop(session);
    pool.close().await;
    teardown_test_db(&database).await;
}
