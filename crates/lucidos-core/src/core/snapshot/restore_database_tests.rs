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
    sqlx::raw_sql("ALTER TABLE push_subscriptions DROP COLUMN scope_url; DELETE FROM _sqlx_migrations WHERE version = 20260621193600")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO devices (id) VALUES ('revoked-device')")
        .execute(&pool)
        .await
        .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    sqlx::raw_sql(
        "DROP TABLE backup_origin; DELETE FROM _sqlx_migrations WHERE version = 20261125170055",
    )
    .execute(&pool)
    .await
    .unwrap();
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
    crate::core::store::MIGRATOR.run(&pool).await.unwrap();
    let origin = crate::core::backup::resolve_backup_origin(&pool, workspace.path())
        .await
        .unwrap();
    sqlx::raw_sql("DELETE FROM devices; INSERT INTO devices (id) VALUES ('current-device'); INSERT INTO device_presence (device_id) VALUES ('current-device'); INSERT INTO push_subscriptions (endpoint, p256dh, auth, device_id) VALUES ('https://push.example.test', 'test-public-key', 'lsec1:test-key:test-ciphertext', 'current-device'); INSERT INTO preferences (key,value) VALUES ('restore-test', 'current')")
        .execute(&pool).await.unwrap();
    sqlx::raw_sql("UPDATE push_subscriptions SET scope_url = 'https://scope.example.test/'")
        .execute(&pool)
        .await
        .unwrap();
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
    let access_count: i64 = sqlx::query_scalar("SELECT count(*) FROM devices")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(access_count, 0);
    assert!(finish_access_after_migrations(&pool).await.is_err());
    restore(&url, &dump, &manifest.table_rows).unwrap();
    crate::core::store::MIGRATOR.run(&pool).await.unwrap();
    finish_access_after_migrations(&pool).await.unwrap();
    finish_access_after_migrations(&pool).await.unwrap();
    assert_eq!(
        crate::core::backup::read_backup_origin(&pool, workspace.path())
            .await
            .unwrap(),
        Some(origin)
    );
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
    crate::core::store::MIGRATOR.run(&pool).await.unwrap();
    finish_access_after_migrations(&pool).await.unwrap();
    assert_eq!(policy_rows().await, ["ask"]);

    sqlx::query("DELETE FROM preferences WHERE key = $1")
        .bind(policy)
        .execute(&pool)
        .await
        .unwrap();
    restore(&url, &dump, &manifest.table_rows).unwrap();
    crate::core::store::MIGRATOR.run(&pool).await.unwrap();
    finish_access_after_migrations(&pool).await.unwrap();
    assert!(policy_rows().await.is_empty());
    drop(session);
    pool.close().await;
    teardown_test_db(&database).await;
}

#[tokio::test]
async fn a_cluster_outside_utc_still_carries_current_access() {
    carries_current_access_on_a_cluster_outside_utc(false).await;
}

#[tokio::test]
async fn staging_an_older_build_wrote_outside_utc_still_carries_current_access() {
    carries_current_access_on_a_cluster_outside_utc(true).await;
}

async fn carries_current_access_on_a_cluster_outside_utc(restage_as_older_build: bool) {
    let (pool, database) = setup_test_db().await;
    let url = test_db_url(&database);
    sqlx::raw_sql(&format!(
        "ALTER DATABASE \"{database}\" SET TimeZone = 'Pacific/Auckland'"
    ))
    .execute(&pool)
    .await
    .unwrap();
    crate::test_support::seed_device(&pool, "current-device", Some("Mozilla/5.0"), None).await;
    let access = || async {
        sqlx::query_scalar::<_, serde_json::Value>("SELECT to_jsonb(d) FROM devices d ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap()
    };
    let current = access().await;
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

    restore(&url, &dump, &manifest.table_rows).unwrap();
    if restage_as_older_build {
        let staged = || async {
            sqlx::query_scalar::<_, String>(
                "SELECT value::text FROM snapshot_restore_access.rows ORDER BY table_name, value::text",
            )
            .fetch_all(&pool)
            .await
            .unwrap()
        };
        let in_utc = staged().await;
        let mut restage = String::from("BEGIN; SET LOCAL TimeZone = 'Pacific/Auckland';");
        for table in ACCESS_TABLES {
            restage.push_str(&format!(
                "UPDATE snapshot_restore_access.rows SET value = to_jsonb(jsonb_populate_record(NULL::public.{table}, value)) WHERE table_name = '{table}';"
            ));
        }
        restage.push_str("COMMIT;");
        sqlx::raw_sql(&restage).execute(&pool).await.unwrap();
        assert_ne!(staged().await, in_utc);
    }
    crate::core::store::MIGRATOR.run(&pool).await.unwrap();
    finish_access_after_migrations(&pool).await.unwrap();
    assert_eq!(access().await, current);
    drop(session);
    pool.close().await;
    teardown_test_db(&database).await;
}
