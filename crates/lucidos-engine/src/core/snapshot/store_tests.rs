use super::*;
use crate::core::{
    backup::capture::capture_files,
    snapshot::{
        check::component_hash,
        manifest::{self, ComponentName},
    },
};

fn populated(store: &Store, key: &SecretKey) -> Manifest {
    let mut manifest = manifest::fixture();
    manifest.workspace_id = store.workspace_id.clone();
    let partial = store.begin(&manifest.handle).unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("note"), b"saved bytes").unwrap();
    let files = capture_files(workspace.path(), &partial).unwrap();
    manifest.files = manifest::Files {
        count: files.count,
        bytes: files.bytes,
        tree_sha256: files.tree_sha256,
    };
    // Unit tests cover publication/index integrity independently of PostgreSQL.
    std::fs::write(partial.join("db.dump"), b"synthetic dump").unwrap();
    for component in &mut manifest.components {
        (component.bytes, component.sha256) =
            component_hash(&partial.join(component.name.as_str())).unwrap();
    }
    store
        .publish(
            &AuthenticatedManifest::seal(manifest.clone(), key).unwrap(),
            key,
        )
        .unwrap();
    manifest
}

#[test]
fn registry_is_external_atomic_authenticated_and_owner_only() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::at(root.path().join("registry"), "demo-workspace").unwrap();
    let key = SecretKey::generate();
    let partial = store.begin(&Handle::generate()).unwrap();
    assert!(store.list(&key).unwrap().is_empty());
    assert!(partial.exists());
    let a = populated(&store, &key);
    let b = populated(&store, &key);
    assert_eq!(store.list(&key).unwrap().len(), 2);
    assert!(store.get(&a.handle, &SecretKey::generate()).is_err());
    assert_eq!(store.workspace_id, "demo-workspace");
    let foreign = Store::at(store.root().to_owned(), "another-workspace").unwrap();
    assert!(foreign.get(&a.handle, &key).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(store.root())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for name in [
            "db.dump",
            "files.tar.zst",
            "files.index.jsonl.zst",
            "manifest.json",
        ] {
            assert_eq!(
                std::fs::metadata(store.snapshot_dir(&a.handle).join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    std::fs::write(
        store
            .snapshot_dir(&a.handle)
            .join(ComponentName::Files.as_str()),
        b"tampered",
    )
    .unwrap();
    assert!(super::super::check::verify_components(&store.snapshot_dir(&a.handle), &a).is_err());
    store.delete(&a.handle, &key).unwrap();
    assert!(store.get(&b.handle, &key).is_ok());
    assert_eq!(store.list(&key).unwrap().len(), 1);
}

#[test]
fn retention_and_floor_respect_protections_and_boundaries() {
    let mut items = Vec::new();
    for index in 0..8 {
        let mut item = manifest::fixture();
        item.created_at += chrono::Duration::seconds(index);
        items.push(item);
    }
    items[7].keep = true;
    let referenced = BTreeSet::from([items[6].handle.clone()]);
    let victims = retention_victims(&items, &referenced, 2, 0);
    assert_eq!(victims.len(), 4);
    assert!(!victims.contains(&items[7].handle));
    assert!(!victims.contains(&items[6].handle));
    for item in &mut items {
        item.kind = Kind::Recovery;
        item.restore_id = Some(Uuid::new_v4());
    }
    let victims = retention_victims(&items, &BTreeSet::new(), 0, 0);
    assert!(!victims.contains(&items[7].handle));
    assert!(!victims.contains(&items[6].handle));
    let gib = 1024 * 1024 * 1024;
    assert!(check_floor(6 * gib, 20 * gib, gib).is_ok());
    assert!(check_floor(6 * gib - 1, 20 * gib, gib).is_err());
    assert!(check_floor(11 * gib, 100 * gib, gib).is_ok());
    assert!(check_floor(11 * gib - 1, 100 * gib, gib).is_err());
    assert!(check_floor(0, u64::MAX, u64::MAX).is_err());
}

#[test]
fn journal_survives_reopen_and_protects_both_handles() {
    use super::super::journal::Phase;
    let root = tempfile::tempdir().unwrap();
    let id = "demo-workspace";
    let key = SecretKey::generate();
    let store = Store::at(root.path().join("registry"), id).unwrap();
    let a = populated(&store, &key);
    let b = populated(&store, &key);
    let lock = store.try_lock().unwrap();
    assert!(store.try_lock().is_err());
    drop(lock);
    assert!(store.try_lock().is_ok());
    let mut journal = Journal::new(id, a.handle.clone(), a.captured_by, None);
    store.save_journal(&journal).unwrap();
    journal.advance(Phase::RecoveryCapture).unwrap();
    journal.recovery = Some(b.handle.clone());
    store.save_journal(&journal).unwrap();
    let reopened = Store::at(root.path().join("registry"), id).unwrap();
    assert_eq!(
        reopened.journals().unwrap()[0].phase(),
        Phase::RecoveryCapture
    );
    assert!(reopened.delete(&a.handle, &key).is_err());
    assert!(reopened.delete(&b.handle, &key).is_err());
    let mut changed = journal.clone();
    changed.reason = Some("changed attribution".into());
    assert!(store.save_journal(&changed).is_err());
    let mut changed = journal.clone();
    changed.recovery = Some(Handle::generate());
    assert!(store.save_journal(&changed).is_err());
    journal.advance(Phase::FailedUnchanged).unwrap();
    store.save_journal(&journal).unwrap();
    reopened.delete(&a.handle, &key).unwrap();
    assert!(!store.snapshot_dir(&a.handle).exists());
    assert!(!store
        .root()
        .join(".partial")
        .join(a.handle.as_str())
        .exists());
    assert!(reopened.get(&b.handle, &key).is_ok());
}

#[tokio::test]
async fn real_dump_publishes_checks_and_refuses_tampering() {
    use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
    let (pool, database) = setup_test_db().await;
    let root = tempfile::tempdir().unwrap();
    let store = Store::at(root.path().join("snapshots"), "demo-workspace").unwrap();
    let key = SecretKey::generate();
    let mut manifest = manifest::fixture();
    manifest.workspace_id = store.workspace_id.clone();
    let partial = store.begin(&manifest.handle).unwrap();
    let db = crate::core::backup::capture::capture_database(
        &pool,
        &test_db_url(&database),
        &partial.join("db.dump"),
    )
    .await
    .unwrap();
    manifest.table_rows = db.table_rows;
    manifest.secret_key_ids = db.secret_key_ids;
    manifest.schema_version = db.schema_version;
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("note"), b"archived bytes").unwrap();
    let files = capture_files(workspace.path(), &partial).unwrap();
    manifest.files = manifest::Files {
        count: files.count,
        bytes: files.bytes,
        tree_sha256: files.tree_sha256,
    };
    for component in &mut manifest.components {
        (component.bytes, component.sha256) =
            component_hash(&partial.join(component.name.as_str())).unwrap();
    }
    let signed = AuthenticatedManifest::seal(manifest.clone(), &key).unwrap();
    store.publish(&signed, &key).unwrap();
    let checked =
        super::super::check::check(&store, &manifest.handle, &key, &test_db_url(&database))
            .unwrap();
    assert!(checked.verified.unwrap().mac);
    let path = store.snapshot_dir(&manifest.handle).join("manifest.json");
    let mut edited = serde_json::to_value(&signed).unwrap();
    edited["keep"] = serde_json::json!(true);
    write_json(&path, &edited).unwrap();
    assert!(store.get(&manifest.handle, &key).is_err());
    let mut edited = serde_json::to_value(&signed).unwrap();
    edited["extra"] = serde_json::json!("unrecognized metadata");
    write_json(&path, &edited).unwrap();
    assert!(store.get(&manifest.handle, &key).is_err());
    write_json(&path, &signed).unwrap();
    std::fs::write(
        store.snapshot_dir(&manifest.handle).join("db.dump"),
        b"tampered",
    )
    .unwrap();
    assert!(
        super::super::check::check(&store, &manifest.handle, &key, &test_db_url(&database))
            .is_err()
    );
    pool.close().await;
    teardown_test_db(&database).await;
}

#[test]
fn keep_is_authenticated_and_pruning_never_removes_the_newest_recovery() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::at(root.path().join("snapshots"), "demo-workspace").unwrap();
    let key = SecretKey::generate();
    let a = populated(&store, &key);
    let b = populated(&store, &key);
    store.set_keep(&a.handle, &key, true).unwrap();
    assert!(store.get(&a.handle, &key).unwrap().keep);
    let victims = retention_victims(
        &store.list(&key).unwrap(),
        &store.referenced().unwrap(),
        0,
        0,
    );
    assert_eq!(victims.as_slice(), std::slice::from_ref(&b.handle));
    store.delete(&b.handle, &key).unwrap();
    assert!(store.delete(&a.handle, &key).is_err());
    let mut recovery = populated(&store, &key);
    recovery.kind = Kind::Recovery;
    recovery.restore_id = Some(Uuid::new_v4());
    write_json(
        &store.snapshot_dir(&recovery.handle).join("manifest.json"),
        &AuthenticatedManifest::seal(recovery.clone(), &key).unwrap(),
    )
    .unwrap();
    assert!(retention_victims(
        &store.list(&key).unwrap(),
        &store.referenced().unwrap(),
        0,
        0
    )
    .is_empty());
    assert!(store.delete(&recovery.handle, &key).is_err());
    let old = root
        .path()
        .join("snapshots/.partial")
        .join(Handle::generate().as_str());
    std::fs::create_dir(&old).unwrap();
    let lock = store.try_lock().unwrap();
    store
        .clean_partial(Utc::now() + chrono::Duration::hours(2), &lock)
        .unwrap();
    assert!(!old.exists());
    assert!(store.get(&a.handle, &key).is_ok());
}

#[test]
fn snapshot_mac_key_survives_replacement_of_the_secrets_key() {
    use crate::core::secret_store::key_store::{secrets_key_name, snapshot_key_name, FileKeyStore};
    let directory = tempfile::tempdir().unwrap();
    let keys = FileKeyStore::new(directory.path().join("keys"));
    let database = "snapshot_fixture";
    keys.put(&secrets_key_name(database), &SecretKey::generate())
        .unwrap();
    let first = mac_key(&keys, database).unwrap();
    assert_ne!(
        first,
        keys.get(&secrets_key_name(database)).unwrap().unwrap()
    );
    keys.delete(&secrets_key_name(database)).unwrap();
    assert_eq!(first, mac_key(&keys, database).unwrap());
    assert_eq!(
        first,
        keys.get(&snapshot_key_name(database)).unwrap().unwrap()
    );
}

#[test]
fn status_store_resolution_never_initializes_an_unused_registry() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("unused");
    assert!(Store::existing_at(root.clone(), "demo".into()).is_err());
    assert!(!root.exists());
    let store = Store::existing_at(temp.path().to_owned(), "demo".into()).unwrap();
    assert!(super::super::jobs::read(&store, uuid::Uuid::new_v4()).is_err());
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

/// A settled restore's staging trees go, but nothing a journal could name
/// outside this store's own staging, and nothing an open restore still holds.
#[test]
fn settled_restore_staging_is_discarded_and_nothing_else() {
    use super::super::{
        file_swap::FrozenPolicy,
        journal::{Phase, PreparedFiles},
    };
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let id = "demo-workspace";
    let key = SecretKey::generate();
    let store = Store::at(root.path().join("registry"), id).unwrap();
    let target = populated(&store, &key);
    let recovery = populated(&store, &key);
    let swap = outside.path().join(".lucidos-snapshot-swap-test");
    let extract = store.root().join(".partial").join("restore-test");
    let foreign = outside.path().join("my-documents");
    for dir in [&swap, &extract, &foreign] {
        std::fs::create_dir_all(dir.join("old-0")).unwrap();
    }
    let mut journal = Journal::new(id, target.handle.clone(), target.captured_by, None);
    store.save_journal(&journal).unwrap();
    journal.advance(Phase::RecoveryCapture).unwrap();
    journal.recovery = Some(recovery.handle.clone());
    store.save_journal(&journal).unwrap();
    journal.advance(Phase::Staging).unwrap();
    journal.files = Some(PreparedFiles {
        policy: FrozenPolicy::capture(workspace.path()).unwrap(),
        staging_roots: vec![swap.clone(), extract.clone(), foreign.clone()],
    });
    store.save_journal(&journal).unwrap();
    let lock = store.try_lock().unwrap();

    store.discard_settled_staging(&lock).unwrap();
    assert!(
        swap.is_dir() && extract.is_dir(),
        "an open restore keeps its staging"
    );

    journal.advance(Phase::FailedUnchanged).unwrap();
    store.save_journal(&journal).unwrap();
    store.discard_settled_staging(&lock).unwrap();
    assert!(!swap.exists());
    assert!(!extract.exists());
    assert!(foreign.is_dir(), "a path the store never named stays");
    store.discard_settled_staging(&lock).unwrap();
}
