use std::path::{Path, PathBuf};

use super::SecretKey;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub const KEY_STORE_ENV: &str = "LUCIDOS_SECRET_KEY_STORE";

pub const KEYCHAIN_SERVICE: &str = "Lucidos workspace secrets";

pub const KEY_DIR_NAME: &str = lucidos_home::private::SECRET_KEYS_DIR;

pub trait KeyStore: Send + Sync {
    fn describe(&self, name: &str) -> String;
    fn get(&self, name: &str) -> Result<Option<SecretKey>, BoxError>;
    fn put(&self, name: &str, key: &SecretKey) -> Result<(), BoxError>;
    fn delete(&self, name: &str) -> Result<(), BoxError>;

    fn put_new(&self, name: &str, key: &SecretKey) -> Result<bool, BoxError> {
        if self.get(name)?.is_some() {
            return Ok(false);
        }
        self.put(name, key)?;
        Ok(true)
    }
}

pub fn replace_keeping(
    store: &dyn KeyStore,
    name: &str,
    key: &SecretKey,
) -> Result<Option<String>, BoxError> {
    let parked = match store.get(name)? {
        Some(old) if &old == key => return Ok(None),
        Some(old) => {
            let parked = parked_key_name(name, &old.id());
            store.put(&parked, &old)?;
            Some(parked)
        }
        None => None,
    };
    store.put(name, key)?;
    Ok(parked)
}

pub fn parked_key_name(name: &str, key_id: &str) -> String {
    format!("{name}.{key_id}")
}

pub fn secrets_key_name(database: &str) -> String {
    database.to_string()
}

pub fn backup_key_name(database: &str) -> String {
    format!("{database}.backup")
}

pub fn snapshot_key_name(database: &str) -> String {
    format!("{database}.snapshot")
}

