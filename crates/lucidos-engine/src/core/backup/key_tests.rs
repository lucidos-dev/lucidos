use super::*;
use crate::core::secret_store::key_store::FileKeyStore;

const DATABASE: &str = "lucidos_ws";

struct Fixture {
    workspace: tempfile::TempDir,
    keys: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            workspace: tempfile::tempdir().unwrap(),
            keys: tempfile::tempdir().unwrap(),
        }
    }

    fn file_store(&self) -> FileKeyStore {
        FileKeyStore::new(self.keys.path().to_path_buf())
    }

    fn backup_keys(&self) -> BackupKeyStore {
        BackupKeyStore::new(Box::new(self.file_store()), DATABASE, self.workspace.path())
    }

    fn legacy_file(&self) -> PathBuf {
        legacy_key_file(self.workspace.path())
    }

    fn write_legacy_file(&self, key: &SecretKey) {
        std::fs::create_dir_all(self.legacy_file().parent().unwrap()).unwrap();
        std::fs::write(self.legacy_file(), key.to_base64()).unwrap();
    }

    fn stored(&self) -> Option<SecretKey> {
        self.file_store().get(&backup_key_name(DATABASE)).unwrap()
    }

    fn store(&self, key: &SecretKey) {
        self.file_store()
            .put(&backup_key_name(DATABASE), key)
            .unwrap();
    }
}

#[test]
fn a_legacy_file_alone_moves_into_the_store_and_is_deleted() {
    let fixture = Fixture::new();
    let key = SecretKey::generate();
    fixture.write_legacy_file(&key);

    assert_eq!(
        fixture.backup_keys().move_legacy_file().unwrap(),
        LegacyKeyFile::Moved
    );
    assert_eq!(fixture.stored(), Some(key));
    assert!(!fixture.legacy_file().exists());
}

#[test]
fn a_key_in_the_store_alone_is_left_untouched() {
    let fixture = Fixture::new();
    let key = SecretKey::generate();
    fixture.store(&key);

    let keys = fixture.backup_keys();
    assert_eq!(keys.move_legacy_file().unwrap(), LegacyKeyFile::Absent);
    assert_eq!(keys.load().unwrap(), Some(key));
}

#[test]
fn a_legacy_file_matching_the_store_is_deleted() {
    let fixture = Fixture::new();
    let key = SecretKey::generate();
    fixture.store(&key);
    fixture.write_legacy_file(&key);

    assert_eq!(
        fixture.backup_keys().move_legacy_file().unwrap(),
        LegacyKeyFile::Duplicate
    );
    assert_eq!(fixture.stored(), Some(key));
    assert!(!fixture.legacy_file().exists());
}

#[test]
fn a_legacy_file_differing_from_the_store_keeps_both_keys() {
    let fixture = Fixture::new();
    let (stored, in_file) = (SecretKey::generate(), SecretKey::generate());
    fixture.store(&stored);
    fixture.write_legacy_file(&in_file);

    let keys = fixture.backup_keys();
    assert_eq!(
        keys.move_legacy_file().unwrap(),
        LegacyKeyFile::Kept {
            file_key_id: in_file.id(),
            store_key_id: stored.id(),
        }
    );
    assert_eq!(keys.load().unwrap(), Some(stored.clone()));
    assert_eq!(fixture.stored(), Some(stored));
    assert_eq!(
        read_legacy_key_file(&fixture.legacy_file()).unwrap(),
        Some(in_file),
        "a key older archives may need must never be deleted on a guess"
    );
}

/// Every read moves the file first, so a backup after an upgrade seals with
/// the key the user saved.
#[test]
fn a_read_moves_the_legacy_file() {
    let fixture = Fixture::new();
    let key = SecretKey::generate();
    fixture.write_legacy_file(&key);

    assert_eq!(fixture.backup_keys().load().unwrap(), Some(key));
    assert!(!fixture.legacy_file().exists());
}

#[test]
fn an_unreadable_legacy_file_fails_reads() {
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.legacy_file().parent().unwrap()).unwrap();
    std::fs::write(fixture.legacy_file(), "not a key").unwrap();

    let keys = fixture.backup_keys();
    let err = keys.load().unwrap_err();
    assert!(
        err.to_string().contains("holds no usable backup key"),
        "{err}"
    );
    assert!(!keys.exists());
    assert_eq!(fixture.stored(), None);
    assert!(fixture.legacy_file().exists());
}

/// A store that refuses to write. The legacy file must survive a failed move.
struct ReadOnlyStore;

impl KeyStore for ReadOnlyStore {
    fn describe(&self, name: &str) -> String {
        format!("a read-only store ({name})")
    }
    fn get(&self, _name: &str) -> Result<Option<SecretKey>, BoxError> {
        Ok(None)
    }
    fn put(&self, _name: &str, _key: &SecretKey) -> Result<(), BoxError> {
        Err("the store is read-only".into())
    }
    fn delete(&self, _name: &str) -> Result<(), BoxError> {
        Ok(())
    }
}

#[test]
fn a_failed_move_keeps_the_legacy_file() {
    let fixture = Fixture::new();
    fixture.write_legacy_file(&SecretKey::generate());

    let keys = BackupKeyStore::new(Box::new(ReadOnlyStore), DATABASE, fixture.workspace.path());
    assert!(keys.move_legacy_file().is_err());
    assert!(keys.load().is_err());
    assert!(fixture.legacy_file().exists());
}

#[test]
fn a_workspace_without_a_key_has_none() {
    let fixture = Fixture::new();
    let keys = fixture.backup_keys();

    assert_eq!(keys.load().unwrap(), None);
    assert!(!keys.exists());
    assert_eq!(fixture.stored(), None);
}

/// The switch forgets every copy, so no later read moves a file's key back.
#[test]
fn delete_forgets_the_store_key_and_the_legacy_file() {
    let fixture = Fixture::new();
    fixture.store(&SecretKey::generate());
    fixture.write_legacy_file(&SecretKey::generate());

    let keys = fixture.backup_keys();
    keys.delete().unwrap();
    assert_eq!(keys.load().unwrap(), None);
    assert!(!fixture.legacy_file().exists());
    keys.delete().unwrap();
}
