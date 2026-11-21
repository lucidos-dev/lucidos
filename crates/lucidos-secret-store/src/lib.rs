//! Shared sealing format and platform key storage for engines and gateways.

pub mod key_store;

use aes_gcm::aead::{Aead, OsRng, Payload};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use sha2::{Digest, Sha256};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The associated-data namespace and empty-value policy of a sealed field.
pub trait SealingContext {
    fn name(&self) -> String;
    fn empty_means_none(&self) -> bool;
}

/// The prefix every sealed value starts with. The version digit leaves room
/// for another format without a guess at read time.
pub const SEALED_PREFIX: &str = "lsec1:";

const NONCE_LEN: usize = 12;

/// A 32-byte AES-256 key. `Debug` prints the key id, never the bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretKey([u8; 32]);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretKey({})", self.id())
    }
}

impl SecretKey {
    pub fn generate() -> Self {
        Self(Aes256Gcm::generate_key(OsRng).into())
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BoxError> {
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| format!("a key is 32 bytes, not {}", bytes.len()))?;
        Ok(Self(bytes))
    }

    pub fn from_base64(text: &str) -> Result<Self, BoxError> {
        Self::from_bytes(&BASE64.decode(text.trim())?)
    }

    pub fn to_base64(&self) -> String {
        BASE64.encode(self.0)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The first 8 bytes of the key's SHA-256, in hex. Safe to show and log.
    pub fn id(&self) -> String {
        hex_encode(&Sha256::digest(self.0)[..8])
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.0))
    }

    /// Seal `plain` for `column`. The empty string stays empty where the
    /// column reads it as "none".
    pub fn seal(&self, column: &impl SealingContext, plain: &str) -> Result<String, BoxError> {
        if plain.is_empty() && column.empty_means_none() {
            return Ok(String::new());
        }
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let sealed = self
            .cipher()
            .encrypt(
                &nonce,
                Payload {
                    msg: plain.as_bytes(),
                    aad: column.name().as_bytes(),
                },
            )
            .map_err(|_| format!("sealing {} failed", column.name()))?;
        let mut body = nonce.to_vec();
        body.extend_from_slice(&sealed);
        Ok(format!(
            "{SEALED_PREFIX}{}:{}",
            self.id(),
            BASE64.encode(body)
        ))
    }

    /// Open a value [`Self::seal`] wrote for `column`.
    pub fn open(&self, column: &impl SealingContext, stored: &str) -> Result<String, BoxError> {
        if stored.is_empty() && column.empty_means_none() {
            return Ok(String::new());
        }
        let Some(key_id) = sealed_key_id(stored) else {
            return Err(format!("{} holds a value that is not sealed", column.name()).into());
        };
        if key_id != self.id() {
            return Err(format!(
                "{} was sealed with key {key_id}, not this workspace's key {}",
                column.name(),
                self.id()
            )
            .into());
        }
        let encoded = &stored[SEALED_PREFIX.len() + key_id.len() + 1..];
        let body = BASE64.decode(encoded)?;
        if body.len() <= NONCE_LEN {
            return Err(format!("{} holds a truncated sealed value", column.name()).into());
        }
        let (nonce, sealed) = body.split_at(NONCE_LEN);
        let plain = self
            .cipher()
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: sealed,
                    aad: column.name().as_bytes(),
                },
            )
            .map_err(|_| format!("{} did not open: it was altered or moved", column.name()))?;
        Ok(String::from_utf8(plain)?)
    }
}

/// The key id inside a sealed value, or `None` when it is not one.
pub fn sealed_key_id(stored: &str) -> Option<&str> {
    let rest = stored.strip_prefix(SEALED_PREFIX)?;
    let (key_id, _) = rest.split_once(':')?;
    Some(key_id)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
