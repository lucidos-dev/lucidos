//! Engine compatibility facade; unit tests never resolve production keys.

pub use lucidos_secret_store::key_store::*;

#[cfg(any(test, feature = "test-support"))]
use super::SecretKey;
#[cfg(any(test, feature = "test-support"))]
type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[cfg(any(test, feature = "test-support"))]
pub fn resolve() -> Result<Box<dyn KeyStore>, BoxError> {
    #[cfg(not(test))]
    if !USE_TEST_STORE.load(std::sync::atomic::Ordering::Relaxed) {
        return lucidos_secret_store::key_store::resolve();
    }
    Ok(Box::new(TestKeyStore))
}

#[cfg(all(not(test), feature = "test-support"))]
static USE_TEST_STORE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Install the same fixed key as the engine unit-test harness. Integration
/// binaries keep their normal store unless a fixture explicitly calls this.
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn install_test_key_store() {
    super::install(SecretKey::from_bytes(&[7; 32]).expect("test key"))
        .expect("install fixture key");
    #[cfg(not(test))]
    USE_TEST_STORE.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Answers every `get` with the shared unit-test key and stores nothing.
#[cfg(any(test, feature = "test-support"))]
pub struct TestKeyStore;

#[cfg(any(test, feature = "test-support"))]
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