fn check_name(name: &str) -> Result<(), BoxError> {
    let plain = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if plain {
        Ok(())
    } else {
        Err(format!("{name:?} cannot name a key").into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStoreKind {
    Keychain,
    File,
}

pub fn kind_for(
    override_value: Option<&str>,
    in_app_bundle: bool,
    macos: bool,
) -> Result<KeyStoreKind, BoxError> {
    match override_value.map(str::trim).filter(|v| !v.is_empty()) {
        Some("keychain") if macos => Ok(KeyStoreKind::Keychain),
        Some("keychain") => Err(format!("{KEY_STORE_ENV}=keychain needs macOS").into()),
        Some("file") => Ok(KeyStoreKind::File),
        Some(other) => {
            Err(format!("{KEY_STORE_ENV}={other:?} is not one of keychain, file").into())
        }
        None if in_app_bundle && macos => Ok(KeyStoreKind::Keychain),
        None => Ok(KeyStoreKind::File),
    }
}

pub fn resolve() -> Result<Box<dyn KeyStore>, BoxError> {
    let override_value = std::env::var(KEY_STORE_ENV).ok();
    let in_app_bundle = std::env::current_exe().is_ok_and(|exe| is_in_app_bundle(&exe));
    store_of(kind_for(
        override_value.as_deref(),
        in_app_bundle,
        cfg!(target_os = "macos"),
    )?)
}

pub fn is_in_app_bundle(exe: &Path) -> bool {
    exe.components().any(|part| {
        part.as_os_str()
            .to_str()
            .is_some_and(|name| name.len() > 4 && name.ends_with(".app"))
    })
}

pub fn store_of(kind: KeyStoreKind) -> Result<Box<dyn KeyStore>, BoxError> {
    match kind {
        KeyStoreKind::Keychain => keychain(),
        KeyStoreKind::File => Ok(Box::new(FileKeyStore::new(default_key_dir()?))),
    }
}

pub fn default_key_dir() -> Result<PathBuf, BoxError> {
    if lucidos_home::path().is_none() {
        return Err("HOME is not set, so there is no key directory".into());
    }
    Ok(lucidos_home::ensure_private_entry(KEY_DIR_NAME)?)
}

#[cfg(target_os = "macos")]
fn keychain() -> Result<Box<dyn KeyStore>, BoxError> {
    Ok(Box::new(KeychainKeyStore))
}

#[cfg(not(target_os = "macos"))]
fn keychain() -> Result<Box<dyn KeyStore>, BoxError> {
    Err("the Keychain exists only on macOS".into())
}

pub struct FileKeyStore {
    dir: PathBuf,
}

impl FileKeyStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.key"))
    }

    fn ensure_dir(&self) -> Result<(), BoxError> {
        std::fs::create_dir_all(&self.dir)?;
        restrict(&self.dir, 0o700)?;
        if let Err(e) = lucidos_file_backup_exclusion::ensure_excluded(&self.dir) {
            eprintln!(
                "[SecretStore] Cannot exclude {} from file-level backup: {}",
                self.dir.display(),
                e
            );
        }
        Ok(())
    }
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> Result<(), BoxError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32) -> Result<(), BoxError> {
    Ok(())
}

impl KeyStore for FileKeyStore {
    fn describe(&self, name: &str) -> String {
        format!("the key file {}", self.path(name).display())
    }

    fn get(&self, name: &str) -> Result<Option<SecretKey>, BoxError> {
        check_name(name)?;
        match std::fs::read_to_string(self.path(name)) {
            Ok(text) => Ok(Some(SecretKey::from_base64(&text).map_err(|e| {
                format!("{} holds no usable key: {e}", self.describe(name))
            })?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("cannot read {}: {e}", self.describe(name)).into()),
        }
    }

    fn put(&self, name: &str, key: &SecretKey) -> Result<(), BoxError> {
        use std::io::Write as _;
        check_name(name)?;
        self.ensure_dir()?;
        let mut temp = tempfile::NamedTempFile::new_in(&self.dir)?;
        temp.write_all(key.to_base64().as_bytes())?;
        temp.as_file().sync_all()?;
        temp.persist(self.path(name)).map_err(|e| e.error)?;
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), BoxError> {
        check_name(name)?;
        match std::fs::remove_file(self.path(name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    fn put_new(&self, name: &str, key: &SecretKey) -> Result<bool, BoxError> {
        use std::io::Write as _;
        check_name(name)?;
        self.ensure_dir()?;
        let mut temp = tempfile::NamedTempFile::new_in(&self.dir)?;
        temp.write_all(key.to_base64().as_bytes())?;
        temp.as_file().sync_all()?;
        match temp.persist_noclobber(self.path(name)) {
            Ok(_) => Ok(true),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e.error.into()),
        }
    }
}

#[cfg(target_os = "macos")]
pub struct KeychainKeyStore;

#[cfg(target_os = "macos")]
impl KeychainKeyStore {
    const NOT_FOUND: i32 = -25300;
}

#[cfg(target_os = "macos")]
impl KeyStore for KeychainKeyStore {
    fn describe(&self, name: &str) -> String {
        format!("the macOS Keychain item \"{KEYCHAIN_SERVICE}\" / {name}")
    }

    fn get(&self, name: &str) -> Result<Option<SecretKey>, BoxError> {
        check_name(name)?;
        match security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, name) {
            Ok(bytes) => Ok(Some(SecretKey::from_base64(&String::from_utf8(bytes)?)?)),
            Err(e) if e.code() == Self::NOT_FOUND => Ok(None),
            Err(e) => Err(format!("cannot read {}: {e}", self.describe(name)).into()),
        }
    }

    fn put(&self, name: &str, key: &SecretKey) -> Result<(), BoxError> {
        check_name(name)?;
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            name,
            key.to_base64().as_bytes(),
        )
        .map_err(|e| format!("cannot write {}: {e}", self.describe(name)).into())
    }

    fn delete(&self, name: &str) -> Result<(), BoxError> {
        check_name(name)?;
        match security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, name) {
            Err(e) if e.code() != Self::NOT_FOUND => Err(e.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
#[path = "key_store_tests.rs"]
mod tests;
