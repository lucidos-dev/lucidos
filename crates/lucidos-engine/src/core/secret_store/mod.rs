//! The secret store: every secret the workspace database holds is sealed with
//! one per-workspace key (ADR 0509).
//!
//! A sealed value is text, `lsec1:<key id>:<base64 nonce and ciphertext>`,
//! AES-256-GCM with the `table.column` name as associated data. A `CHECK`
//! constraint on each column refuses anything else, so a dump or a `SELECT`
//! through any role yields ciphertext.
//!
//! [`SECRET_COLUMNS`] is the one list of secret columns. The boot backfill,
//! the agent role's grants and the guard test all read it.

pub mod boot;
pub mod key_store;
pub mod one_home;

use std::marker::PhantomData;
use std::sync::OnceLock;

use aes_gcm::aead::{Aead, OsRng, Payload};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueRef};
use sqlx::Postgres;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

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
    pub fn seal(&self, column: &SecretColumnSpec, plain: &str) -> Result<String, BoxError> {
        if plain.is_empty() && column.empty_means_none {
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
    pub fn open(&self, column: &SecretColumnSpec, stored: &str) -> Result<String, BoxError> {
        if stored.is_empty() && column.empty_means_none {
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

/// How a secret column holds its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// One sealed `TEXT` value (nullable or not).
    Text,
    /// A `JSONB` object whose every value is a sealed string. Its names stay
    /// readable, so a list can show them.
    JsonValues,
}

/// What `lucidos-engine secrets discard` does to a value no key opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discard {
    /// The row means nothing without the secret: remove it.
    DeleteRow,
    /// The column reads `''` as "none": set it to that.
    SetEmpty,
    /// Drop the entry from the JSON object.
    DropEntry,
}

/// One secret column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretColumnSpec {
    pub table: &'static str,
    pub column: &'static str,
    pub shape: Shape,
    pub empty_means_none: bool,
    pub discard: Discard,
}

impl SecretColumnSpec {
    /// `table.column`, the associated data every seal binds.
    pub fn name(&self) -> String {
        format!("{}.{}", self.table, self.column)
    }

    /// The `CHECK` constraint's name, written by the migration.
    pub fn constraint(&self) -> String {
        format!("{}_{}_sealed", self.table, self.column)
    }
}

/// A secret `TEXT` column, as a type, so a row struct names its column once.
pub trait SecretColumn {
    const SPEC: SecretColumnSpec;
}

macro_rules! secret_columns {
    ($($marker:ident => $table:literal . $column:literal, $shape:ident, empty: $empty:literal, discard: $discard:ident;)*) => {
        $(
            pub struct $marker;
            impl SecretColumn for $marker {
                const SPEC: SecretColumnSpec = SecretColumnSpec {
                    table: $table,
                    column: $column,
                    shape: Shape::$shape,
                    empty_means_none: $empty,
                    discard: Discard::$discard,
                };
            }
        )*
        /// Every secret column in the workspace database.
        pub const SECRET_COLUMNS: &[SecretColumnSpec] = &[$($marker::SPEC),*];
    };
}

secret_columns! {
    CredentialAuthValue => "credentials"."auth_value", Text, empty: false, discard: DeleteRow;
    OAuthAccessToken => "oauth_accounts"."access_token", Text, empty: false, discard: DeleteRow;
    OAuthRefreshToken => "oauth_accounts"."refresh_token", Text, empty: false, discard: DeleteRow;
    PushAuth => "push_subscriptions"."auth", Text, empty: false, discard: DeleteRow;
    VapidPrivateKey => "vapid_keys"."private_key_pem", Text, empty: false, discard: DeleteRow;
    McpServerEnv => "mcp_servers"."env", JsonValues, empty: false, discard: DropEntry;
    ClaudeGatewayAccessToken => "claude_gateway_accounts"."access_token", Text, empty: false, discard: DeleteRow;
    ClaudeGatewayRefreshToken => "claude_gateway_accounts"."refresh_token", Text, empty: false, discard: DeleteRow;
}

/// The mailbox password column, which held a copy of the `email_password`
/// credential. [`one_home::move_at_boot`] moves what it holds into
/// `credentials` and drops it.
pub struct EmailPassword;
impl SecretColumn for EmailPassword {
    const SPEC: SecretColumnSpec = SecretColumnSpec {
        table: "email_accounts",
        column: "password",
        shape: Shape::Text,
        empty_means_none: true,
        discard: Discard::SetEmpty,
    };
}

/// Secret columns a boot step moves into `credentials` and then drops. While
/// one still exists, the boot check reads its key ids and `secrets discard`
/// clears it, exactly as for [`SECRET_COLUMNS`].
pub const MOVED_SECRET_COLUMNS: &[SecretColumnSpec] = &[EmailPassword::SPEC];

/// The key this engine seals with, installed once at boot.
static KEY: OnceLock<SecretKey> = OnceLock::new();

/// Install the workspace key. A second install of another key is refused: one
/// engine serves one database.
pub fn install(key: SecretKey) -> Result<(), BoxError> {
    let installed = KEY.get_or_init(|| key.clone());
    if *installed != key {
        return Err("a different secret store key is already installed".into());
    }
    Ok(())
}

/// The installed key. Unit tests share one fixed key, so a store round-trips
/// without a boot.
pub fn key() -> Result<&'static SecretKey, BoxError> {
    #[cfg(test)]
    {
        Ok(KEY.get_or_init(|| SecretKey([7; 32])))
    }
    #[cfg(not(test))]
    {
        KEY.get()
            .ok_or_else(|| "the secret store key is not loaded yet".into())
    }
}

