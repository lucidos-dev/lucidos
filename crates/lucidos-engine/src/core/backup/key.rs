//! The legacy backup key: the symmetric key a workspace sealed its `.enc`
//! archives with before it switched to a backup recipient. It lives in the key
//! store, beside the secrets key (ADR 0509). A workspace from before that kept
//! it in `<ws>/.lucidos/backup.key`, and every read first moves that file into
//! the store.
//!
//! Nothing mints a legacy key any more. A workspace that holds one keeps
//! sealing with it until the switch, which deletes it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::core::secret_store::key_store::{self, backup_key_name, KeyStore};
use crate::core::secret_store::{boot, SecretKey};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// One engine serves one workspace, so one lock serializes every read, move
/// and delete. Without it, a move could put the file's key back into the
/// store just after the switch deleted it.
static KEY_LOCK: Mutex<()> = Mutex::new(());

/// What the move of a legacy key file found.
#[derive(Debug, PartialEq, Eq)]
pub enum LegacyKeyFile {
    /// No file: nothing to move.
    Absent,
    /// The store had no key. It holds the file's key now, and the file is gone.
    Moved,
    /// The store already held the same key, so the file was deleted.
    Duplicate,
    /// The store holds another key. Older archives may need the file's key,
    /// so both stay and the store's key seals new backups.
    Kept {
        file_key_id: String,
        store_key_id: String,
    },
}

/// A workspace's backup key, in its key store.
pub struct BackupKeyStore {
    store: Box<dyn KeyStore>,
    name: String,
    legacy_file: PathBuf,
}

impl BackupKeyStore {
    /// The backup key of this engine's database, in this engine's key store.
    pub fn for_engine(workspace: &Path) -> Result<Self, BoxError> {
        let database = boot::database_name(&crate::core::database_url())?;
        Ok(Self::new(key_store::resolve()?, &database, workspace))
    }

    pub fn new(store: Box<dyn KeyStore>, database: &str, workspace: &Path) -> Self {
        Self {
            store,
            name: backup_key_name(database),
            legacy_file: legacy_key_file(workspace),
        }
    }

    /// Where the key lives, in words a log line or an error can show.
    pub fn describe(&self) -> String {
        self.store.describe(&self.name)
    }

    /// The key, or `None` when the workspace holds none.
    pub fn load(&self) -> Result<Option<SecretKey>, BoxError> {
        let _guard = lock();
        self.move_legacy_file_locked()?;
        self.store.get(&self.name)
    }

    /// Whether a usable key exists. A key the store cannot read counts as
    /// none, so the page offers no reveal that would fail.
    pub fn exists(&self) -> bool {
        self.load().ok().flatten().is_some()
    }

    /// Forget the legacy key: the store's copy and any legacy file. The switch
    /// to a backup recipient calls this once the recipient is in place.
    pub fn delete(&self) -> Result<(), BoxError> {
        let _guard = lock();
        self.store.delete(&self.name)?;
        match std::fs::remove_file(&self.legacy_file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("cannot delete {}: {e}", self.legacy_file.display()).into())
            }
            _ => Ok(()),
        }
    }

    /// Move `<ws>/.lucidos/backup.key` into the store. The boot calls this to
    /// log the outcome; every read calls it too.
    pub fn move_legacy_file(&self) -> Result<LegacyKeyFile, BoxError> {
        let _guard = lock();
        self.move_legacy_file_locked()
    }

    fn move_legacy_file_locked(&self) -> Result<LegacyKeyFile, BoxError> {
        let Some(file_key) = read_legacy_key_file(&self.legacy_file)? else {
            return Ok(LegacyKeyFile::Absent);
        };
        let outcome = match self.store.get(&self.name)? {
            None if self.put_new_and_confirm(&file_key)? => LegacyKeyFile::Moved,
            None => return self.move_legacy_file_locked(),
            Some(stored) if stored == file_key => LegacyKeyFile::Duplicate,
            Some(stored) => {
                return Ok(LegacyKeyFile::Kept {
                    file_key_id: file_key.id(),
                    store_key_id: stored.id(),
                })
            }
        };
        match std::fs::remove_file(&self.legacy_file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!(
                "moved the backup key into {}, but cannot delete {}: {e}",
                self.describe(),
                self.legacy_file.display()
            )
            .into()),
            _ => Ok(outcome),
        }
    }

    /// `false` means another process stored a key first, left untouched.
    fn put_new_and_confirm(&self, key: &SecretKey) -> Result<bool, BoxError> {
        if !self.store.put_new(&self.name, key)? {
            return Ok(false);
        }
        match self.store.get(&self.name)? {
            Some(back) if &back == key => Ok(true),
            _ => Err(format!("{} did not read the backup key back", self.describe()).into()),
        }
    }
}

/// The boot's move of a legacy key file. Never fatal: every later read retries
/// the move, and a backup fails loudly while it cannot happen.
pub fn move_legacy_file_at_boot(workspace: &Path) {
    let keys = match BackupKeyStore::for_engine(workspace) {
        Ok(keys) => keys,
        Err(e) => return log!("[Backup] Cannot reach the backup key store: {}", e),
    };
    match keys.move_legacy_file() {
        Ok(LegacyKeyFile::Absent) => {}
        Ok(LegacyKeyFile::Moved | LegacyKeyFile::Duplicate) => log!(
            "[Backup] Moved .lucidos/backup.key into {}",
            keys.describe()
        ),
        Ok(LegacyKeyFile::Kept {
            file_key_id,
            store_key_id,
        }) => log!(
            "[Backup] .lucidos/backup.key holds key {} but {} holds key {}. \
             New backups use {}; the file stays for older archives",
            file_key_id,
            keys.describe(),
            store_key_id,
            store_key_id
        ),
        Err(e) => log!("[Backup] Cannot move .lucidos/backup.key yet: {}", e),
    }
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    KEY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Where a workspace from before the key store kept its backup key.
fn legacy_key_file(workspace: &Path) -> PathBuf {
    workspace.join(".lucidos").join("backup.key")
}

/// A file that holds no key is an error, so backups fail loudly rather than
/// skip a key the user may have saved.
fn read_legacy_key_file(path: &Path) -> Result<Option<SecretKey>, BoxError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(SecretKey::from_base64(&text).map_err(|e| {
            format!("{} holds no usable backup key: {e}", path.display())
        })?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display()).into()),
    }
}

#[cfg(test)]
#[path = "key_tests.rs"]
mod tests;
