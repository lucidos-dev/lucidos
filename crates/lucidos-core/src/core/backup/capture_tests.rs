use super::*;

#[tokio::test]
async fn captured_backup_round_trips_through_restore_archive_into() {
    use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
    let (source_pool, source) = setup_test_db().await;
    let (target_pool, target) = setup_test_db().await;
    crate::test_support::seed_preference(&source_pool, "capture-round-trip", "saved")
        .await
        .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("data/artifacts")).unwrap();
    std::fs::write(workspace.path().join("data/artifacts/note"), b"saved bytes").unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let workspace_path = workspace.path().to_path_buf();
    let scratch_path = scratch.path().to_path_buf();
    let source_url = test_db_url(&source);
    let archive = tokio::task::spawn_blocking(move || {
        capture_workspace_archive(
            &workspace_path,
            &source_url,
            &scratch_path,
            None,
            0,
            &|_, _, _| {},
        )
    })
    .await
    .unwrap()
    .unwrap();
    let (identity, recipient) = crypto::generate_key_pair();
    let encrypted = scratch.path().join("backup.enc");
    crypto::seal(
        &crypto::SealingKey::Recipient(recipient),
        std::fs::File::open(archive).unwrap(),
        &mut std::fs::File::create(&encrypted).unwrap(),
    )
    .unwrap();
    // Restore terminates target connections. Close the pool first and reconnect
    // afterwards; the key store resolves to TestKeyStore under cfg(test).
    target_pool.close().await;
    let destination = tempfile::tempdir().unwrap();
    let warnings = super::super::restore_archive_into(
        destination.path(),
        &test_db_url(&target),
        crypto::OpeningKey::Age(identity),
        &encrypted,
        |_, _, _| {},
    )
    .await
    .unwrap();
    assert!(warnings.is_empty());
    assert_eq!(
        std::fs::read(destination.path().join("data/artifacts/note")).unwrap(),
        b"saved bytes"
    );
    assert!(!destination.path().join(SECRET_STORE_KEY_ENTRY).exists());
    assert!(!destination.path().join("lucidos_backup.dump").exists());
    let restored = PgPool::connect(&test_db_url(&target)).await.unwrap();
    let value: String =
        sqlx::query_scalar("SELECT value FROM preferences WHERE key = 'capture-round-trip'")
            .fetch_one(&restored)
            .await
            .unwrap();
    assert_eq!(value, "saved");
    source_pool.close().await;
    restored.close().await;
    teardown_test_db(&source).await;
    teardown_test_db(&target).await;
}

#[test]
fn snapshot_files_have_no_backup_key_or_dump_entries() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("data")).unwrap();
    std::fs::write(workspace.path().join("data/note"), b"hello").unwrap();
    std::fs::create_dir_all(workspace.path().join(".lucidos")).unwrap();
    std::fs::write(workspace.path().join(".lucidos/secret"), b"excluded").unwrap();
    let output = tempfile::tempdir().unwrap();
    let report = capture_files(workspace.path(), output.path()).unwrap();
    assert_eq!(report.files_changed_during_read, 0);
    assert_eq!(report.count, 1);
    assert_eq!(report.bytes, 5);
    assert!(report.files_finished_at >= report.files_started_at);
    let decoder =
        zstd::Decoder::new(std::fs::File::open(output.path().join("files.tar.zst")).unwrap())
            .unwrap();
    let mut archive = tar::Archive::new(decoder);
    let names: Vec<_> = archive
        .entries()
        .unwrap()
        .map(|entry| entry.unwrap().path().unwrap().into_owned())
        .collect();
    assert_eq!(names, [PathBuf::from("data/note")]);
    let index =
        zstd::decode_all(std::fs::File::open(output.path().join("files.index.jsonl.zst")).unwrap())
            .unwrap();
    let entry: FileIndexEntry = serde_json::from_slice(&index).unwrap();
    use sha2::{Digest, Sha256};
    assert_eq!(report.tree_sha256, format!("{:x}", Sha256::digest(&index)));
    assert_eq!(entry.path, PathBuf::from("data/note"));
    assert_eq!(entry.size, 5);
    assert_eq!(
        entry.sha256,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
}

#[tokio::test]
async fn database_dump_matches_captured_counts_and_key_ids() {
    use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
    let (pool, source) = setup_test_db().await;
    let (restored, target) = setup_test_db().await;
    crate::test_support::seed_credential(
        &pool,
        "snapshot-fixture",
        "https://example.org",
        crate::core::AuthType::Bearer,
        "synthetic-token",
    )
    .await;
    sqlx::query("CREATE TABLE capture_probe (id BIGINT GENERATED ALWAYS AS IDENTITY)")
        .execute(&pool)
        .await
        .unwrap();
    let stop = tokio_util::sync::CancellationToken::new();
    let writer = {
        let pool = pool.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            let mut writes = 0;
            while !stop.is_cancelled() {
                sqlx::query("INSERT INTO capture_probe DEFAULT VALUES")
                    .execute(&pool)
                    .await
                    .unwrap();
                writes += 1;
            }
            writes
        })
    };
    let output = tempfile::tempdir().unwrap();
    let dump = output.path().join("db.dump");
    let metadata = capture_database(&pool, &test_db_url(&source), &dump)
        .await
        .unwrap();
    stop.cancel();
    assert!(writer.await.unwrap() > 0);
    assert!(metadata.schema_version > 0);
    assert!(metadata
        .secret_key_ids
        .contains(&crate::core::secret_store::key().unwrap().id()));
    let target_url = test_db_url(&target);
    tokio::task::spawn_blocking(move || super::super::pg_restore(&target_url, &dump))
        .await
        .unwrap()
        .unwrap();
    for (table, expected) in metadata.table_rows {
        let actual: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM public.{}",
            quote_identifier(&table)
        ))
        .fetch_one(&restored)
        .await
        .unwrap();
        assert_eq!(actual, expected, "row count differs for {table}");
    }
    pool.close().await;
    restored.close().await;
    teardown_test_db(&source).await;
    teardown_test_db(&target).await;
}

#[test]
fn snapshot_output_cannot_be_inside_the_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    let output = workspace.path().join("output");
    std::fs::create_dir(&output).unwrap();
    assert!(capture_files(workspace.path(), &output).is_err());
    assert!(!output.join("files.tar.zst").exists());
}

#[cfg(unix)]
#[test]
fn file_index_is_stable_and_preserves_executable_mode() {
    use std::os::unix::fs::PermissionsExt;
    let workspace = tempfile::tempdir().unwrap();
    let script = workspace.path().join("script.sh");
    std::fs::write(&script, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o750)).unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let a = capture_files(workspace.path(), first.path()).unwrap();
    let b = capture_files(workspace.path(), second.path()).unwrap();
    assert_eq!(a.tree_sha256, b.tree_sha256);
    assert_eq!(a.count, 1);
    assert_eq!(a.bytes, 10);
    let index =
        zstd::decode_all(std::fs::File::open(first.path().join("files.index.jsonl.zst")).unwrap())
            .unwrap();
    let entry: FileIndexEntry = serde_json::from_slice(&index).unwrap();
    assert_eq!(entry.mode, 0o750);
}
