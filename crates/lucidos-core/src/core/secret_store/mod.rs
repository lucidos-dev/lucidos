pub mod boot;
pub mod key_store;
pub mod one_home;

use std::marker::PhantomData;
use std::sync::OnceLock;

use sqlx::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueRef};
use sqlx::Postgres;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub use lucidos_secret_store::{sealed_key_id, SecretKey, SEALED_PREFIX};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Text,
    JsonValues,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discard {
    DeleteRow,
    SetEmpty,
    DropEntry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretColumnSpec {
    pub table: &'static str,
    pub column: &'static str,
    pub shape: Shape,
    pub empty_means_none: bool,
    pub discard: Discard,
}

impl SecretColumnSpec {
    /// INVARIANT: `table.column`, the associated data every seal binds.
    pub fn name(&self) -> String {
        format!("{}.{}", self.table, self.column)
    }

    pub fn constraint(&self) -> String {
        format!("{}_{}_sealed", self.table, self.column)
    }
}

impl lucidos_secret_store::SealingContext for SecretColumnSpec {
    fn name(&self) -> String {
        self.name()
    }
    fn empty_means_none(&self) -> bool {
        self.empty_means_none
    }
}

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
    ChatGptPlanIdToken => "chatgpt_plan_accounts"."id_token", Text, empty: false, discard: DeleteRow;
    ChatGptPlanAccessToken => "chatgpt_plan_accounts"."access_token", Text, empty: false, discard: DeleteRow;
    ChatGptPlanRefreshToken => "chatgpt_plan_accounts"."refresh_token", Text, empty: false, discard: DeleteRow;
}

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

pub const MOVED_SECRET_COLUMNS: &[SecretColumnSpec] = &[EmailPassword::SPEC];

static KEY: OnceLock<SecretKey> = OnceLock::new();

/// INVARIANT: Install the workspace key. A second install of another key is refused: one
/// engine serves one database.
pub fn install(key: SecretKey) -> Result<(), BoxError> {
    let installed = KEY.get_or_init(|| key.clone());
    if *installed != key {
        return Err("a different secret store key is already installed".into());
    }
    Ok(())
}

pub fn key() -> Result<&'static SecretKey, BoxError> {
    #[cfg(test)]
    {
        Ok(KEY.get_or_init(|| SecretKey::from_bytes(&[7; 32]).expect("test key")))
    }
    #[cfg(not(test))]
    {
        KEY.get()
            .ok_or_else(|| "the secret store key is not loaded yet".into())
    }
}

pub fn seal(column: &SecretColumnSpec, plain: &str) -> Result<String, BoxError> {
    key()?.seal(column, plain)
}

pub fn open(column: &SecretColumnSpec, stored: &str) -> Result<String, BoxError> {
    key()?.open(column, stored)
}

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