/// Seal `plain` for `column` with the installed key.
pub fn seal(column: &SecretColumnSpec, plain: &str) -> Result<String, BoxError> {
    key()?.seal(column, plain)
}

/// Open a value of `column` with the installed key.
pub fn open(column: &SecretColumnSpec, stored: &str) -> Result<String, BoxError> {
    key()?.open(column, stored)
}

/// A secret of column `C`, plain in memory. It seals as it binds and opens as
/// it decodes, so a query never sees the plain text and a caller never sees
/// the ciphertext. `Debug` redacts it.
pub struct Secret<C> {
    plain: String,
    column: PhantomData<C>,
}

impl<C> Secret<C> {
    pub fn new(plain: impl Into<String>) -> Self {
        Self {
            plain: plain.into(),
            column: PhantomData,
        }
    }

    pub fn into_inner(self) -> String {
        self.plain
    }

    pub fn as_str(&self) -> &str {
        &self.plain
    }
}

/// Lets a `FromRow` derive open a column with `#[sqlx(try_from = "Secret<C>")]`.
impl<C> From<Secret<C>> for String {
    fn from(secret: Secret<C>) -> Self {
        secret.plain
    }
}

impl<C> Clone for Secret<C> {
    fn clone(&self) -> Self {
        Self::new(self.plain.clone())
    }
}

impl<C> std::fmt::Debug for Secret<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl<C> sqlx::Type<Postgres> for Secret<C> {
    fn type_info() -> PgTypeInfo {
        <&str as sqlx::Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <&str as sqlx::Type<Postgres>>::compatible(ty)
    }
}

impl<'r, C: SecretColumn> sqlx::Decode<'r, Postgres> for Secret<C> {
    fn decode(value: PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let stored = <&str as sqlx::Decode<Postgres>>::decode(value)?;
        Ok(Self::new(open(&C::SPEC, stored)?))
    }
}

impl<C: SecretColumn> sqlx::Encode<'_, Postgres> for Secret<C> {
    fn encode_by_ref(
        &self,
        buf: &mut PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        let sealed = seal(&C::SPEC, &self.plain)?;
        <&str as sqlx::Encode<Postgres>>::encode_by_ref(&sealed.as_str(), buf)
    }
}

/// Seal every value of a JSON object, for a [`Shape::JsonValues`] column.
pub fn seal_json_values(
    column: &SecretColumnSpec,
    plain: &std::collections::HashMap<String, String>,
) -> Result<serde_json::Value, BoxError> {
    let mut sealed = serde_json::Map::new();
    for (name, value) in plain {
        sealed.insert(name.clone(), seal(column, value)?.into());
    }
    Ok(serde_json::Value::Object(sealed))
}

/// Open every value of a JSON object [`seal_json_values`] wrote.
pub fn open_json_values(
    column: &SecretColumnSpec,
    stored: serde_json::Value,
) -> Result<std::collections::HashMap<String, String>, BoxError> {
    let serde_json::Value::Object(entries) = stored else {
        return Err(format!("{} is not a JSON object", column.name()).into());
    };
    entries
        .into_iter()
        .map(|(name, value)| match value {
            serde_json::Value::String(text) => Ok((name, open(column, &text)?)),
            _ => Err(format!("{} entry {name} is not a string", column.name()).into()),
        })
        .collect()
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
