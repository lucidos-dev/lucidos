//! Engine compatibility facade; unit tests never resolve production keys.

pub use lucidos_secret_store::key_store::*;

#[cfg(test)]
use super::SecretKey;
#[cfg(test)]
type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[cfg(test)]
pub fn resolve() -> Result<Box<dyn KeyStore>, BoxError> {
    Ok(Box::new(TestKeyStore))
}

/// Answers every `get` with the shared unit-test key and stores nothing.
#[cfg(test)]
pub struct TestKeyStore;

#[cfg(test)]
impl KeyStore for TestKeyStore {
    fn describe(&self, name: &str) -> String {
        format!("the test key store ({name})")
    }

    fn get(&self, _name: &str) -> Result<Option<SecretKey>, BoxError> {
        Ok(Some(super::key()?.clone()))
    }

    fn put(&self, _name: &str, _key: &SecretKey) -> Result<(), BoxError> {
        Ok(())
    }

    fn delete(&self, _name: &str) -> Result<(), BoxError> {
        Ok(())
    }
}
