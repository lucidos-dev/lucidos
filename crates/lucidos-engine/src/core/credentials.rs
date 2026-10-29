use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use uuid::Uuid;

use crate::core::secret_reference::{engine_resolver, SecretField, SecretResolver};
use crate::core::secret_store::{CredentialAuthValue, Secret};
use crate::core::url_scope::check_path_within;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

/// Authentication type for a stored credential.
///
/// `Unknown` catches DB rows written by a newer engine version with an
/// auth_type variant this binary doesn't recognize — falls through the
/// header-injection sites as a raw value instead of pretending to be ApiKey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    ApiKey,
    Bearer,
    Basic,
    Password,
    OauthClient,
    EmailPassword,
    /// A shared secret with no transport role. Every other variant says how a
    /// value is SENT; this one is never sent. A webhook signing secret is the
    /// first, and it is only ever fed to an HMAC.
    Secret,
    /// The token `op` reads 1Password with, for *secret references*. Only the
    /// `op` child ever sees it (`core::secret_reference::op`).
    OnepasswordServiceAccount,
    Unknown,
}

impl AuthType {
    /// Parse from a database string, returning `Unknown` for unrecognized
    /// values so callers can detect-and-skip rather than getting silently
    /// coerced to `ApiKey`. The auth-header injection sites already gate on
    /// the specific variants (`Bearer`, `Basic`, `Password`) and treat
    /// everything else as raw value, so `Unknown` lands in the safe branch.
    pub fn parse(s: &str) -> Self {
        match s {
            "api_key" => Self::ApiKey,
            "bearer" => Self::Bearer,
            "basic" => Self::Basic,
            "password" => Self::Password,
            "oauth_client" => Self::OauthClient,
            "email_password" => Self::EmailPassword,
            "secret" => Self::Secret,
            "onepassword_service_account" => Self::OnepasswordServiceAccount,
            _ => Self::Unknown,
        }
    }

    /// Every variant. The requestable ones, in this order, are the
    /// `request_credential` enum, so append rather than insert: moving a value
    /// rewrites a cached prompt segment. Rust cannot enumerate a plain enum, so
    /// `all_lists_every_variant` reads the declaration back to catch an
    /// omission.
    pub const ALL: &'static [Self] = &[
        Self::ApiKey,
        Self::Bearer,
        Self::Basic,
        Self::Password,
        Self::OauthClient,
        Self::EmailPassword,
        Self::Secret,
        Self::OnepasswordServiceAccount,
        Self::Unknown,
    ];

    /// Whether `request_credential` may ask the user for this type.
    ///
    /// Exhaustive on purpose: a new variant does not compile until it is
    /// classified here, so the tool schema cannot fall behind the enum.
    pub fn agent_requestable(self) -> bool {
        match self {
            Self::ApiKey
            | Self::Bearer
            | Self::Basic
            | Self::Password
            | Self::OauthClient
            | Self::Secret => true,
            // `configure_email` owns email_password, building a JSON value the
            // credential modal has no fields for. `Unknown` is what a row from
            // a newer engine parses to, never something to ask a user for. The
            // agent never asks for the token that reads the user's vaults.
            Self::EmailPassword | Self::OnepasswordServiceAccount | Self::Unknown => false,
        }
    }

    /// Whether a value of this type may reach a subprocess, a request header
    /// or the proxy. An `oauth_client` blob is read only by the OAuth flow,
    /// and a service account token only by the `op` child.
    pub fn is_injectable(self) -> bool {
        !matches!(self, Self::OauthClient | Self::OnepasswordServiceAccount)
    }

    /// The JSON key of the secret field, for a type whose value is a JSON
    /// object. `None` for a type whose whole value is the secret.
    fn secret_json_key(self) -> Option<&'static str> {
        match self {
            Self::Password => Some("password"),
            Self::OauthClient => Some("client_secret"),
            _ => None,
        }
    }

    /// The `auth_type` values `request_credential` accepts, for its schema.
    pub fn agent_requestable_values() -> Vec<String> {
        Self::ALL
            .iter()
            .filter(|t| t.agent_requestable())
            .map(Self::to_string)
            .collect()
    }
}

impl fmt::Display for AuthType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::ApiKey => "api_key",
            Self::Bearer => "bearer",
            Self::Basic => "basic",
            Self::Password => "password",
            Self::OauthClient => "oauth_client",
            Self::EmailPassword => "email_password",
            Self::Secret => "secret",
            Self::OnepasswordServiceAccount => "onepassword_service_account",
            Self::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

/// Credential information for API access (without the secret)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialInfo {
    pub id: Uuid,
    pub service_name: String,
    /// Every base URL this credential may be presented to, its *credential
    /// scope*. A set rather than one value, because one key often covers
    /// several hostnames of one provider. Empty means it goes nowhere.
    pub base_urls: Vec<String>,
    pub auth_type: AuthType,
    pub auth_header: String,
    /// Optional custom env var name for the injected secret (e.g. `GITHUB_TOKEN`
    /// instead of the default `CRED_<NAME>`). `None` = default `CRED_` form.
    pub env_var_name: Option<String>,
    /// Every variable name a subprocess can name to get this credential. See
    /// [`credential_env_var_names`].
    pub env_var_names: Vec<String>,
    /// Whether a secret field is a *secret reference*, read from 1Password.
    pub secret_reference: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Full credential including the stored secret.
///
/// **The stored value is private.** It may be a *secret reference*, so a
/// consumer reads the secret through [`Self::secret`], which resolves it.
/// Only reveal and edit read [`Self::stored_value`].
#[derive(Clone)]
pub struct Credential {
    pub id: Uuid,
    pub service_name: String,
    /// See [`CredentialInfo::base_urls`]. Matched by
    /// [`credential_scope_covers`], which asks whether ANY member covers a URL.
    pub base_urls: Vec<String>,
    pub auth_type: AuthType,
    stored_value: String,
    pub auth_header: String,
    /// Optional custom env var name for the injected secret (e.g. `GITHUB_TOKEN`
    /// instead of the default `CRED_<NAME>`). `None` = default `CRED_` form.
    pub env_var_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// Manual `Debug` so the stored secret (API key, bearer token, password, or the
// oauth client_secret JSON blob) never leaks through `{:?}`. Everything else
// stays visible so the struct is still useful for debugging.
impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("id", &self.id)
            .field("service_name", &self.service_name)
            .field("base_urls", &self.base_urls)
            .field("auth_type", &self.auth_type)
            .field("stored_value", &"<redacted>")
            .field("auth_header", &self.auth_header)
            .field("env_var_name", &self.env_var_name)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

impl Credential {
    /// The secret as stored: a reference stays a reference. Reveal and the
    /// edit form read this; every other consumer reads [`Self::secret`].
    pub fn stored_value(&self) -> &str {
        &self.stored_value
    }

    /// A credential built in memory, for a test to adjust field by field.
    #[cfg(test)]
    pub(crate) fn fixture(service_name: &str, auth_type: AuthType, stored_value: &str) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            service_name: service_name.to_string(),
            base_urls: Vec::new(),
            auth_type,
            stored_value: stored_value.to_string(),
            auth_header: "Authorization".to_string(),
            env_var_name: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Whether a secret field holds a *secret reference*.
    pub fn holds_reference(&self) -> bool {
        secret_field_raw(self.auth_type, &self.stored_value)
            .is_some_and(|raw| matches!(SecretField::parse(&raw), Ok(SecretField::Reference(_))))
    }

    /// The secret, in the stored shape, with a reference resolved through the
    /// engine's resolver. The one read every consumer makes.
    ///
    /// The error names the credential and the reason, never a value.
    pub async fn secret(&self, pool: &PgPool) -> Result<String, String> {
        self.secret_with(&engine_resolver(pool)).await
    }

    /// [`Self::secret`] for a consumer that treats an unreadable secret as no
    /// secret, such as an LLM provider. The reason goes to the log under
    /// `module`.
    pub async fn secret_or_log(&self, pool: &PgPool, module: &str) -> Option<String> {
        match self.secret(pool).await {
            Ok(secret) => Some(secret),
            Err(reason) => {
                log!("{module} {reason}");
                None
            }
        }
    }

    /// [`Self::secret`] through a given resolver. Resolves only when a field
    /// holds a reference, and reports the outcome to the resolver.
    pub async fn secret_with<R: SecretResolver + ?Sized>(
        &self,
        resolver: &R,
    ) -> Result<String, String> {
        if self.auth_type == AuthType::OnepasswordServiceAccount {
            return Err(format!(
                "credential '{}' is a 1Password service account token, which only the op tool reads",
                self.service_name
            ));
        }
        let result = resolve_secret_fields(self.auth_type, &self.stored_value, resolver)
            .await
            .map_err(|reason| format!("credential '{}' {reason}", self.service_name));
        if self.holds_reference() {
            resolver
                .record_outcome(
                    &self.service_name,
                    result.as_ref().err().map(String::as_str),
                )
                .await;
        }
        result
    }
}

/// The raw secret field of a stored value: the whole value, or the string at
/// the type's JSON key. `None` for a JSON value with no such string.
fn secret_field_raw(auth_type: AuthType, stored: &str) -> Option<String> {
    match auth_type.secret_json_key() {
        None => Some(stored.to_string()),
        Some(key) => serde_json::from_str::<serde_json::Value>(stored)
            .ok()?
            .get(key)?
            .as_str()
            .map(str::to_string),
    }
}

/// `stored` with its secret field resolved, when that field is a reference.
/// A value whose JSON does not parse passes through, for its consumer to
/// refuse as it always did.
async fn resolve_secret_fields<R: SecretResolver + ?Sized>(
    auth_type: AuthType,
    stored: &str,
    resolver: &R,
) -> Result<String, String> {
    let Some(raw) = secret_field_raw(auth_type, stored) else {
        return Ok(stored.to_string());
    };
    let reference = match SecretField::parse(&raw) {
        Ok(SecretField::Stored(_)) => return Ok(stored.to_string()),
        Ok(SecretField::Reference(reference)) => reference,
        Err(reason) => return Err(format!("holds a malformed secret reference: {reason}")),
    };
    let value = resolver
        .resolve(&reference)
        .await
        .map_err(|failure| format!("could not be read from 1Password: {failure}"))?;
    let Some(key) = auth_type.secret_json_key() else {
        return Ok(value);
    };
    let mut parsed: serde_json::Value =
        serde_json::from_str(stored).map_err(|e| format!("holds invalid JSON: {e}"))?;
    parsed[key] = serde_json::Value::String(value);
    Ok(parsed.to_string())
}

/// Refuse a value about to be saved whose reference is malformed or does not
/// resolve now, so a typo never lands. A service account token can never be
/// a reference: it is what reads them.
///
/// Asks 1Password directly, past the cache, so the answer is current.
pub async fn check_secret_references(
    pool: &PgPool,
    auth_type: AuthType,
    value: &str,
) -> Result<(), String> {
    if auth_type == AuthType::OnepasswordServiceAccount {
        let account = crate::core::secret_reference::op::ServiceAccount::parse(value);
        if account.token.starts_with("op://") || account.token.is_empty() {
            return Err("a 1Password service account needs its token itself".to_string());
        }
        return Ok(());
    }
    let resolver = crate::core::secret_reference::op::OpResolver::new(pool.clone());
    resolve_secret_fields(auth_type, value, &resolver)
        .await
        .map(|_| ())
        .map_err(|reason| format!("This secret {reason}"))
}

/// Full credential DB row tuple (with secret). `env_var_name` is the trailing
/// nullable column added by `20260618124350_add_env_var_name_to_credentials`.
type CredentialRow = (
    Uuid,
    String,
    Vec<String>,
    String,
    Secret<CredentialAuthValue>,
    String,
    DateTime<Utc>,
    DateTime<Utc>,
    Option<String>,
);

/// Columns selected for a full credential (with secret), in `CredentialRow` order.
const CREDENTIAL_COLUMNS: &str =
    "id, service_name, base_urls, auth_type, auth_value, auth_header, created_at, updated_at, env_var_name";

/// The SQL form of [`AuthType::is_injectable`], for a lookup by name or URL.
const INJECTABLE_SQL: &str = "auth_type NOT IN ('oauth_client', 'onepassword_service_account')";

/// Parse a credential row tuple from the database (with secret).
fn parse_credential(row: CredentialRow) -> Credential {
    let (
        id,
        service_name,
        base_urls,
        auth_type,
        auth_value,
        auth_header,
        created_at,
        updated_at,
        env_var_name,
    ) = row;
    Credential {
        id,
        service_name,
        base_urls,
        auth_type: AuthType::parse(&auth_type),
        stored_value: auth_value.into_inner(),
        auth_header,
        env_var_name,
        created_at,
        updated_at,
    }
}

/// What a list shows of a credential: everything but the secret, plus
/// whether the secret is a reference.
fn parse_credential_info(row: CredentialRow) -> CredentialInfo {
    let credential = parse_credential(row);
    let secret_reference = credential.holds_reference();
    let Credential {
        id,
        service_name,
        base_urls,
        auth_type,
        auth_header,
        env_var_name,
        created_at,
        updated_at,
        ..
    } = credential;
    CredentialInfo {
        env_var_names: credential_env_var_names(&service_name, auth_type, env_var_name.as_deref()),
        secret_reference,
        id,
        service_name,
        base_urls,
        auth_type,
        auth_header,
        env_var_name,
        created_at,
        updated_at,
    }
}

/// Store for managing API credentials in the database.
///
/// **No caller can skip the event.** [`Self::upsert`], [`Self::update`] and
/// [`Self::delete`] are the only reachable mutators: the raw row writes
/// ([`Self::upsert_row`], [`Self::update_row`], [`Self::delete_row`]) are
/// private to this module, so nothing anywhere in the crate can change a
/// credential without the paired `Credential{Created,Updated,Deleted}` emit
/// being attempted. Those events are what reload the Settings credentials list
/// over SSE and, for a provider credential, what makes the engine hot-swap its
/// active LLM provider without a restart.
///
/// Same shape and the same reachability-not-atomicity guarantee as
/// `RepositoryStore`; see its type doc, and `core::announced_surfaces` for why
/// every announced surface is built this way.
pub struct CredentialStore;

impl CredentialStore {
    /// Insert or update a credential (upsert by service_name), reporting
    /// whether the row was created.
    ///
    /// **Private on purpose.** This is the raw row write with no event; going
    /// through [`Self::upsert`] is what guarantees the paired
    /// `Credential{Created,Updated}`. See the type-level doc.
    ///
    /// `xmax = 0` is Postgres' standard way to tell an `ON CONFLICT` insert
    /// from an update in a single round trip: a freshly inserted tuple has no
    /// deleting transaction, an updated one carries the id of the transaction
    /// that superseded the old version.
    ///
    /// **The conflict target follows the auth type, because the table carries
    /// two unique constraints and they arbitrate different things** (see
    /// `20260805134838_drop_credential_name_prefixes_use_auth_type.sql`):
    ///
    /// * `oauth_client` arbitrates on `(service_name, auth_type)`, the only
    ///   constraint it participates in. Re-registering a provider overwrites its
    ///   own row and leaves a same-named API key alone.
    /// * Everything else arbitrates on the partial index, which is keyed on
    ///   `service_name` alone. That preserves the pre-split behavior exactly:
    ///   saving `github` as a bearer when it exists as an api_key REPLACES it,
    ///   type included, rather than raising a bare 23505 the user cannot act on.
    ///
    /// A single `ON CONFLICT (service_name, auth_type)` for both would turn that
    /// second case into an unhandled unique violation, because the partial index
    /// would be the constraint actually violated and it is not the arbiter.
    async fn upsert_row(
        pool: &PgPool,
        service_name: &str,
        base_urls: &[String],
        auth_type: AuthType,
        auth_value: &str,
        auth_header: Option<&str>,
        env_var_name: Option<&str>,
    ) -> Result<(Uuid, bool), sqlx::Error> {
        let auth_header = auth_header.unwrap_or("Authorization");
        // The floor: no caller can store a blank member. See `scope_members`.
        let base_urls = scope_members(base_urls);

        // Postgres infers a partial unique index from a matching index
        // predicate, so the non-oauth arm names the predicate verbatim.
        let conflict_target = if auth_type == AuthType::OauthClient {
            "(service_name, auth_type)"
        } else {
            "(service_name) WHERE auth_type <> 'oauth_client'"
        };

        let result = sqlx::query_as::<_, (Uuid, bool)>(&format!(
            r#"
            INSERT INTO credentials (service_name, base_urls, auth_type, auth_value, auth_header, env_var_name)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT {conflict_target} DO UPDATE SET
                base_urls = EXCLUDED.base_urls,
                auth_type = EXCLUDED.auth_type,
                auth_value = EXCLUDED.auth_value,
                auth_header = EXCLUDED.auth_header,
                env_var_name = EXCLUDED.env_var_name,
                scope_declared = true,
                updated_at = NOW()
            RETURNING id, (xmax = 0) AS created
            "#
        ))
        .bind(service_name)
        .bind(&base_urls)
        .bind(auth_type.to_string())
        .bind(Secret::<CredentialAuthValue>::new(auth_value))
        .bind(auth_header)
        .bind(env_var_name)
        .fetch_one(pool)
        .await?;

        Ok(result)
    }

    /// Save a credential and announce it. The only way to create one.
    ///
    /// Emits `CredentialCreated` when the service was new and
    /// `CredentialUpdated` when an existing one was overwritten, so the
    /// timeline distinguishes "connected a provider" from "rotated its key".
    /// The emit is not the caller's choice: see the type-level doc.
    #[allow(clippy::too_many_arguments)] // one arg per credential column, plus the bus and actor
    pub async fn upsert(
        pool: &PgPool,
        event_bus: &EventBus,
        service_name: &str,
        base_urls: &[String],
        auth_type: AuthType,
        auth_value: &str,
        auth_header: Option<&str>,
        env_var_name: Option<&str>,
        actor: Option<MessageOrigin>,
    ) -> Result<Uuid, sqlx::Error> {
        let (id, created) = Self::upsert_row(
            pool,
            service_name,
            base_urls,
            auth_type,
            auth_value,
            auth_header,
            env_var_name,
        )
        .await?;
        crate::core::secret_reference::forget_resolved_values();
        let event = if created {
            SystemEvent::CredentialCreated {
                service_name: service_name.to_string(),
                auth_type,
                actor,
            }
        } else {
            SystemEvent::CredentialUpdated {
                service_name: service_name.to_string(),
                actor,
            }
        };
        event_bus
            .emit_or_log(BusEvent::System(event), "[Credentials] upsert")
            .await;
        Ok(id)
    }

    /// Get a credential by service name (includes the secret).
    ///
    /// **Deliberately blind to `oauth_client` rows.** An OAuth client
    /// registration is the one auth type allowed to shadow a name (see the
    /// partial unique index in
    /// `20260805134838_drop_credential_name_prefixes_use_auth_type.sql`), so
    /// without the exclusion this could return a `{client_id, ...}` JSON blob to
    /// a caller that asked for an API key. Every bare-name caller wants the
    /// non-oauth row: the four provider keys in `llm/provider_build.rs`, the
    /// `apis.json` resolvers in `api/proxy.rs` and `api/proxy_builtin.rs`. The
    /// OAuth flow uses [`Self::get_oauth_client`] instead.
    ///
    /// Blind to a 1Password service account too, so no name-keyed consumer
    /// (the proxy, a webhook, an MCP pointer) can ever send its token.
    ///
    /// Still returns at most one row: the same index keeps a name globally
    /// unique across every other type.
    pub async fn get(pool: &PgPool, service_name: &str) -> Result<Option<Credential>, sqlx::Error> {
        let result = sqlx::query_as::<_, CredentialRow>(&format!(
            "SELECT {CREDENTIAL_COLUMNS} FROM credentials \
             WHERE service_name = $1 AND {INJECTABLE_SQL}"
        ))
        .bind(service_name)
        .fetch_optional(pool)
        .await?;

        Ok(result.map(parse_credential))
    }

    /// Get a credential by its primary key (includes the secret).
    ///
    /// The unambiguous lookup for a caller that is holding a row it already
    /// listed: the Settings copy-value / edit / delete verbs, where a name is
    /// no longer a unique handle.
    pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Credential>, sqlx::Error> {
        let result = sqlx::query_as::<_, CredentialRow>(&format!(
            "SELECT {CREDENTIAL_COLUMNS} FROM credentials WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(pool)
        .await?;

        Ok(result.map(parse_credential))
    }

    /// Get a credential by service name AND auth type. The typed lookup the
    /// name prefixes used to fake: `auth_type` is the discriminator, so nothing
    /// has to encode it into the name and keep the two in sync.
    pub async fn get_typed(
        pool: &PgPool,
        service_name: &str,
        auth_type: AuthType,
    ) -> Result<Option<Credential>, sqlx::Error> {
        let result = sqlx::query_as::<_, CredentialRow>(&format!(
            "SELECT {CREDENTIAL_COLUMNS} FROM credentials \
             WHERE service_name = $1 AND auth_type = $2"
        ))
        .bind(service_name)
        .bind(auth_type.to_string())
        .fetch_optional(pool)
        .await?;

        Ok(result.map(parse_credential))
    }

    /// Get the OAuth client registration for a provider. Replaces resolving the
    /// string `oauth:<provider>`.
    ///
    /// Lowercases the provider, which the deleted `client_service_name` also
    /// did, so `Dropbox` and `dropbox` cannot address two registrations.
    pub async fn get_oauth_client(
        pool: &PgPool,
        provider: &str,
    ) -> Result<Option<Credential>, sqlx::Error> {
        Self::get_typed(pool, &provider.trim().to_lowercase(), AuthType::OauthClient).await
    }

    /// Get the mailbox password for an email account. Replaces resolving the
    /// string `email:<account>`.
    ///
    /// **Temporary measure**, registered in `docs/temporary-measures.md` under
    /// "`email:`-prefixed credential fallback in `get_email_password`": this also
    /// matches a still-prefixed
    /// `email:<account>` row. The prefix-stripping migration skips any row whose
    /// unprefixed name is already taken by another non-oauth credential, so a
    /// workspace with both an `email:work` mailbox password and a separate
    /// `work` API key keeps the prefixed name until the user resolves it. Remove
    /// the `OR` once no such row remains.
    ///
    /// Case-sensitive on the account name, matching `email_accounts.name`.
    pub async fn get_email_password(
        pool: &PgPool,
        account: &str,
    ) -> Result<Option<Credential>, sqlx::Error> {
        let result = sqlx::query_as::<_, CredentialRow>(&format!(
            "SELECT {CREDENTIAL_COLUMNS} FROM credentials \
             WHERE auth_type = 'email_password' \
               AND (service_name = $1 OR service_name = 'email:' || $1) \
             ORDER BY (service_name = $1) DESC \
             LIMIT 1"
        ))
        .bind(account)
        .fetch_optional(pool)
        .await?;

        Ok(result.map(parse_credential))
    }

    /// The workspace's 1Password service account, the newest if several.
    /// Read by the `op` resolver alone.
    pub(crate) async fn onepassword_service_account(
        pool: &PgPool,
    ) -> Result<Option<crate::core::secret_reference::op::ServiceAccount>, sqlx::Error> {
        let stored: Option<Secret<CredentialAuthValue>> = sqlx::query_scalar(
            "SELECT auth_value FROM credentials \
             WHERE auth_type = 'onepassword_service_account' \
             ORDER BY updated_at DESC LIMIT 1",
        )
        .fetch_optional(pool)
        .await?;
        Ok(stored.map(|stored| {
            crate::core::secret_reference::op::ServiceAccount::parse(&stored.into_inner())
        }))
    }

    /// Insert a secret a boot step moved here from another table. Returns
    /// whether it landed: a name already held by another credential leaves
    /// the table as it was.
    ///
    /// Silent, like a migration: it runs before the EventBus exists, and no
    /// client is connected to tell (`secret_store::one_home`).
    pub(crate) async fn insert_moved_secret(
        conn: &mut sqlx::PgConnection,
        service_name: &str,
        auth_type: AuthType,
        base_urls: &[String],
        value: &str,
    ) -> Result<bool, sqlx::Error> {
        let inserted = sqlx::query(
            "INSERT INTO credentials (service_name, base_urls, auth_type, auth_value) \
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(service_name)
        .bind(scope_members(base_urls))
        .bind(auth_type.to_string())
        .bind(Secret::<CredentialAuthValue>::new(value))
        .execute(conn)
        .await?;
        Ok(inserted.rows_affected() > 0)
    }

    /// List all credentials, without their secrets.
    pub async fn list(pool: &PgPool) -> Result<Vec<CredentialInfo>, sqlx::Error> {
        let results = sqlx::query_as::<_, CredentialRow>(&format!(
            "SELECT {CREDENTIAL_COLUMNS} FROM credentials ORDER BY service_name ASC"
        ))
        .fetch_all(pool)
        .await?;

        Ok(results.into_iter().map(parse_credential_info).collect())
    }

    /// List all credentials including secrets (for env injection into scripts)
    pub async fn list_all_with_secrets(pool: &PgPool) -> Result<Vec<Credential>, sqlx::Error> {
        let results = sqlx::query_as::<_, CredentialRow>(&format!(
            "SELECT {CREDENTIAL_COLUMNS} FROM credentials ORDER BY service_name ASC"
        ))
        .fetch_all(pool)
        .await?;

        Ok(results.into_iter().map(parse_credential).collect())
    }

    /// Delete a credential row, returning the deleted row's `service_name` for
    /// the announcement. **Private on purpose**, same as [`Self::upsert_row`]:
    /// [`Self::delete`] is the reachable mutator, and it emits.
    ///
    /// Keyed on `id` rather than `service_name`: since
    /// `20260805134838_drop_credential_name_prefixes_use_auth_type.sql` a name
    /// can be held by two rows (an `oauth_client` registration shadowing an API
    /// key), so a name-keyed DELETE would be a coin flip over which one goes.
    /// `RETURNING` is what keeps the event able to name the deleted service
    /// without a second round trip.
    async fn delete_row(pool: &PgPool, id: Uuid) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar("DELETE FROM credentials WHERE id = $1 RETURNING service_name")
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    /// Delete a credential and announce it. The only way to remove one.
    ///
    /// `CredentialDeleted` fires only when a row was actually deleted, so a
    /// repeated or racing delete announces once.
    pub async fn delete(
        pool: &PgPool,
        event_bus: &EventBus,
        id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let removed = Self::delete_row(pool, id).await?;
        crate::core::secret_reference::forget_resolved_values();
        if let Some(service_name) = &removed {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::CredentialDeleted {
                        service_name: service_name.clone(),
                        actor,
                    }),
                    "[Credentials] CredentialDeleted",
                )
                .await;
        }
        Ok(removed.is_some())
    }

    /// Update an existing credential's editable fields (everything except the
    /// immutable `service_name`). `auth_value: None` keeps the stored secret
    /// untouched — used when the user edits non-secret fields without
    /// re-entering the secret. Returns the row's `service_name` when one
    /// existed, for the announcement.
    ///
    /// `base_urls: None` keeps the stored scope, on the same terms. An EMPTY
    /// slice is a different answer and clears it, which is what a `secret`
    /// carries. So a caller that never mentioned the scope cannot empty it.
    ///
    /// Keyed on `id`, for a sharper reason than [`Self::delete_row`]'s: the edit
    /// form can CHANGE `auth_type`, so `(service_name, auth_type)` cannot name
    /// the row being edited when the type is the field being edited.
    ///
    /// **Private on purpose**, same as [`Self::upsert_row`]: [`Self::update`]
    /// is the reachable mutator, and it emits.
    async fn update_row(
        pool: &PgPool,
        id: Uuid,
        base_urls: Option<&[String]>,
        auth_type: AuthType,
        auth_header: Option<&str>,
        auth_value: Option<&str>,
        env_var_name: Option<&str>,
    ) -> Result<Option<String>, sqlx::Error> {
        let auth_header = auth_header.unwrap_or("Authorization");
        let base_urls = base_urls.map(scope_members);

        // `COALESCE($2::text[], base_urls)` is what makes `None` mean "keep":
        // a NULL bind leaves the column as it stands. Only a named scope,
        // empty or not, declares it. `auth_value` gets two
        // static queries instead, because a NULL there is the value the user
        // just typed. `env_var_name` is always set (NULL clears it back to the
        // `CRED_` default).
        match auth_value {
            Some(value) => {
                sqlx::query_scalar::<_, String>(
                    r#"
                    UPDATE credentials
                    SET base_urls = COALESCE($2::text[], base_urls), auth_type = $3, auth_header = $4, auth_value = $5, env_var_name = $6,
                        scope_declared = scope_declared OR $2::text[] IS NOT NULL, updated_at = NOW()
                    WHERE id = $1
                    RETURNING service_name
                    "#,
                )
                .bind(id)
                .bind(&base_urls)
                .bind(auth_type.to_string())
                .bind(auth_header)
                .bind(Secret::<CredentialAuthValue>::new(value))
                .bind(env_var_name)
                .fetch_optional(pool)
                .await
            }
            None => {
                sqlx::query_scalar::<_, String>(
                    r#"
                    UPDATE credentials
                    SET base_urls = COALESCE($2::text[], base_urls), auth_type = $3, auth_header = $4, env_var_name = $5,
                        scope_declared = scope_declared OR $2::text[] IS NOT NULL, updated_at = NOW()
                    WHERE id = $1
                    RETURNING service_name
                    "#,
                )
                .bind(id)
                .bind(&base_urls)
                .bind(auth_type.to_string())
                .bind(auth_header)
                .bind(env_var_name)
                .fetch_optional(pool)
                .await
            }
        }
    }

    /// Update an existing credential and announce it. The only way to edit one.
    ///
    /// `CredentialUpdated` fires only when a row was actually updated, so an
    /// edit aimed at a missing id announces nothing. See [`Self::update_row`]
    /// for the `auth_value: None` "keep the stored secret" contract.
    ///
    /// Returns the updated row's `service_name`, which callers need because the
    /// id they passed in does not carry it: the `email_password` path uses it to
    /// find the matching `email_accounts` row.
    #[allow(clippy::too_many_arguments)] // one arg per editable column, plus the bus and actor
    pub async fn update(
        pool: &PgPool,
        event_bus: &EventBus,
        id: Uuid,
        base_urls: Option<&[String]>,
        auth_type: AuthType,
        auth_header: Option<&str>,
        auth_value: Option<&str>,
        env_var_name: Option<&str>,
        actor: Option<MessageOrigin>,
    ) -> Result<Option<String>, sqlx::Error> {
        let updated = Self::update_row(
            pool,
            id,
            base_urls,
            auth_type,
            auth_header,
            auth_value,
            env_var_name,
        )
        .await?;
        crate::core::secret_reference::forget_resolved_values();
        if let Some(service_name) = &updated {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::CredentialUpdated {
                        service_name: service_name.clone(),
                        actor,
                    }),
                    "[Credentials] CredentialUpdated",
                )
                .await;
        }
        Ok(updated)
    }

    /// Replace one credential's scope set, and announce it. Returns the row's
    /// `service_name` when one was touched.
    ///
    /// The narrow verb behind `PUT /api/v1/credential-base-urls`, so a caller
    /// widening a scope cannot also rewrite the secret, the auth type or the
    /// auth header. [`Self::update`] is the whole-row edit the Settings form
    /// makes; this is the one field a script or the CLI has business changing.
    ///
    /// Takes an already-normalized set: [`normalized_base_urls`] is the one
    /// speller, and the API boundary runs it so the refusal reaches the user.
    pub async fn set_base_urls(
        pool: &PgPool,
        event_bus: &EventBus,
        id: Uuid,
        base_urls: &[String],
        actor: Option<MessageOrigin>,
    ) -> Result<Option<String>, sqlx::Error> {
        let base_urls = scope_members(base_urls);
        let updated = sqlx::query_scalar::<_, String>(
            "UPDATE credentials SET base_urls = $2, scope_declared = true, updated_at = NOW() \
             WHERE id = $1 RETURNING service_name",
        )
        .bind(id)
        .bind(&base_urls)
        .fetch_optional(pool)
        .await?;
        if let Some(service_name) = &updated {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::CredentialUpdated {
                        service_name: service_name.clone(),
                        actor,
                    }),
                    "[Credentials] CredentialUpdated",
                )
                .await;
        }
        Ok(updated)
    }

    /// The startup pass behind ADR 0144 decision 4. A credential stored before
    /// the proxy checked scopes gets the one scope its `apis.json` entries
    /// agree on, announced. Returns each `(service_name, base_url)` it scoped.
    ///
    /// **Once, for the whole table.** Only a row whose `scope_declared` is
    /// false is a candidate, and the pass ends by declaring every row. Every
    /// write path declares too, an empty set included. So an empty scope a
    /// user chose is never filled in from `apis.json`, which an app can write
    /// (ADR 0156).
    ///
    /// It never appends to a scope set, and two entries naming different
    /// hosts are no answer. Only a type sent as a header is inferred: a
    /// `secret` is signed with, an `oauth_client` blob is read by the OAuth
    /// flow alone, and neither needs a host. Scoping any of them is the user's
    /// act, through Settings or `lucidos credentials set-base-urls`.
    pub async fn infer_undeclared_scopes(
        pool: &PgPool,
        event_bus: &EventBus,
        configured: &BTreeMap<String, BTreeSet<String>>,
    ) -> Result<Vec<(String, String)>, sqlx::Error> {
        let inferable: Vec<String> = [
            AuthType::ApiKey,
            AuthType::Bearer,
            AuthType::Basic,
            AuthType::Password,
        ]
        .iter()
        .map(AuthType::to_string)
        .collect();
        let mut scoped = Vec::new();
        for (service_name, scopes) in configured {
            // Through the same speller as every other write. A malformed
            // value is declined rather than stored as a scope the gate refuses.
            let base_url = match normalized_base_urls(scopes.iter().cloned().collect()) {
                Ok(urls) => match urls.as_slice() {
                    [one] => one.clone(),
                    _ => continue,
                },
                Err(reason) => {
                    log!("[Credentials] not scoping '{service_name}': {reason}");
                    continue;
                }
            };
            let updated: Option<String> = sqlx::query_scalar(
                "UPDATE credentials SET base_urls = ARRAY[$1], scope_declared = true, \
                 updated_at = NOW() \
                 WHERE service_name = $2 AND NOT scope_declared \
                 AND cardinality(base_urls) = 0 AND auth_type = ANY($3) \
                 RETURNING service_name",
            )
            .bind(&base_url)
            .bind(service_name)
            .bind(&inferable)
            .fetch_optional(pool)
            .await?;
            if updated.is_none() {
                continue;
            }
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::CredentialScopeInferred {
                        service_name: service_name.clone(),
                        base_url: base_url.clone(),
                        actor: None,
                    }),
                    "[Credentials] CredentialScopeInferred",
                )
                .await;
            scoped.push((service_name.clone(), base_url));
        }
        // What makes the pass once. Declaring a row changes nothing a reader
        // sees, so it announces nothing beyond the inferences above.
        sqlx::query("UPDATE credentials SET scope_declared = true WHERE NOT scope_declared")
            .execute(pool)
            .await?;
        Ok(scoped)
    }

    /// Find a credential whose scope covers the given URL.
    ///
    /// Matching is URL-aware: same scheme, host, effective port, and a path
    /// prefix on a segment boundary. A raw string prefix check would inject a
    /// credential scoped to `https://api.example.com` into
    /// `https://api.example.com.evil.test/`.
    ///
    /// The narrowest scope wins, measured on the MEMBER that matched rather
    /// than on the row. Two credentials can both cover a URL, and the one
    /// naming a longer path is the more specific answer whatever else its set
    /// holds. Ties keep the first row in name order, so the answer is stable.
    pub async fn find_by_url(pool: &PgPool, url: &str) -> Result<Option<Credential>, sqlx::Error> {
        // Blind to `oauth_client`, for the same reason [`Self::get`] is, and on
        // its own merits besides: an OAuth client registration's `auth_value` is
        // a `{client_id, client_secret, ...}` JSON blob, never a usable auth
        // header, so injecting it into an outbound request could only leak the
        // secret and fail the call. Its `base_url` is the provider's API host
        // (`https://www.googleapis.com`), which is exactly what a request to
        // that provider matches, and now that an API key may share the row's
        // name the two are far likelier to sit side by side. The tie-break here
        // is longest-matching-scope-first, so which one won was arbitrary. A
        // service account token goes to the `op` child only.
        let results = sqlx::query_as::<_, CredentialRow>(&format!(
            "SELECT {CREDENTIAL_COLUMNS} FROM credentials \
             WHERE {INJECTABLE_SQL} \
             ORDER BY service_name ASC"
        ))
        .fetch_all(pool)
        .await?;

        let mut best: Option<(usize, CredentialRow)> = None;
        for row in results {
            let Some(len) = longest_matching_scope(&row.2, url) else {
                continue;
            };
            if best.as_ref().is_none_or(|(seen, _)| len > *seen) {
                best = Some((len, row));
            }
        }

        Ok(best.map(|(_, row)| parse_credential(row)))
    }
}

/// The length of the longest member of `base_urls` covering `request_url`, or
/// `None` when no member does.
///
/// A matching member's PATH length, the only thing that can differ.
///
/// Scheme, host and port were all equal already, or the member did not match.
/// So ranking the whole string lets an explicit `:443` or a trailing slash
/// out-rank a member that really is narrower.
fn longest_matching_scope(base_urls: &[String], request_url: &str) -> Option<usize> {
    base_urls
        .iter()
        .filter(|base| credential_base_url_matches(base, request_url))
        .filter_map(|base| reqwest::Url::parse(base.trim()).ok())
        .map(|base| base.path().trim_end_matches('/').len())
        .max()
}

/// Whether a credential whose scope is `base_urls` may be presented to
/// `request_url`. True when ANY member covers it.
///
/// This is the whole of multi-scope matching. Each member is judged by
/// [`credential_base_url_matches`], exactly as a lone scope always was. So a
/// second member widens the credential by one named host and by nothing else.
pub fn credential_scope_covers(base_urls: &[String], request_url: &str) -> bool {
    longest_matching_scope(base_urls, request_url).is_some()
}

/// Whether `value` parses as a URL that names a host.
///
/// The one definition of what a scope member and a proxy upstream must look
/// like. `api::proxy::ScopedPipeline::bind` asks it of the outbound URL, and
/// [`normalized_base_urls`] asks it of every member. So the two cannot drift
/// apart on what the gate accepts.
pub(crate) fn url_names_a_host(value: &str) -> bool {
    reqwest::Url::parse(value.trim()).is_ok_and(|u| u.host_str().is_some())
}

/// Trim, drop blanks and collapse duplicates, keeping order. Infallible.
///
/// The floor under every write, applied by the store itself, so a scope set can
/// never hold a blank member. A blank one is a SECOND spelling of "no scope":
/// `cardinality(base_urls) = 0` is what the startup pass and the gate's "no
/// base URL" arm both ask, and `{""}` answers no to both while covering
/// nothing.
fn scope_members(base_urls: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(base_urls.len());
    for raw in base_urls {
        let trimmed = raw.trim();
        if !trimmed.is_empty() && !out.iter().any(|kept| kept == trimmed) {
            out.push(trimmed.to_string());
        }
    }
    out
}

/// [`scope_members`], plus a refusal for anything that is not a URL with a host.
///
/// The API boundary's speller. An unparseable member could only ever be refused
/// at the gate, silently and far from the form it was typed into. Refusing it at
/// the write puts the message where the user can act on it.
///
/// An empty result is legal and means the credential goes nowhere. A `secret`
/// carries that, and so does a row awaiting the startup scope pass.
pub fn normalized_base_urls(base_urls: Vec<String>) -> Result<Vec<String>, String> {
    let members = scope_members(&base_urls);
    if let Some(bad) = members.iter().find(|m| !url_names_a_host(m)) {
        return Err(format!(
            "'{bad}' is not a URL with a host. Write the scheme too, \
             for example https://api.example.com"
        ));
    }
    Ok(members)
}

/// Whether one scope member, `base_url`, covers `request_url`.
///
/// [`credential_scope_covers`] is what a caller asks. `core::git_auth` re-checks
/// the same predicate on every credential callback. A redirect therefore cannot
/// carry a secret to a host the user never scoped it to.
///
/// The path must stay under the member's prefix in every decoded reading an
/// upstream might apply, which [`check_path_within`] judges.
pub(crate) fn credential_base_url_matches(base_url: &str, request_url: &str) -> bool {
    let Ok(base) = reqwest::Url::parse(base_url.trim()) else {
        return false;
    };
    let Ok(request) = reqwest::Url::parse(request_url.trim()) else {
        return false;
    };

    if base.scheme() != request.scheme()
        || base.host_str() != request.host_str()
        || base.port_or_known_default() != request.port_or_known_default()
    {
        return false;
    }

    check_path_within(base.path().trim_end_matches('/'), &request).is_ok()
}

/// The prefixes of the secret variables a subprocess gets only by naming them.
const SECRET_VAR_PREFIXES: [&str; 2] = ["CRED_", "OAUTH_"];

/// The most of one script file [`SecretMentions`] reads.
const MENTION_SCAN_MAX_BYTES: u64 = 256 * 1024;

/// The most sibling scripts [`SecretMentions`] reads beside one named script.
const MENTION_SCAN_MAX_SIBLINGS: usize = 32;

/// The env-shaped words in the text a subprocess runs. A chat or script
/// subprocess gets a `CRED_*` or `OAUTH_*` variable only when this names it.
///
/// A word is a maximal run of `[A-Za-z0-9_]`. So `$CRED_X`,
/// `os.environ["CRED_X"]` and `${CRED_X:-}` all name `CRED_X`, while
/// `MY_CRED_X` names nothing. Only env-shaped words are kept: capitals,
/// digits and `_`, as a custom alias may be. A name built at run time is
/// declared with a header comment, `# lucidos-credentials: CRED_A CRED_B`,
/// which is just another mention.
#[derive(Debug, Default, Clone)]
pub struct SecretMentions {
    words: BTreeSet<String>,
}

impl SecretMentions {
    /// The mentions in one piece of text.
    pub fn scan(text: &str) -> Self {
        let mut mentions = Self::default();
        mentions.add_text(text);
        mentions
    }

    /// The mentions in a command or code, plus those in every workspace
    /// script it names. `python data/triggers/x/fetch.py` then gets what
    /// `fetch.py` names, with no edit to the command.
    pub fn of_command(text: &str, workspace: &std::path::Path) -> Self {
        let mut mentions = Self::scan(text);
        for script in named_workspace_scripts(text, workspace) {
            mentions.add_script(&script);
        }
        mentions
    }

    /// The mentions in one script file.
    pub fn of_script_file(path: &std::path::Path) -> Self {
        let mut mentions = Self::default();
        mentions.add_script(path);
        mentions
    }

    /// A script and the `.py` / `.sh` files beside it, since a trigger's
    /// `run.sh` may call its `fetch.py`, and a script may import a sibling.
    fn add_script(&mut self, path: &std::path::Path) {
        self.add_file(path);
        let Some(entries) = path.parent().and_then(|dir| std::fs::read_dir(dir).ok()) else {
            return;
        };
        let siblings = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|sibling| {
                sibling != path
                    && sibling.is_file()
                    && matches!(
                        sibling.extension().and_then(|ext| ext.to_str()),
                        Some("py" | "sh")
                    )
            })
            .take(MENTION_SCAN_MAX_SIBLINGS);
        for sibling in siblings {
            self.add_file(&sibling);
        }
    }

    fn add_text(&mut self, text: &str) {
        let words = text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));
        for word in words.filter(|word| is_env_shaped(word)) {
            self.words.insert(word.to_string());
        }
    }

    /// Reads the head of the file. An unreadable file adds nothing: the run
    /// then fails on its own terms, with the secret-names note to help.
    fn add_file(&mut self, path: &std::path::Path) {
        use std::io::Read;
        let mut head = Vec::new();
        let read = std::fs::File::open(path)
            .and_then(|file| file.take(MENTION_SCAN_MAX_BYTES).read_to_end(&mut head));
        if read.is_ok() {
            self.add_text(&String::from_utf8_lossy(&head));
        }
    }

    /// Whether the text names `var` as a whole word.
    pub fn names(&self, var: &str) -> bool {
        self.words.contains(var)
    }

    /// Whether resolving credentials could inject anything, or the names are
    /// wanted. True for a `CRED_*` mention, and for any other env-shaped word,
    /// which may be a credential's custom alias.
    pub fn wants_credentials(&self) -> bool {
        self.asks_for_names() || self.words.iter().any(|word| !word.starts_with("OAUTH_"))
    }

    /// Whether the text names any `OAUTH_*` variable, or asks for the names.
    /// Only a named account is refreshed.
    pub fn wants_oauth(&self) -> bool {
        self.asks_for_names() || self.secret_names().any(|name| name.starts_with("OAUTH_"))
    }

    /// Whether the text holds a bare prefix, as `env | grep CRED_` does. It
    /// names nothing, so it is a search for the names.
    fn asks_for_names(&self) -> bool {
        SECRET_VAR_PREFIXES
            .iter()
            .any(|prefix| self.words.contains(*prefix))
    }

    /// Every `CRED_*` / `OAUTH_*` name the text mentions.
    fn secret_names(&self) -> impl Iterator<Item = &str> {
        self.words.iter().map(String::as_str).filter(|word| {
            SECRET_VAR_PREFIXES
                .iter()
                .any(|prefix| word.len() > prefix.len() && word.starts_with(prefix))
        })
    }
}

/// The shape a custom alias may take, so `TOKEN` counts as well as `CRED_X`.
fn is_env_shaped(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !word.starts_with(|c: char| c.is_ascii_digit())
}

/// The `.py` / `.sh` files inside `workspace` that `text` names as a word.
/// A relative path resolves against the workspace, which is where the
/// subprocess runs.
fn named_workspace_scripts(text: &str, workspace: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(root) = workspace.canonicalize() else {
        return Vec::new();
    };
    let mut scripts = Vec::new();
    // A shell operator can touch a path, as in `./fetch.sh|tee log`.
    for word in text.split(|c: char| c.is_whitespace() || "|&;<>".contains(c)) {
        let word = word.trim_matches(|c: char| "\"'`()[]{},;".contains(c));
        if !(word.ends_with(".py") || word.ends_with(".sh")) {
            continue;
        }
        let Ok(path) = root.join(word).canonicalize() else {
            continue;
        };
        if path.starts_with(&root) && path.is_file() && !scripts.contains(&path) {
            scripts.push(path);
        }
    }
    scripts
}

/// Secret variables resolved against a subprocess's [`SecretMentions`].
#[derive(Debug, Default)]
pub struct NamedSecretVars {
    /// What the subprocess gets: only what its text names.
    pub env_vars: Vec<(String, String)>,
    /// Every `CRED_*` / `OAUTH_*` name it could have named. Names, never values.
    pub known_names: BTreeSet<String>,
    /// Why each named credential that could not be read was left out: the
    /// credential and the reason, never a value.
    pub unavailable: Vec<String>,
}

impl NamedSecretVars {
    pub fn extend(&mut self, other: NamedSecretVars) {
        self.env_vars.extend(other.env_vars);
        self.known_names.extend(other.known_names);
        self.unavailable.extend(other.unavailable);
    }

    /// The note a tool result carries for the named credentials left out.
    /// `None` when every one loaded.
    pub fn unavailable_note(&self) -> Option<String> {
        if self.unavailable.is_empty() {
            return None;
        }
        let lines: Vec<String> = self
            .unavailable
            .iter()
            .map(|reason| format!("[lucidos] Not set: {reason}."))
            .collect();
        Some(lines.join("\n"))
    }
}

/// The `CRED_*` variables for the credentials `mentions` names.
///
/// - **A mention injects the whole credential.** Naming either half of a
///   `password` pair, or its bare `CRED_<NAME>`, injects both halves.
/// - **A custom alias counts.** Naming `GITHUB_TOKEN` injects that credential,
///   under both its alias and `CRED_<NAME>`.
/// - **`oauth_client` and a service account token are never injected**
///   ([`AuthType::is_injectable`]).
/// - **Only a named credential is resolved.** A *secret reference* runs `op`
///   only when the text names its credential. One that cannot be read is left
///   out with a reason in [`NamedSecretVars::unavailable`], and the rest load.
///
/// A caller holding one credential the user configured by name (a
/// `script_handshake` layer's `credential` field) calls
/// [`credential_env_vars_for`] instead.
pub async fn named_credential_env_vars<R: SecretResolver + ?Sized>(
    credentials: Vec<Credential>,
    mentions: &SecretMentions,
    resolver: &R,
) -> NamedSecretVars {
    let mut named = NamedSecretVars::default();
    for cred in credentials {
        if !cred.auth_type.is_injectable() {
            continue;
        }
        let prefix = format!("CRED_{}", crate::core::env_var_segment(&cred.service_name));
        let names = env_var_names_for(
            &cred.service_name,
            cred.auth_type,
            cred.env_var_name.as_deref(),
        );
        let mentioned = mentions.names(&prefix) || names.iter().any(|name| mentions.names(name));
        named
            .known_names
            .extend(names.into_iter().filter(|name| name.starts_with("CRED_")));
        if mentioned {
            match credential_env_vars_for(cred, resolver).await {
                Ok(vars) => named.env_vars.extend(vars),
                Err(reason) => named.unavailable.push(reason),
            }
        }
    }
    named
}

/// The note a tool result carries when its text names a `CRED_*` / `OAUTH_*`
/// variable the workspace does not have, or holds a bare prefix. `None` when
/// every name resolved.
///
/// It lists names only, never values, nearest to the first unknown name first.
/// A mention that prefixes a known name, like `CRED_MAIL` for
/// `CRED_MAIL_PASSWORD`, is not unknown.
pub fn secret_names_note(
    mentions: &SecretMentions,
    known_names: &BTreeSet<String>,
) -> Option<String> {
    const LISTED: usize = 20;
    let unknown: Vec<&str> = mentions
        .secret_names()
        .filter(|name| {
            !known_names.contains(*name)
                && !known_names
                    .iter()
                    .any(|known| known.starts_with(&format!("{name}_")))
        })
        .collect();
    let bare: Vec<&str> = SECRET_VAR_PREFIXES
        .into_iter()
        .filter(|prefix| mentions.names(prefix))
        .collect();
    let first = *unknown.first().or(bare.first())?;

    let mut note = String::new();
    for prefix in &bare {
        note.push_str(&format!(
            "[lucidos] A bare {prefix} names no credential, so none was set.\n"
        ));
    }
    for name in &unknown {
        note.push_str(&format!(
            "[lucidos] {name} is not a credential in this workspace, so it was not set.\n"
        ));
    }
    let mut nearest: Vec<&String> = known_names.iter().collect();
    nearest.sort_by_key(|known| {
        let shared = known
            .chars()
            .zip(first.chars())
            .take_while(|(a, b)| a == b)
            .count();
        let distance = crate::core::event_subscription::known_names::levenshtein(known, first);
        (std::cmp::Reverse(shared), distance, (*known).clone())
    });
    let listed: Vec<&str> = nearest.iter().take(LISTED).map(|s| s.as_str()).collect();
    let more = if nearest.len() > LISTED { ", …" } else { "" };
    if listed.is_empty() {
        note.push_str("This workspace has no credentials to name.\n");
    } else {
        note.push_str(&format!(
            "Credentials you can name: {}{more}\n",
            listed.join(", ")
        ));
    }
    note.push_str(
        "Run `lucidos credentials list` for every stored credential and the names it sets. \
         To add one, use request_credential.",
    );
    Some(note)
}

/// The `CRED_*` variables for ONE credential, with no type filtering.
///
/// - `password`: `CRED_{NAME}_USERNAME` and `CRED_{NAME}_PASSWORD`, from the
///   JSON-encoded value. A custom `env_var_name` does not apply.
/// - every other type: `CRED_{NAME}`, plus the same value under the custom
///   `env_var_name` when one is set. The alias is additional, so a reference
///   to `CRED_{NAME}` keeps working.
///
/// `{NAME}` is the `service_name` through [`crate::core::env_var_segment`].
/// The handshake runner drops the alias, because a handshake script gets only
/// `CRED_*` and `OAUTH_*` names (`api::proxy_script_runner`).
///
/// A reference resolves here; one that cannot is an `Err` naming the
/// credential and the reason.
pub async fn credential_env_vars_for<R: SecretResolver + ?Sized>(
    cred: Credential,
    resolver: &R,
) -> Result<Vec<(String, String)>, String> {
    let names = env_var_names_for(
        &cred.service_name,
        cred.auth_type,
        cred.env_var_name.as_deref(),
    );
    let secret = cred.secret_with(resolver).await?;
    if cred.auth_type != AuthType::Password {
        return Ok(names
            .into_iter()
            .map(|name| (name, secret.clone()))
            .collect());
    }
    match serde_json::from_str::<serde_json::Value>(&secret) {
        Ok(parsed) => {
            let half = |key: &str| parsed[key].as_str().unwrap_or("").to_string();
            Ok(names
                .into_iter()
                .zip([half("username"), half("password")])
                .collect())
        }
        Err(e) => {
            // The script would see both halves missing with no signal, so
            // the log is the user's only clue.
            log!(
                "[Credentials] {} has invalid password JSON, skipping env injection: {}",
                cred.service_name,
                e
            );
            Ok(Vec::new())
        }
    }
}

/// The names a subprocess can set for one stored credential, as
/// `lucidos credentials list` shows them. A type that is not
/// [`AuthType::is_injectable`] sets none.
pub fn credential_env_var_names(
    service_name: &str,
    auth_type: AuthType,
    env_var_name: Option<&str>,
) -> Vec<String> {
    if !auth_type.is_injectable() {
        return Vec::new();
    }
    env_var_names_for(service_name, auth_type, env_var_name)
}

/// The one definition of a credential's variable names, with no type
/// filtering. A `password` sets `_USERNAME` then `_PASSWORD`. Every other type
/// sets its custom alias, when one is set, then the canonical `CRED_<NAME>`.
fn env_var_names_for(
    service_name: &str,
    auth_type: AuthType,
    env_var_name: Option<&str>,
) -> Vec<String> {
    let prefix = format!("CRED_{}", crate::core::env_var_segment(service_name));
    if auth_type == AuthType::Password {
        return vec![format!("{prefix}_USERNAME"), format!("{prefix}_PASSWORD")];
    }
    let alias = env_var_name
        .map(str::trim)
        .filter(|alias| !alias.is_empty() && *alias != prefix);
    alias
        .map(str::to_string)
        .into_iter()
        .chain([prefix])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::secret_reference::tests::ScriptedResolver;
    use crate::core::url_scope::MAX_DECODE_ROUNDS;
    use crate::test_support::{setup_test_db, teardown_test_db};

    /// Every agent-facing type list is rendered from `ALL`, so a variant
    /// missing from it is a type no agent can see. That is how `secret` shipped
    /// while `request_credential` still offered five.
    ///
    /// Nothing in the language enumerates a plain enum, so a hand-written `ALL`
    /// can silently fall short: the exhaustive matches elsewhere force a new
    /// variant to be CLASSIFIED, never to be LISTED. Reading the declaration
    /// back is what closes that.
    #[test]
    fn all_lists_every_variant() {
        let body = include_str!("credentials.rs")
            .split_once("pub enum AuthType {")
            .expect("the enum is declared in this file")
            .1
            .split_once('}')
            .expect("its declaration closes")
            .0;
        let mut declared: Vec<&str> = body
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("//") && !line.starts_with('#'))
            .map(|line| line.trim_end_matches(','))
            .collect();
        declared.sort_unstable();

        let mut listed: Vec<String> = AuthType::ALL.iter().map(|t| format!("{t:?}")).collect();
        listed.sort();
        assert_eq!(
            listed, declared,
            "AuthType::ALL must hold every declared variant. A missing one is \
             offered by no agent-facing surface, and a stale one names nothing"
        );

        for auth_type in AuthType::ALL {
            assert_eq!(
                AuthType::parse(&auth_type.to_string()),
                *auth_type,
                "every listed variant round-trips through its wire spelling"
            );
        }
    }

    /// The two exclusions are deliberate, and both would be a bug to offer.
    #[test]
    fn the_agent_may_request_every_type_but_email_and_unknown() {
        assert_eq!(
            AuthType::agent_requestable_values(),
            vec![
                "api_key",
                "bearer",
                "basic",
                "password",
                "oauth_client",
                "secret"
            ],
            "the request_credential enum, in schema order"
        );
        assert!(!AuthType::EmailPassword.agent_requestable());
        assert!(!AuthType::Unknown.agent_requestable());
    }

    async fn emitted(pool: &PgPool, event_type: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
            .bind(event_type)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// The load-bearing guarantee: a credential write and its announcement are
    /// one operation. A first save reads as a creation and a second as an
    /// update, so the timeline distinguishes connecting a provider from
    /// rotating its key.
    #[tokio::test]
    async fn upsert_announces_creation_then_update() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        CredentialStore::upsert(
            &pool,
            &bus,
            "openai",
            &["https://api.openai.com".to_string()],
            AuthType::ApiKey,
            "sk-one",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(emitted(&pool, "CredentialCreated").await, 1);
        assert_eq!(emitted(&pool, "CredentialUpdated").await, 0);

        CredentialStore::upsert(
            &pool,
            &bus,
            "openai",
            &["https://api.openai.com".to_string()],
            AuthType::ApiKey,
            "sk-two",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            emitted(&pool, "CredentialCreated").await,
            1,
            "overwriting an existing service is not a creation"
        );
        assert_eq!(emitted(&pool, "CredentialUpdated").await, 1);

        teardown_test_db(&db).await;
    }

    /// An edit aimed at a service that does not exist changes nothing, so it
    /// announces nothing.
    #[tokio::test]
    async fn update_announces_only_when_a_row_was_touched() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        assert!(CredentialStore::update(
            &pool,
            &bus,
            Uuid::new_v4(),
            Some(&["https://example.test".to_string()][..]),
            AuthType::ApiKey,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap()
        .is_none());
        assert_eq!(emitted(&pool, "CredentialUpdated").await, 0);

        let id = CredentialStore::upsert(
            &pool,
            &bus,
            "svc",
            &["https://example.test".to_string()],
            AuthType::ApiKey,
            "secret",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            CredentialStore::update(
                &pool,
                &bus,
                id,
                Some(&["https://example.test/v2".to_string()][..]),
                AuthType::ApiKey,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap()
            .as_deref(),
            Some("svc"),
            "the updated row's name comes back, since the id does not carry it"
        );
        assert_eq!(emitted(&pool, "CredentialUpdated").await, 1);

        teardown_test_db(&db).await;
    }

    /// A blank member is not a second spelling of "no scope". The store drops
    /// it, so `cardinality(base_urls) = 0` stays the one question the gate and
    /// the startup pass both ask.
    #[tokio::test]
    async fn a_blank_member_is_never_stored() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        let id = CredentialStore::upsert(
            &pool,
            &bus,
            "svc",
            &[String::new(), "  ".to_string()],
            AuthType::Secret,
            "shared-secret",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(CredentialStore::get(&pool, "svc")
            .await
            .unwrap()
            .expect("the row exists")
            .base_urls
            .is_empty());

        CredentialStore::update(
            &pool,
            &bus,
            id,
            Some(&["   ".to_string()]),
            AuthType::Secret,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(CredentialStore::get(&pool, "svc")
            .await
            .unwrap()
            .expect("the row exists")
            .base_urls
            .is_empty());

        teardown_test_db(&db).await;
    }

    /// `None` keeps the stored scope, on the same terms `auth_value: None`
    /// keeps the secret. An edit of the auth header alone must not empty a set,
    /// which would refuse the credential everywhere with nothing to undo it.
    #[tokio::test]
    async fn updating_without_a_scope_keeps_the_stored_one() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        let id = CredentialStore::upsert(
            &pool,
            &bus,
            "svc",
            &["https://api.example.test".to_string()],
            AuthType::ApiKey,
            "k",
            None,
            None,
            None,
        )
        .await
        .unwrap();

        CredentialStore::update(
            &pool,
            &bus,
            id,
            None,
            AuthType::ApiKey,
            Some("X-Api-Key"),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let kept = CredentialStore::get(&pool, "svc").await.unwrap().unwrap();
        assert_eq!(kept.base_urls, vec!["https://api.example.test".to_string()]);
        assert_eq!(kept.auth_header, "X-Api-Key", "the edit still landed");

        // An EMPTY slice is a different answer, and it does clear the set.
        CredentialStore::update(
            &pool,
            &bus,
            id,
            Some(&[]),
            AuthType::Secret,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(CredentialStore::get(&pool, "svc")
            .await
            .unwrap()
            .unwrap()
            .base_urls
            .is_empty());

        teardown_test_db(&db).await;
    }

    /// Removal announces exactly once: a repeated delete finds no row and stays
    /// silent, so a racing double-remove cannot emit twice.
    #[tokio::test]
    async fn delete_announces_once_and_is_silent_when_nothing_was_removed() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        let id = CredentialStore::upsert(
            &pool,
            &bus,
            "svc",
            &["https://example.test".to_string()],
            AuthType::ApiKey,
            "secret",
            None,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(CredentialStore::delete(&pool, &bus, id, None)
            .await
            .unwrap());
        assert_eq!(emitted(&pool, "CredentialDeleted").await, 1);

        assert!(
            !CredentialStore::delete(&pool, &bus, id, None)
                .await
                .unwrap(),
            "second delete removes nothing"
        );
        assert_eq!(
            emitted(&pool, "CredentialDeleted").await,
            1,
            "and therefore announces nothing"
        );

        teardown_test_db(&db).await;
    }

    /// IMAP and SMTP log in with the copy of the password in `email_accounts`,
    /// so deleting the credential alone left mail access live and invisible.
    #[tokio::test]
    async fn deleting_an_email_password_ends_mail_access() {
        use crate::core::EmailStore;
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        EmailStore::upsert(
            &pool,
            "mail",
            "user@example.com",
            "imap.example.com",
            993,
            "smtp.example.com",
            465,
            "user@example.com",
            true,
            true,
        )
        .await
        .unwrap();
        EmailStore::upsert(
            &pool,
            "other",
            "other@example.com",
            "imap.example.com",
            993,
            "smtp.example.com",
            465,
            "other@example.com",
            true,
            true,
        )
        .await
        .unwrap();
        CredentialStore::upsert(
            &pool,
            &bus,
            "other",
            &[],
            AuthType::EmailPassword,
            "kept",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let id = CredentialStore::upsert(
            &pool,
            &bus,
            "mail",
            &[],
            AuthType::EmailPassword,
            "hunter2",
            None,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(CredentialStore::delete(&pool, &bus, id, None)
            .await
            .unwrap());

        let account = EmailStore::get(&pool, "mail").await.unwrap().unwrap();
        assert!(
            account.password == crate::core::MailPassword::Missing,
            "the mailbox password must go with it"
        );
        let other = EmailStore::get(&pool, "other").await.unwrap().unwrap();
        assert!(
            other.password == crate::core::MailPassword::Saved("kept".into()),
            "another account is untouched"
        );

        teardown_test_db(&db).await;
    }

    fn make_cred(service_name: &str, auth_type: AuthType, auth_value: &str) -> Credential {
        Credential::fixture(service_name, auth_type, auth_value)
    }

    /// A resolver for credentials holding no reference: any call fails the test.
    fn no_references() -> ScriptedResolver {
        ScriptedResolver::answering(Vec::new())
    }

    /// [`credential_env_vars_for`] for a credential holding no reference.
    fn env_for(cred: Credential) -> Vec<(String, String)> {
        futures::executor::block_on(credential_env_vars_for(cred, &no_references()))
            .expect("a stored value always loads")
    }

    /// [`named_credential_env_vars`] over credentials holding no reference.
    fn named_for(creds: Vec<Credential>, mentions: &SecretMentions) -> NamedSecretVars {
        futures::executor::block_on(named_credential_env_vars(creds, mentions, &no_references()))
    }

    #[test]
    fn custom_env_var_name_adds_alias_alongside_cred_prefix() {
        let mut cred = make_cred("github", AuthType::Bearer, "ghp_xxx");
        cred.env_var_name = Some("GITHUB_TOKEN".to_string());
        let env: std::collections::HashMap<_, _> = env_for(cred).into_iter().collect();
        // Additive: the custom name is an EXTRA alias, the canonical CRED_<NAME>
        // is still emitted so existing references keep working.
        assert_eq!(env.get("GITHUB_TOKEN").map(String::as_str), Some("ghp_xxx"));
        assert_eq!(env.get("CRED_GITHUB").map(String::as_str), Some("ghp_xxx"));
    }

    #[test]
    fn blank_custom_env_var_name_emits_only_cred_prefix() {
        let mut cred = make_cred("github", AuthType::Bearer, "ghp_xxx");
        cred.env_var_name = Some("   ".to_string());
        let pairs = env_for(cred);
        assert_eq!(pairs.len(), 1, "blank custom name adds no alias");
        assert_eq!(pairs[0].0, "CRED_GITHUB");
        assert_eq!(pairs[0].1, "ghp_xxx");
    }

    #[test]
    fn custom_name_equal_to_prefix_is_not_duplicated() {
        let mut cred = make_cred("github", AuthType::Bearer, "ghp_xxx");
        cred.env_var_name = Some("CRED_GITHUB".to_string());
        let pairs = env_for(cred);
        assert_eq!(
            pairs.len(),
            1,
            "custom name equal to the default must not double-emit"
        );
        assert_eq!(pairs[0].0, "CRED_GITHUB");
    }

    #[test]
    fn password_emits_split_username_and_password() {
        let cred = make_cred(
            "comfort-cloud",
            AuthType::Password,
            r#"{"username":"alice","password":"s3cret"}"#,
        );
        let env: std::collections::HashMap<_, _> = env_for(cred).into_iter().collect();
        assert_eq!(
            env.get("CRED_COMFORT_CLOUD_USERNAME").map(String::as_str),
            Some("alice")
        );
        assert_eq!(
            env.get("CRED_COMFORT_CLOUD_PASSWORD").map(String::as_str),
            Some("s3cret")
        );
        assert!(!env.contains_key("CRED_COMFORT_CLOUD"));
    }

    #[test]
    fn api_key_emits_single_cred_var() {
        let cred = make_cred(
            "firebase-web-api-key",
            AuthType::ApiKey,
            "AIzaSy-fake-key-value",
        );
        let env: std::collections::HashMap<_, _> = env_for(cred).into_iter().collect();
        assert_eq!(
            env.get("CRED_FIREBASE_WEB_API_KEY").map(String::as_str),
            Some("AIzaSy-fake-key-value")
        );
        assert!(!env.contains_key("CRED_FIREBASE_WEB_API_KEY_USERNAME"));
        assert!(!env.contains_key("CRED_FIREBASE_WEB_API_KEY_PASSWORD"));
    }

    #[test]
    fn bearer_emits_single_cred_var() {
        let cred = make_cred("openai-key", AuthType::Bearer, "sk-test-123");
        let env: std::collections::HashMap<_, _> = env_for(cred).into_iter().collect();
        assert_eq!(
            env.get("CRED_OPENAI_KEY").map(String::as_str),
            Some("sk-test-123")
        );
        assert!(!env.contains_key("CRED_OPENAI_KEY_USERNAME"));
    }

    #[test]
    fn basic_emits_single_cred_var_with_user_colon_password() {
        // `basic` stores `user:password` literally; we hand the whole string
        // to the script as `CRED_<NAME>` and let the script decide how to
        // split it. Mirrors what `run_python` / `run_bash` already do.
        let cred = make_cred("svc-basic", AuthType::Basic, "alice:s3cret");
        let env: std::collections::HashMap<_, _> = env_for(cred).into_iter().collect();
        assert_eq!(
            env.get("CRED_SVC_BASIC").map(String::as_str),
            Some("alice:s3cret")
        );
        assert!(!env.contains_key("CRED_SVC_BASIC_USERNAME"));
        assert!(!env.contains_key("CRED_SVC_BASIC_PASSWORD"));
    }

    #[test]
    fn parse_unknown_db_string_yields_unknown_variant_not_apikey() {
        // Regression: a row with an auth_type the binary doesn't recognize
        // (e.g. written by a newer version) used to be coerced to ApiKey and
        // injected with the wrong shape. parse() must return Unknown so the
        // env-var path / header-injection sites fall through safely.
        assert_eq!(AuthType::parse("api_key"), AuthType::ApiKey);
        assert_eq!(AuthType::parse("totally-new-thing"), AuthType::Unknown);
        assert_eq!(AuthType::parse(""), AuthType::Unknown);
    }

    #[test]
    fn unknown_round_trips_through_to_string_and_parse() {
        assert_eq!(AuthType::Unknown.to_string(), "unknown");
        assert_eq!(AuthType::parse("unknown"), AuthType::Unknown);
    }

    #[test]
    fn unknown_emits_single_cred_var_not_password_pair() {
        // The Password pair check (`==` AuthType::Password) must not match
        // Unknown — Unknown lands in the else branch with a single CRED_*
        // variable carrying the raw value, same as ApiKey/Bearer/Basic.
        let cred = make_cred("legacy-svc", AuthType::Unknown, "raw-value");
        let env: std::collections::HashMap<_, _> = env_for(cred).into_iter().collect();
        assert_eq!(
            env.get("CRED_LEGACY_SVC").map(String::as_str),
            Some("raw-value")
        );
        assert!(!env.contains_key("CRED_LEGACY_SVC_USERNAME"));
        assert!(!env.contains_key("CRED_LEGACY_SVC_PASSWORD"));
    }

    #[test]
    fn name_transform_uppercases_and_replaces_separators() {
        let cred = make_cred("snake.storage shared-prod", AuthType::ApiKey, "v");
        let env: std::collections::HashMap<_, _> = env_for(cred).into_iter().collect();
        assert_eq!(
            env.get("CRED_SNAKE_STORAGE_SHARED_PROD")
                .map(String::as_str),
            Some("v")
        );
    }

    /// A namespaced service name (`oauth:<provider>`, `email:<account>`) used
    /// to keep its colon, producing `CRED_OAUTH:GOOGLE`: present in `environ`,
    /// unreachable from bash, and readable from Python. Every character outside
    /// `[A-Z0-9_]` is a separator now, so the two namespaces inject a name a
    /// shell can actually expand.
    #[test]
    fn name_transform_replaces_namespace_colon_and_address_chars() {
        let env: std::collections::HashMap<_, _> = [
            make_cred("oauth:google", AuthType::ApiKey, "client-blob"),
            make_cred("email:user@example.com", AuthType::EmailPassword, "pw"),
        ]
        .into_iter()
        .flat_map(env_for)
        .collect();
        assert_eq!(
            env.get("CRED_OAUTH_GOOGLE").map(String::as_str),
            Some("client-blob")
        );
        assert_eq!(
            env.get("CRED_EMAIL_USER_EXAMPLE_COM").map(String::as_str),
            Some("pw")
        );
        // Every injected name must be a legal shell identifier, or the variable
        // is set and unreadable rather than missing and diagnosable.
        for name in env.keys() {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
                "injected env var {name} is not a legal shell identifier"
            );
        }
    }

    fn named_vars(creds: Vec<Credential>, text: &str) -> std::collections::HashMap<String, String> {
        named_for(creds, &SecretMentions::scan(text))
            .env_vars
            .into_iter()
            .collect()
    }

    /// The core of the rule: a subprocess gets only the credential its text
    /// names, never the rest of the workspace's.
    #[test]
    fn only_the_named_credential_is_injected() {
        let creds = || {
            vec![
                make_cred("github", AuthType::Bearer, "gh-value"),
                make_cred("slack", AuthType::Bearer, "slack-value"),
            ]
        };
        let env = named_vars(creds(), r#"curl -H "Authorization: Bearer $CRED_GITHUB" x"#);
        assert_eq!(env.get("CRED_GITHUB").map(String::as_str), Some("gh-value"));
        assert!(
            !env.contains_key("CRED_SLACK"),
            "an unnamed credential leaked"
        );

        assert!(named_vars(creds(), "ls -la").is_empty());
        assert!(
            named_vars(creds(), "echo $MY_CRED_GITHUB").is_empty(),
            "a mention is a whole word"
        );
    }

    /// Naming either half of a password pair, or its bare prefix, injects both.
    #[test]
    fn a_password_half_injects_the_whole_pair() {
        let cred = || {
            make_cred(
                "mail",
                AuthType::Password,
                r#"{"username":"alice","password":"s3cret"}"#,
            )
        };
        for text in [
            "os.environ['CRED_MAIL_USERNAME']",
            "echo $CRED_MAIL_PASSWORD",
            "# lucidos-credentials: CRED_MAIL",
        ] {
            let env = named_vars(vec![cred()], text);
            assert_eq!(env.len(), 2, "{text}");
            assert_eq!(
                env.get("CRED_MAIL_PASSWORD").map(String::as_str),
                Some("s3cret")
            );
        }
    }

    /// A custom alias injects its credential when the text names the alias
    /// or the canonical name, under both names.
    #[test]
    fn an_alias_or_its_canonical_name_injects_both() {
        let cred = || {
            let mut cred = make_cred("github", AuthType::Bearer, "gh-value");
            cred.env_var_name = Some("GITHUB_TOKEN".to_string());
            cred
        };
        for text in ["gh pr list  # GITHUB_TOKEN", "echo $CRED_GITHUB"] {
            let env = named_vars(vec![cred()], text);
            assert_eq!(
                env.get("GITHUB_TOKEN").map(String::as_str),
                Some("gh-value")
            );
            assert_eq!(env.get("CRED_GITHUB").map(String::as_str), Some("gh-value"));
        }
        assert!(named_vars(vec![cred()], "gh pr list").is_empty());
    }

    /// An OAuth client registration stays out even when named: only the OAuth
    /// flow reads its blob.
    #[test]
    fn an_oauth_client_is_never_injected() {
        let named = named_for(
            vec![make_cred("google", AuthType::OauthClient, "{}")],
            &SecretMentions::scan("echo $CRED_GOOGLE"),
        );
        assert!(named.env_vars.is_empty());
        assert!(named.known_names.is_empty());
    }

    /// A custom alias needs no underscore: `TOKEN` is a valid alias.
    #[test]
    fn an_alias_without_an_underscore_injects_its_credential() {
        let mut aliased = make_cred("github", AuthType::Bearer, "ghp_value");
        aliased.env_var_name = Some("TOKEN".to_string());
        let named = named_for(vec![aliased], &SecretMentions::scan("echo $TOKEN"));
        assert_eq!(named.env_vars.len(), 2, "the alias and CRED_GITHUB");
    }

    /// A text with no env-shaped word needs no credential or OAuth query.
    #[test]
    fn plain_text_wants_no_secret_query() {
        for text in ["ls -la", "print(my_var)", "git log --oneline"] {
            let mentions = SecretMentions::scan(text);
            assert!(!mentions.wants_credentials(), "{text}");
            assert!(!mentions.wants_oauth(), "{text}");
        }
        assert!(SecretMentions::scan("echo $CRED_X").wants_credentials());
        assert!(!SecretMentions::scan("echo $CRED_X").wants_oauth());
        assert!(SecretMentions::scan("echo $OAUTH_GOOGLE_ACCESS_TOKEN").wants_oauth());
        assert!(
            !SecretMentions::scan("echo $OAUTH_GOOGLE_ACCESS_TOKEN").wants_credentials(),
            "an OAuth mention alone loads no credential"
        );
    }

    /// A command running a workspace script names that script's mentions too,
    /// and a script outside the workspace adds nothing.
    #[test]
    fn a_command_names_the_workspace_scripts_it_runs() {
        let workspace = tempfile::TempDir::new().expect("tempdir");
        let dir = workspace.path().join("data/triggers/watch");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("fetch.py"),
            "import os\nos.environ['CRED_GITHUB']\n",
        )
        .unwrap();
        let outside = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(outside.path().join("x.sh"), "echo $CRED_SLACK\n").unwrap();

        let mentions = SecretMentions::of_command(
            &format!(
                "python data/triggers/watch/fetch.py && sh {}",
                outside.path().join("x.sh").display()
            ),
            workspace.path(),
        );
        assert!(mentions.names("CRED_GITHUB"));
        assert!(
            !mentions.names("CRED_SLACK"),
            "a script outside the workspace"
        );

        let quoted = SecretMentions::of_command(
            r#"subprocess.run(["python", "data/triggers/watch/fetch.py"])"#,
            workspace.path(),
        );
        assert!(quoted.names("CRED_GITHUB"), "a quoted path in Python code");

        for command in [
            "python data/triggers/watch/fetch.py|tee log",
            "python data/triggers/watch/fetch.py&&echo done",
        ] {
            let piped = SecretMentions::of_command(command, workspace.path());
            assert!(piped.names("CRED_GITHUB"), "{command}");
        }
    }

    /// A trigger's `run.sh` that calls its `fetch.py` gets what `fetch.py`
    /// names, whether the command or the scheduler names `run.sh`.
    #[test]
    fn a_named_script_brings_its_sibling_scripts_mentions() {
        let workspace = tempfile::TempDir::new().expect("tempdir");
        let dir = workspace.path().join("data/triggers/watch");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.sh"), "python3 fetch.py\n").unwrap();
        std::fs::write(dir.join("fetch.py"), "os.environ['CRED_GITHUB']\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "CRED_SLACK\n").unwrap();

        let by_command =
            SecretMentions::of_command("sh data/triggers/watch/run.sh", workspace.path());
        let by_scheduler = SecretMentions::of_script_file(&dir.join("run.sh"));
        for mentions in [by_command, by_scheduler] {
            assert!(mentions.names("CRED_GITHUB"));
            assert!(!mentions.names("CRED_SLACK"), "only scripts count");
        }
    }

    /// An unknown name never fails the call. The note names the real
    /// credentials, nearest first, and never carries a value.
    #[test]
    fn an_unknown_name_gets_a_note_with_names_only() {
        let named = named_for(
            vec![
                make_cred("github", AuthType::Bearer, "gh-value"),
                make_cred(
                    "mail",
                    AuthType::Password,
                    r#"{"username":"alice","password":"s3cret"}"#,
                ),
            ],
            &SecretMentions::scan("echo $CRED_MAIL_PASSWRD $CRED_MAIL"),
        );
        let note = secret_names_note(
            &SecretMentions::scan("echo $CRED_MAIL_PASSWRD $CRED_MAIL"),
            &named.known_names,
        )
        .expect("an unknown name gets a note");
        assert!(
            note.contains("CRED_MAIL_PASSWRD is not a credential"),
            "{note}"
        );
        assert!(
            !note.contains("CRED_MAIL is not"),
            "a prefix of a real name is not unknown: {note}"
        );
        assert!(
            note.contains("Credentials you can name: CRED_MAIL_PASSWORD, CRED_MAIL_USERNAME"),
            "nearest first: {note}"
        );
        for value in ["gh-value", "s3cret", "alice"] {
            assert!(!note.contains(value), "the note leaked a value: {note}");
        }

        assert!(secret_names_note(
            &SecretMentions::scan("echo $CRED_GITHUB"),
            &named.known_names
        )
        .is_none());
        assert!(secret_names_note(&SecretMentions::scan("ls"), &named.known_names).is_none());
    }

    /// `env | grep CRED_` is how an agent looks for its credentials. It names
    /// none, so it sets none, and the note lists the names to use instead.
    #[test]
    fn a_bare_prefix_asks_for_the_names() {
        for text in ["env | grep CRED_", "echo ${!OAUTH_@}"] {
            let mentions = SecretMentions::scan(text);
            assert!(mentions.wants_credentials(), "{text}");
            assert!(mentions.wants_oauth(), "{text}");
            let known: BTreeSet<String> = ["CRED_GITHUB".to_string()].into();
            let note = secret_names_note(&mentions, &known).expect("a bare prefix gets a note");
            assert!(note.contains("names no credential"), "{note}");
            assert!(
                note.contains("Credentials you can name: CRED_GITHUB"),
                "{note}"
            );
            assert!(note.contains("lucidos credentials list"), "{note}");
        }
        assert!(
            named_vars(
                vec![make_cred("github", AuthType::Bearer, "gh-value")],
                "env | grep CRED_"
            )
            .is_empty(),
            "a bare prefix sets nothing"
        );
    }

    /// The names a credential sets, as `lucidos credentials list` shows them.
    /// They are exactly the names the injection uses.
    #[test]
    fn credential_info_lists_the_names_its_credential_sets() {
        let mut github = make_cred("github", AuthType::Bearer, "gh-value");
        github.env_var_name = Some("GITHUB_TOKEN".to_string());
        let mail = make_cred(
            "mail",
            AuthType::Password,
            r#"{"username":"alice","password":"s3cret"}"#,
        );
        for cred in [github, mail] {
            let names = credential_env_var_names(
                &cred.service_name,
                cred.auth_type,
                cred.env_var_name.as_deref(),
            );
            let injected: Vec<String> = env_for(cred).into_iter().map(|(name, _)| name).collect();
            assert_eq!(names, injected);
        }
        assert_eq!(
            credential_env_var_names("google", AuthType::OauthClient, None),
            Vec::<String>::new(),
            "an OAuth client is never set in a subprocess"
        );
    }

    #[test]
    fn debug_redacts_credential_secret() {
        let cred = make_cred(
            "oauth:google",
            AuthType::OauthClient,
            r#"{"client_id":"abc","client_secret":"super-secret-value"}"#,
        );
        let dbg = format!("{:?}", cred);
        // The stored secret must never appear in a `{:?}` rendering.
        assert!(!dbg.contains("super-secret-value"), "secret leaked: {dbg}");
        assert!(
            dbg.contains("stored_value: \"<redacted>\""),
            "expected a redacted stored_value: {dbg}"
        );
        // Non-secret fields stay visible for debugging.
        assert!(dbg.contains("oauth:google"));
        assert!(dbg.contains("OauthClient"));
    }

    #[test]
    fn credential_url_match_requires_same_origin_not_string_prefix() {
        assert!(credential_base_url_matches(
            "https://api.example.com",
            "https://api.example.com/v1/users"
        ));
        assert!(!credential_base_url_matches(
            "https://api.example.com",
            "https://api.example.com.evil.test/v1/users"
        ));
        assert!(!credential_base_url_matches(
            "https://api.example.com",
            "http://api.example.com/v1/users"
        ));
    }

    #[test]
    fn credential_url_match_respects_path_segment_boundary() {
        assert!(credential_base_url_matches(
            "https://api.example.com/v1",
            "https://api.example.com/v1/users"
        ));
        assert!(credential_base_url_matches(
            "https://api.example.com/v1/",
            "https://api.example.com/v1/users"
        ));
        assert!(!credential_base_url_matches(
            "https://api.example.com/v1",
            "https://api.example.com/v10/users"
        ));
    }

    /// Regression: the scope judged only the first parse. An encoded separator
    /// or dot segment that an upstream decodes again then carried the
    /// credential out of its path prefix.
    #[test]
    fn credential_url_match_judges_every_decoded_reading_of_the_path() {
        let base = "https://api.example.test/safe";
        for escaping in [
            "https://api.example.test/safe/%2e%2e%2fadmin",
            "https://api.example.test/safe/%252e%252e%252fadmin",
            "https://api.example.test/safe/..%255cadmin",
            "https://api.example.test/safe/%252e%252e/admin",
            "https://api.example.test/safe/a/%2e%2e%2f%2e%2e%2fadmin",
            "https://api.example.test/safe/%2%65%2%65/admin",
        ] {
            assert!(
                !credential_base_url_matches(base, escaping),
                "credential must not cover {escaping}"
            );
        }

        let too_deep = format!(
            "https://api.example.test/safe/%{}2fitems",
            "25".repeat(MAX_DECODE_ROUNDS)
        );
        assert!(!credential_base_url_matches(base, &too_deep));

        for contained in [
            "https://api.example.test/safe/a%2fb",
            "https://api.example.test/safe/a/%2e%2e%2fb",
            "https://api.example.test/safe/name%20with%20space",
        ] {
            assert!(
                credential_base_url_matches(base, contained),
                "credential must still cover {contained}"
            );
        }
    }

    #[test]
    fn credential_url_match_rejects_invalid_urls() {
        assert!(!credential_base_url_matches(
            "api.example.com",
            "https://api.example.com/v1"
        ));
        assert!(!credential_base_url_matches(
            "https://api.example.com",
            "not a url"
        ));
    }

    /// The defect this column exists for. One HMAC pair serves Binance's spot
    /// and futures hosts, and one scope could not say so.
    #[test]
    fn a_scope_set_covers_every_host_it_names_and_no_other() {
        let scope = vec![
            "https://api.binance.com".to_string(),
            "https://fapi.binance.com".to_string(),
        ];
        assert!(credential_scope_covers(
            &scope,
            "https://api.binance.com/api/v3/account"
        ));
        assert!(credential_scope_covers(
            &scope,
            "https://fapi.binance.com/fapi/v2/balance"
        ));
        assert!(!credential_scope_covers(
            &scope,
            "https://evil.test/fapi/v2/balance"
        ));
        // A second member widens by one named host, never by a shape. The
        // sibling-host trick the single-scope check already refused stays
        // refused.
        assert!(!credential_scope_covers(
            &scope,
            "https://dapi.binance.com/x"
        ));
        assert!(!credential_scope_covers(
            &scope,
            "https://api.binance.com.evil.test/x"
        ));
    }

    /// An empty set is the fail-closed state, and it is reachable: a `secret`
    /// carries it, and so does a row the startup pass has not scoped.
    #[test]
    fn an_empty_scope_set_covers_nothing() {
        assert!(!credential_scope_covers(&[], "https://api.example.com"));
    }

    /// Ranking on the whole string let a longer SPELLING of the same scope beat
    /// a member that really is narrower. `https://api.example.com:443` is 4
    /// characters longer than `https://api.example.com/v1` is short, so the
    /// broad row won a request both cover. Only the path can differ once a
    /// member matches, so only the path is ranked.
    #[test]
    fn specificity_ranks_the_path_and_not_the_spelling() {
        let broad = vec!["https://api.example.com:443".to_string()];
        let narrow = vec!["https://api.example.com/v1".to_string()];
        let url = "https://api.example.com/v1/things";

        let broad_rank = longest_matching_scope(&broad, url).expect("the broad scope covers it");
        let narrow_rank = longest_matching_scope(&narrow, url).expect("the narrow scope covers it");
        assert!(
            narrow_rank > broad_rank,
            "the member carrying a path is the more specific answer: \
             narrow={narrow_rank} broad={broad_rank}"
        );

        // A trailing slash is the same scope written twice, so it ranks equal.
        let slashed = vec!["https://api.example.com/v1/".to_string()];
        assert_eq!(longest_matching_scope(&slashed, url), Some(narrow_rank));
    }

    /// A blank member is a second spelling of "no scope": it covers nothing,
    /// but `cardinality(base_urls) = 0` answers no, so the startup pass would
    /// skip the row and the gate's "no base URL" arm would not fire. The store
    /// drops it before it can be written.
    #[test]
    fn a_blank_member_never_becomes_a_scope() {
        assert!(scope_members(&[String::new(), "   ".to_string()]).is_empty());
        assert_eq!(
            scope_members(&[
                " https://api.example.com ".to_string(),
                String::new(),
                "https://api.example.com".to_string(),
            ]),
            vec!["https://api.example.com".to_string()],
            "trims, drops the blank, and collapses the duplicate"
        );
    }

    /// Specificity is PATH depth. Scheme, host and port were equal already, or
    /// the member did not match, so a longer string can be the wider scope.
    #[test]
    fn a_longer_string_does_not_out_rank_a_narrower_path() {
        let whole_host = vec!["https://api.example.com:443".to_string()];
        let one_path = vec!["https://api.example.com/v1".to_string()];
        let url = "https://api.example.com/v1/orders";
        assert!(credential_scope_covers(&whole_host, url));
        assert!(credential_scope_covers(&one_path, url));
        assert!(
            longest_matching_scope(&one_path, url) > longest_matching_scope(&whole_host, url),
            "the path-narrowed member is the more specific answer, though its \
             string is shorter"
        );
    }

    #[test]
    fn normalizing_trims_drops_blanks_and_collapses_duplicates() {
        let out = normalized_base_urls(vec![
            "  https://api.binance.com ".to_string(),
            String::new(),
            "   ".to_string(),
            "https://api.binance.com".to_string(),
            "https://fapi.binance.com".to_string(),
        ])
        .expect("every member is a URL with a host");
        assert_eq!(
            out,
            vec![
                "https://api.binance.com".to_string(),
                "https://fapi.binance.com".to_string()
            ],
            "order is kept and the duplicate collapses"
        );
    }

    /// Refused at the write, where the user can act on it. Stored, it could
    /// only ever be refused at the gate, far from the form it was typed into.
    #[test]
    fn normalizing_refuses_a_value_that_is_not_a_url_with_a_host() {
        for bad in ["api.example.com", "not a url", "https://"] {
            let err = normalized_base_urls(vec![bad.to_string()])
                .expect_err("a scope with no host is refused");
            assert!(err.contains("host"), "unhelpful message: {err}");
        }
    }

    // -----------------------------------------------------------------------
    // 20260805134838_drop_credential_name_prefixes_use_auth_type.sql
    // -----------------------------------------------------------------------
    //
    // `setup_test_db` runs the full migration chain, so the schema these tests
    // start from is already post-migration. They therefore rebuild the
    // PRE-migration shape (restore the old single unique constraint, drop the
    // new pair, re-insert prefixed rows) and re-run the shipped file over it.
    //
    // The tests this replaced drove `20260805085054_normalize_oauth_client_...`,
    // which put those prefixes ON. That file stays on disk as applied history
    // (sqlx refuses to run when a previously-applied version is missing from the
    // resolved set), but re-running it now would re-prefix rows against a schema
    // that no longer wants them, so its tests are gone rather than kept.

    /// The shipped migration itself, not a transcription of it. `include_str!`
    /// means there is no second copy that can drift, and Postgres ignores the
    /// `--` comment header, so the file executes as-is.
    const DROP_NAME_PREFIXES: &str = include_str!(
        "../../migrations/20260805134838_drop_credential_name_prefixes_use_auth_type.sql"
    );

    /// Put the table back the way it looked before the migration under test:
    /// one unique constraint on `service_name` alone.
    async fn restore_pre_migration_schema(pool: &PgPool) {
        for stmt in [
            "DROP INDEX IF EXISTS credentials_service_name_not_oauth_key",
            "ALTER TABLE credentials DROP CONSTRAINT IF EXISTS credentials_service_name_auth_type_key",
            "ALTER TABLE credentials ADD CONSTRAINT credentials_service_name_key UNIQUE (service_name)",
        ] {
            sqlx::query(stmt).execute(pool).await.expect(stmt);
        }
    }

    async fn insert_raw(pool: &PgPool, service_name: &str, auth_type: &str) {
        // The schema this migration ran on had no sealed-secret CHECK.
        sqlx::query(
            "ALTER TABLE credentials DROP CONSTRAINT IF EXISTS credentials_auth_value_sealed",
        )
        .execute(pool)
        .await
        .expect("drop the sealed CHECK");
        sqlx::query(
            "INSERT INTO credentials (service_name, base_urls, auth_type, auth_value) \
             VALUES ($1, ARRAY['https://api.example.com'], $2, '{}')",
        )
        .bind(service_name)
        .bind(auth_type)
        .execute(pool)
        .await
        .expect("insert credential");
    }

    /// The engine seals the rows an old migration left plain at its next boot.
    async fn seal_as_boot_would(pool: &PgPool) {
        let key = crate::core::secret_store::key().unwrap();
        crate::core::secret_store::boot::seal_plain_values(pool, key)
            .await
            .unwrap();
    }

    async fn names(pool: &PgPool) -> Vec<String> {
        sqlx::query_scalar("SELECT service_name FROM credentials ORDER BY service_name")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    /// The point of the change: both namespaces collapse to the bare name,
    /// because `auth_type` already carries what the prefix was spelling out.
    #[tokio::test]
    async fn migration_strips_both_name_prefixes() {
        let (pool, db) = setup_test_db().await;
        restore_pre_migration_schema(&pool).await;
        insert_raw(&pool, "oauth:dropbox", "oauth_client").await;
        insert_raw(&pool, "OAuth:Google", "oauth_client").await;
        insert_raw(&pool, "email:work", "email_password").await;

        sqlx::raw_sql(DROP_NAME_PREFIXES)
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(names(&pool).await, vec!["dropbox", "google", "work"]);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// The reachability invariant, and the reason the old constraint is dropped
    /// BEFORE the strip: an app registration is now ALLOWED to shadow a
    /// same-named API key, which the old `UNIQUE (service_name)` forbade. Under
    /// the old ordering this row would have been skipped as "target taken" and
    /// left stranded, which is the failure the whole change exists to end.
    #[tokio::test]
    async fn migration_strips_even_when_a_same_named_api_key_exists() {
        let (pool, db) = setup_test_db().await;
        restore_pre_migration_schema(&pool).await;
        insert_raw(&pool, "google", "api_key").await;
        insert_raw(&pool, "oauth:google", "oauth_client").await;

        sqlx::raw_sql(DROP_NAME_PREFIXES)
            .execute(&pool)
            .await
            .unwrap();

        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT service_name, auth_type FROM credentials ORDER BY auth_type")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![
                ("google".to_string(), "api_key".to_string()),
                ("google".to_string(), "oauth_client".to_string()),
            ],
            "the registration sheds its prefix and shadows the key"
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// `email_password` lives under the GLOBALLY-unique arm, not the shadowing
    /// one, so it cannot take a name another non-oauth credential holds. The row
    /// keeps its prefix rather than the migration aborting and blocking startup;
    /// `get_email_password` still resolves it (a registered temporary measure).
    #[tokio::test]
    async fn migration_strands_an_email_row_whose_bare_name_is_taken() {
        let (pool, db) = setup_test_db().await;
        restore_pre_migration_schema(&pool).await;
        insert_raw(&pool, "work", "api_key").await;
        insert_raw(&pool, "email:work", "email_password").await;

        sqlx::raw_sql(DROP_NAME_PREFIXES)
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(names(&pool).await, vec!["email:work", "work"]);

        seal_as_boot_would(&pool).await;
        let stranded = CredentialStore::get_email_password(&pool, "work")
            .await
            .unwrap()
            .expect("the stranded row is still reachable");
        assert_eq!(stranded.auth_type, AuthType::EmailPassword);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// An OAuth client registration must never be picked as the auth for an
    /// outbound HTTP call. Its `auth_value` is a `{client_id, client_secret}`
    /// JSON blob, so injecting it could only leak the secret and fail the call,
    /// and its `base_url` is the provider's API host, which is exactly what a
    /// request to that provider matches. The tie-break is longest-`base_url`
    /// first, so with an API key beside it the winner was arbitrary.
    #[tokio::test]
    async fn find_by_url_never_returns_an_oauth_client() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        for (name, auth_type, value) in [
            ("acme", AuthType::OauthClient, "{\"client_id\":\"cid\"}"),
            ("acme", AuthType::ApiKey, "the-real-key"),
        ] {
            CredentialStore::upsert(
                &pool,
                &bus,
                name,
                &["https://api.acme.test".to_string()],
                auth_type,
                value,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        }

        let found = CredentialStore::find_by_url(&pool, "https://api.acme.test/v1/things")
            .await
            .unwrap()
            .expect("the API key still matches");
        assert_eq!(found.stored_value(), "the-real-key");

        teardown_test_db(&db).await;
    }

    /// And an OAuth registration alone matches nothing, rather than being
    /// offered as a fallback auth for the host.
    #[tokio::test]
    async fn find_by_url_finds_nothing_when_only_an_oauth_client_matches() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        CredentialStore::upsert(
            &pool,
            &bus,
            "acme",
            &["https://api.acme.test".to_string()],
            AuthType::OauthClient,
            "{\"client_id\":\"cid\"}",
            None,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(
            CredentialStore::find_by_url(&pool, "https://api.acme.test/v1")
                .await
                .unwrap()
                .is_none()
        );

        teardown_test_db(&db).await;
    }

    /// The narrowest scope wins, measured on the member that matched. A wide
    /// credential carrying many hosts must not out-rank a narrow one just for
    /// holding a longer set.
    #[tokio::test]
    async fn find_by_url_ranks_on_the_matching_scope_not_the_widest_row() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        for (name, scopes, value) in [
            (
                "wide",
                vec![
                    "https://api.acme.test".to_string(),
                    "https://other.acme.test/a/very/long/path".to_string(),
                ],
                "wide-key",
            ),
            (
                "narrow",
                vec!["https://api.acme.test/v1".to_string()],
                "narrow-key",
            ),
        ] {
            CredentialStore::upsert(
                &pool,
                &bus,
                name,
                &scopes,
                AuthType::ApiKey,
                value,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        }

        let found = CredentialStore::find_by_url(&pool, "https://api.acme.test/v1/things")
            .await
            .unwrap()
            .expect("both scope it, so one must answer");
        assert_eq!(found.stored_value(), "narrow-key");

        let outside = CredentialStore::find_by_url(&pool, "https://api.acme.test/v2/things")
            .await
            .unwrap()
            .expect("only the wide credential reaches here");
        assert_eq!(outside.stored_value(), "wide-key");

        teardown_test_db(&db).await;
    }

    /// The inversion this migration must not cause. Both `oauth:dropbox` and a
    /// bare `dropbox` exist as `oauth_client`, which is the shape the 2026-08-05
    /// incident produced and the previous migration deliberately left alone.
    ///
    /// Before this migration every OAuth read resolves `oauth:<provider>`, so
    /// the PREFIXED row is the live registration and the bare one is unreachable
    /// by every code path. Simply stranding the pair would hand the bare name to
    /// the dead row and leave the live one prefixed, so `get_oauth_client` would
    /// start reading the duplicate and the working connection would break on its
    /// next refresh. The live row must end up at the bare name.
    #[tokio::test]
    async fn migration_promotes_the_live_prefixed_row_over_a_dead_bare_duplicate() {
        let (pool, db) = setup_test_db().await;
        restore_pre_migration_schema(&pool).await;
        insert_raw(&pool, "dropbox", "oauth_client").await;
        // Distinguishable payload, so the assert proves WHICH row won rather
        // than just that some row holds the name.
        sqlx::query(
            "UPDATE credentials SET auth_value = '{\"client_id\":\"dead\"}' WHERE service_name = 'dropbox'",
        )
        .execute(&pool)
        .await
        .unwrap();
        insert_raw(&pool, "oauth:dropbox", "oauth_client").await;
        sqlx::query(
            "UPDATE credentials SET auth_value = '{\"client_id\":\"live\"}' WHERE service_name = 'oauth:dropbox'",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(DROP_NAME_PREFIXES)
            .execute(&pool)
            .await
            .unwrap();

        seal_as_boot_would(&pool).await;
        let live = CredentialStore::get_oauth_client(&pool, "dropbox")
            .await
            .unwrap()
            .expect("the provider still resolves");
        assert_eq!(
            live.stored_value(),
            "{\"client_id\":\"live\"}",
            "the row every pre-migration read resolved must be the one that keeps the provider"
        );
        // The duplicate is renamed aside, not deleted: a migration must not
        // destroy a secret the user typed.
        assert_eq!(
            names(&pool).await,
            vec!["dropbox", "dropbox (unreachable duplicate)"]
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// The archival rename must never be skipped. An earlier draft bailed when
    /// `<name> (unreachable duplicate)` was already taken, which left the live
    /// row prefixed and put the engine right back to reading the dead
    /// registration: the exact inversion the step above exists to prevent. An
    /// occupied archival name falls back to a primary-key-suffixed one instead.
    #[tokio::test]
    async fn migration_promotes_the_live_row_even_when_the_archival_name_is_taken() {
        let (pool, db) = setup_test_db().await;
        restore_pre_migration_schema(&pool).await;
        insert_raw(&pool, "dropbox", "oauth_client").await;
        insert_raw(&pool, "oauth:dropbox", "oauth_client").await;
        // An unrelated credential the user happened to name that.
        insert_raw(&pool, "dropbox (unreachable duplicate)", "api_key").await;

        sqlx::raw_sql(DROP_NAME_PREFIXES)
            .execute(&pool)
            .await
            .unwrap();

        seal_as_boot_would(&pool).await;
        let live = CredentialStore::get_oauth_client(&pool, "dropbox")
            .await
            .unwrap()
            .expect("the provider must still resolve");
        assert_eq!(live.service_name, "dropbox");
        // Three rows still, none lost: the live one, the user's unrelated one,
        // and the dead duplicate under a key-suffixed name.
        let names = names(&pool).await;
        assert_eq!(names.len(), 3, "no row may be dropped: {names:?}");
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("dropbox (unreachable duplicate ")),
            "the dead row takes a key-suffixed archival name: {names:?}"
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// The same promotion for the other namespace, so the two behave alike.
    /// A bare `work` of ANOTHER type is a different credential and still blocks
    /// (see `migration_strands_an_email_row_whose_bare_name_is_taken`); a bare
    /// `work` that is itself an `email_password` is the dead duplicate.
    #[tokio::test]
    async fn migration_promotes_a_live_prefixed_email_row_over_a_dead_bare_duplicate() {
        let (pool, db) = setup_test_db().await;
        restore_pre_migration_schema(&pool).await;
        insert_raw(&pool, "work", "email_password").await;
        insert_raw(&pool, "email:work", "email_password").await;

        sqlx::raw_sql(DROP_NAME_PREFIXES)
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            names(&pool).await,
            vec!["work", "work (unreachable duplicate)"]
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// Two case variants both strip to `dropbox`. The new composite constraint
    /// is added at the end of the same migration, so moving BOTH would violate
    /// it, fail the migration, and block engine startup. Exactly one may move.
    #[tokio::test]
    async fn migration_moves_only_one_of_two_colliding_case_variants() {
        let (pool, db) = setup_test_db().await;
        restore_pre_migration_schema(&pool).await;
        insert_raw(&pool, "oauth:Dropbox", "oauth_client").await;
        insert_raw(&pool, "oauth:dropbox", "oauth_client").await;

        sqlx::raw_sql(DROP_NAME_PREFIXES)
            .execute(&pool)
            .await
            .expect("must not violate the new composite constraint");

        let names = names(&pool).await;
        assert!(
            names.contains(&"dropbox".to_string()),
            "one variant must be freed: {names:?}"
        );
        assert_eq!(names.len(), 2, "the other is left intact: {names:?}");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// Scoped to the two namespaced types. Every other name survives byte for
    /// byte, because that name is what `CRED_<NAME>` injection and `apis.json`
    /// service lookups resolve: renaming one would break a live script. The
    /// `oauth-ish-name` row is the trap, an `api_key` that merely reads like a
    /// namespaced one.
    #[tokio::test]
    async fn migration_leaves_every_other_auth_type_alone() {
        let (pool, db) = setup_test_db().await;
        restore_pre_migration_schema(&pool).await;
        insert_raw(&pool, "dropbox", "api_key").await;
        insert_raw(&pool, "github", "bearer").await;
        insert_raw(&pool, "oauth:not-a-client", "api_key").await;

        sqlx::raw_sql(DROP_NAME_PREFIXES)
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            names(&pool).await,
            vec!["dropbox", "github", "oauth:not-a-client"]
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// The partial index is what keeps `CRED_<NAME>` unambiguous: only
    /// `oauth_client` may shadow a name, so two injectable credentials can never
    /// contend for one variable.
    #[tokio::test]
    async fn partial_index_rejects_a_second_non_oauth_row_with_the_same_name() {
        let (pool, db) = setup_test_db().await;
        insert_raw(&pool, "shared", "api_key").await;

        let err = sqlx::query(
            "INSERT INTO credentials (service_name, base_urls, auth_type, auth_value) \
             VALUES ('shared', ARRAY['https://api.example.com'], 'bearer', '{}')",
        )
        .execute(&pool)
        .await
        .expect_err("a second injectable credential may not take the name");
        assert!(
            err.as_database_error().and_then(|e| e.code()).as_deref() == Some("23505"),
            "expected a unique violation, got {err}"
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    // -----------------------------------------------------------------------
    // 20260805104126_backfill_dropbox_offline_authorize_params.sql
    // -----------------------------------------------------------------------

    /// Shipped file, not a transcription, for the same reason as above.
    const BACKFILL_DROPBOX_OFFLINE: &str = include_str!(
        "../../migrations/20260805104126_backfill_dropbox_offline_authorize_params.sql"
    );

    async fn insert_client(pool: &PgPool, service_name: &str, auth_value: &str) {
        // The schema this migration ran on had no sealed-secret CHECK.
        sqlx::query(
            "ALTER TABLE credentials DROP CONSTRAINT IF EXISTS credentials_auth_value_sealed",
        )
        .execute(pool)
        .await
        .expect("drop the sealed CHECK");
        sqlx::query(
            "INSERT INTO credentials (service_name, base_urls, auth_type, auth_value) \
             VALUES ($1, ARRAY['https://api.example.com'], 'oauth_client', $2)",
        )
        .bind(service_name)
        .bind(auth_value)
        .execute(pool)
        .await
        .expect("insert credential");
    }

    async fn authorize_params(pool: &PgPool, service_name: &str) -> Option<String> {
        let raw: String =
            sqlx::query_scalar("SELECT auth_value FROM credentials WHERE service_name = $1")
                .bind(service_name)
                .fetch_one(pool)
                .await
                .unwrap();
        serde_json::from_str::<serde_json::Value>(&raw)
            .ok()?
            .get("authorize_params")?
            .as_str()
            .map(str::to_string)
    }

    /// The row this migration exists for: a Dropbox client connected before the
    /// field existed. Without the backfill it falls back to Google's spelling,
    /// so even the *Grant access* reconnect built to rescue these users yields a
    /// token with no refresh token behind it.
    #[tokio::test]
    async fn backfill_gives_an_existing_dropbox_client_offline_access() {
        let (pool, db) = setup_test_db().await;
        insert_client(
            &pool,
            "oauth:dropbox",
            r#"{"client_id":"abc","auth_url":"https://www.dropbox.com/oauth2/authorize"}"#,
        )
        .await;

        sqlx::query(BACKFILL_DROPBOX_OFFLINE)
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            authorize_params(&pool, "oauth:dropbox").await.as_deref(),
            Some("token_access_type=offline")
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// Keyed on the endpoint, not the credential name: a dedicated Dropbox
    /// connection is a user-chosen alias, and it needs the parameter just as
    /// much.
    #[tokio::test]
    async fn backfill_matches_the_endpoint_not_the_name() {
        let (pool, db) = setup_test_db().await;
        insert_client(
            &pool,
            "oauth:dropbox2",
            r#"{"client_id":"abc","auth_url":"https://www.dropbox.com/oauth2/authorize"}"#,
        )
        .await;

        sqlx::query(BACKFILL_DROPBOX_OFFLINE)
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            authorize_params(&pool, "oauth:dropbox2").await.as_deref(),
            Some("token_access_type=offline")
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// Another provider's client is untouched: its default is correct, and
    /// Google needs `access_type=offline&prompt=consent` rather than Dropbox's
    /// spelling.
    #[tokio::test]
    async fn backfill_leaves_other_providers_alone() {
        let (pool, db) = setup_test_db().await;
        insert_client(
            &pool,
            "oauth:google",
            r#"{"client_id":"abc","auth_url":"https://accounts.google.com/o/oauth2/v2/auth"}"#,
        )
        .await;

        sqlx::query(BACKFILL_DROPBOX_OFFLINE)
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(authorize_params(&pool, "oauth:google").await, None);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// A deliberate value stands, including the `none` opt-out. Re-running must
    /// therefore be a no-op, which is also what makes the migration idempotent.
    #[tokio::test]
    async fn backfill_never_overwrites_a_chosen_value() {
        let (pool, db) = setup_test_db().await;
        insert_client(
            &pool,
            "oauth:dropbox",
            r#"{"client_id":"abc","auth_url":"https://www.dropbox.com/oauth2/authorize","authorize_params":"none"}"#,
        )
        .await;

        for _ in 0..2 {
            sqlx::query(BACKFILL_DROPBOX_OFFLINE)
                .execute(&pool)
                .await
                .unwrap();
        }

        assert_eq!(
            authorize_params(&pool, "oauth:dropbox").await.as_deref(),
            Some("none")
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// A row whose value is not a JSON object must not abort the migration: an
    /// error here fails startup, which is the failure mode the sibling
    /// migration had to be fixed for.
    #[tokio::test]
    async fn backfill_survives_a_credential_that_is_not_json() {
        let (pool, db) = setup_test_db().await;
        insert_client(&pool, "oauth:broken", "not json at all").await;
        insert_client(
            &pool,
            "oauth:dropbox",
            r#"{"client_id":"abc","auth_url":"https://www.dropbox.com/oauth2/authorize"}"#,
        )
        .await;

        sqlx::query(BACKFILL_DROPBOX_OFFLINE)
            .execute(&pool)
            .await
            .expect("a malformed row must not take the migration down");

        assert_eq!(
            authorize_params(&pool, "oauth:dropbox").await.as_deref(),
            Some("token_access_type=offline")
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    // -----------------------------------------------------------------------
    // 20260930204255_strip_oauth_client_default_scopes.sql
    // -----------------------------------------------------------------------

    /// The shipped file, so there is no second copy that can drift.
    const STRIP_CLIENT_SCOPES: &str =
        include_str!("../../migrations/20260930204255_strip_oauth_client_default_scopes.sql");

    async fn auth_value_json(pool: &PgPool, service_name: &str) -> serde_json::Value {
        let raw: String =
            sqlx::query_scalar("SELECT auth_value FROM credentials WHERE service_name = $1")
                .bind(service_name)
                .fetch_one(pool)
                .await
                .unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    /// The key goes, and every other key of the client stays as it was.
    #[tokio::test]
    async fn strip_removes_only_the_scopes_key() {
        let (pool, db) = setup_test_db().await;
        insert_client(
            &pool,
            "google",
            r#"{"client_id":"abc","client_secret":"s","auth_url":"https://a.test/auth","token_url":"https://a.test/token","scopes":"read write"}"#,
        )
        .await;

        for _ in 0..2 {
            sqlx::query(STRIP_CLIENT_SCOPES)
                .execute(&pool)
                .await
                .unwrap();
        }

        assert_eq!(
            auth_value_json(&pool, "google").await,
            serde_json::json!({
                "client_id": "abc",
                "client_secret": "s",
                "auth_url": "https://a.test/auth",
                "token_url": "https://a.test/token",
            })
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// Only rows carrying the key are rewritten. The rest stay byte for byte,
    /// and a row that is not JSON cannot abort the migration and startup.
    #[tokio::test]
    async fn strip_leaves_other_rows_alone() {
        let (pool, db) = setup_test_db().await;
        insert_client(&pool, "broken", "not json at all").await;
        let clean_client = r#"{"client_id":"abc", "token_url":"https://a.test/token"}"#;
        insert_client(&pool, "clean", clean_client).await;
        let api_key_value = r#"{"key":"k","scopes":"kept"}"#;
        sqlx::query(
            "INSERT INTO credentials (service_name, base_urls, auth_type, auth_value) \
             VALUES ('other', ARRAY['https://api.other.test'], 'api_key', $1)",
        )
        .bind(api_key_value)
        .execute(&pool)
        .await
        .expect("insert api key");

        sqlx::query(STRIP_CLIENT_SCOPES)
            .execute(&pool)
            .await
            .expect("a malformed row must not take the migration down");

        let other: String =
            sqlx::query_scalar("SELECT auth_value FROM credentials WHERE service_name = 'other'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(other, api_key_value);
        let clean: (String, bool) = sqlx::query_as(
            "SELECT auth_value, updated_at = created_at FROM credentials WHERE service_name = 'clean'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(clean, (clean_client.to_string(), true));

        pool.close().await;
        teardown_test_db(&db).await;
    }

    // -----------------------------------------------------------------------
    // 20260829132711_credential_scope_is_a_set.sql
    // -----------------------------------------------------------------------

    /// The shipped file, so there is no second copy that can drift.
    const SCOPE_IS_A_SET: &str =
        include_str!("../../migrations/20260829132711_credential_scope_is_a_set.sql");

    /// Put the column back the way it looked before: one `base_url TEXT`.
    async fn restore_single_scope_column(pool: &PgPool) {
        for stmt in [
            "ALTER TABLE credentials DROP COLUMN base_urls",
            "ALTER TABLE credentials ADD COLUMN base_url TEXT NOT NULL DEFAULT ''",
        ] {
            sqlx::query(stmt).execute(pool).await.expect(stmt);
        }
    }

    async fn insert_with_single_scope(pool: &PgPool, service_name: &str, base_url: &str) {
        // The schema this migration ran on had no sealed-secret CHECK.
        sqlx::query(
            "ALTER TABLE credentials DROP CONSTRAINT IF EXISTS credentials_auth_value_sealed",
        )
        .execute(pool)
        .await
        .expect("drop the sealed CHECK");
        sqlx::query(
            "INSERT INTO credentials (service_name, base_url, auth_type, auth_value) \
             VALUES ($1, $2, 'api_key', 'k')",
        )
        .bind(service_name)
        .bind(base_url)
        .execute(pool)
        .await
        .expect("insert credential");
    }

    async fn scopes_of(pool: &PgPool, service_name: &str) -> Vec<String> {
        sqlx::query_scalar("SELECT base_urls FROM credentials WHERE service_name = $1")
            .bind(service_name)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// The load-bearing property: the upgrade is lossless and widens nothing.
    /// A scoped row keeps exactly its own host, and a blank one stays refused
    /// everywhere rather than becoming a credential that goes anywhere.
    #[tokio::test]
    async fn migration_carries_one_scope_across_and_invents_none() {
        let (pool, db) = setup_test_db().await;
        restore_single_scope_column(&pool).await;
        insert_with_single_scope(&pool, "binance", "https://api.binance.com").await;
        insert_with_single_scope(&pool, "webhook-secret", "").await;
        insert_with_single_scope(&pool, "padded", "  https://api.example.com  ").await;

        sqlx::raw_sql(SCOPE_IS_A_SET).execute(&pool).await.unwrap();

        assert_eq!(
            scopes_of(&pool, "binance").await,
            vec!["https://api.binance.com".to_string()]
        );
        assert!(
            scopes_of(&pool, "webhook-secret").await.is_empty(),
            "a blank scope must not become a scope that covers something"
        );
        assert_eq!(
            scopes_of(&pool, "padded").await,
            vec!["https://api.example.com".to_string()],
            "the carried value is trimmed, matching what the write path stores"
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// A reference no other test shares, since the engine's cache is global.
    fn unique_reference() -> String {
        format!(
            "op://Engineering/item-{}/credential",
            Uuid::new_v4().simple()
        )
    }

    fn not_an_item() -> crate::core::secret_reference::ResolveFailure {
        crate::core::secret_reference::ResolveFailure::Definitive(
            "op said: \"Stripe\" isn't an item".to_string(),
        )
    }

    /// The *secret reference* half of ADR 0516: `op` runs only for a
    /// credential the subprocess names.
    #[tokio::test]
    async fn a_reference_resolves_only_when_its_credential_is_named() {
        let creds = || {
            vec![
                make_cred(
                    "stripe",
                    AuthType::ApiKey,
                    "op://Engineering/Stripe/credential",
                ),
                make_cred("github", AuthType::Bearer, "gh-value"),
            ]
        };
        let resolver = ScriptedResolver::answering(vec![Ok("sk-from-vault".into())]);

        let named = named_credential_env_vars(
            creds(),
            &SecretMentions::scan("echo $CRED_GITHUB"),
            &resolver,
        )
        .await;
        assert_eq!(resolver.calls(), 0, "naming another credential runs no op");
        assert_eq!(
            named.env_vars,
            vec![("CRED_GITHUB".into(), "gh-value".into())]
        );

        let named = named_credential_env_vars(
            creds(),
            &SecretMentions::scan("echo $CRED_STRIPE"),
            &resolver,
        )
        .await;
        assert_eq!(resolver.calls(), 1);
        assert_eq!(
            named.env_vars,
            vec![("CRED_STRIPE".into(), "sk-from-vault".into())]
        );
    }

    /// One unreadable reference costs its own variables and a note. The other
    /// named credentials still load, and the name is not called unknown.
    #[tokio::test]
    async fn a_named_reference_that_cannot_be_read_is_left_out_and_the_rest_load() {
        let creds = vec![
            make_cred(
                "stripe",
                AuthType::ApiKey,
                "op://Engineering/Stripe/credential",
            ),
            make_cred("github", AuthType::Bearer, "gh-value"),
        ];
        let resolver = ScriptedResolver::answering(vec![Err(not_an_item())]);
        let mentions = SecretMentions::scan("echo $CRED_STRIPE $CRED_GITHUB");
        let named = named_credential_env_vars(creds, &mentions, &resolver).await;

        assert_eq!(
            named.env_vars,
            vec![("CRED_GITHUB".into(), "gh-value".into())]
        );
        let note = named
            .unavailable_note()
            .expect("the left-out credential gets a note");
        assert!(
            note.contains("'stripe'") && note.contains("isn't an item"),
            "{note}"
        );
        assert!(secret_names_note(&mentions, &named.known_names).is_none());
    }

    #[tokio::test]
    async fn a_password_reference_resolves_the_password_and_keeps_the_username() {
        let cred = make_cred(
            "mail",
            AuthType::Password,
            r#"{"username":"alice","password":"op://Personal/Mail/password"}"#,
        );
        let resolver = ScriptedResolver::answering(vec![Ok("s3cret".into())]);
        let env: std::collections::HashMap<_, _> = credential_env_vars_for(cred, &resolver)
            .await
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(
            env.get("CRED_MAIL_USERNAME").map(String::as_str),
            Some("alice")
        );
        assert_eq!(
            env.get("CRED_MAIL_PASSWORD").map(String::as_str),
            Some("s3cret")
        );
    }

    /// The token that reads 1Password reaches only the `op` child.
    #[test]
    fn a_service_account_is_never_injected_even_when_named() {
        let mut token = make_cred(
            "onepassword",
            AuthType::OnepasswordServiceAccount,
            "ops_FAKETOKEN",
        );
        token.env_var_name = Some("OP_TOKEN".to_string());
        let named = named_for(
            vec![token],
            &SecretMentions::scan("echo $CRED_ONEPASSWORD $OP_TOKEN"),
        );
        assert!(named.env_vars.is_empty());
        assert!(named.known_names.is_empty());
        assert_eq!(
            credential_env_var_names(
                "onepassword",
                AuthType::OnepasswordServiceAccount,
                Some("OP_TOKEN")
            ),
            Vec::<String>::new()
        );
        assert!(!AuthType::OnepasswordServiceAccount.agent_requestable());
    }

    #[tokio::test]
    async fn name_and_url_lookups_never_return_a_service_account() {
        let (pool, db) = setup_test_db().await;
        crate::test_support::seed_credential(
            &pool,
            "onepassword",
            "https://my.1password.com",
            AuthType::OnepasswordServiceAccount,
            "ops_FAKETOKEN",
        )
        .await;

        assert!(CredentialStore::get(&pool, "onepassword")
            .await
            .unwrap()
            .is_none());
        assert!(
            CredentialStore::find_by_url(&pool, "https://my.1password.com/api")
                .await
                .unwrap()
                .is_none()
        );
        let account = CredentialStore::onepassword_service_account(&pool)
            .await
            .unwrap()
            .expect("the op resolver still finds it");
        assert_eq!(account.token, "ops_FAKETOKEN");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// A resolved value lives only in memory: the row keeps the reference,
    /// and no event or sealed column holds the value.
    #[tokio::test]
    async fn a_resolved_value_is_never_persisted() {
        const VALUE: &str = "resolved-value-never-stored";
        let (pool, db) = setup_test_db().await;
        let (_dir, cli) =
            crate::core::secret_reference::op::tests::fake_op(&format!("printf '{VALUE}'"));
        let account = serde_json::json!({ "token": "ops_FAKETOKEN", "cli_path": cli });
        crate::test_support::seed_credential(
            &pool,
            "onepassword",
            "",
            AuthType::OnepasswordServiceAccount,
            &account.to_string(),
        )
        .await;
        let reference = unique_reference();
        crate::test_support::seed_credential(
            &pool,
            "stripe",
            "https://api.stripe.com",
            AuthType::ApiKey,
            &reference,
        )
        .await;

        let stripe = CredentialStore::get(&pool, "stripe")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stripe.secret(&pool).await.unwrap(), VALUE);
        assert_eq!(
            stripe.stored_value(),
            reference,
            "the row keeps the reference"
        );
        let listed = CredentialStore::list(&pool).await.unwrap();
        assert!(listed
            .iter()
            .any(|c| c.service_name == "stripe" && c.secret_reference));

        let in_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE payload::text LIKE '%' || $1 || '%'",
        )
        .bind(VALUE)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(in_events, 0, "an event holds the value");
        for stored in CredentialStore::list_all_with_secrets(&pool).await.unwrap() {
            assert!(
                !stored.stored_value().contains(VALUE),
                "{} holds it",
                stored.service_name
            );
        }

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// Save resolves a reference once, so a typo never lands.
    #[tokio::test]
    async fn a_reference_that_does_not_resolve_is_refused_on_save() {
        let (pool, db) = setup_test_db().await;
        let refused = check_secret_references(&pool, AuthType::ApiKey, &unique_reference())
            .await
            .unwrap_err();
        assert!(refused.contains("service account"), "{refused}");
        assert!(
            check_secret_references(&pool, AuthType::ApiKey, "op://only-a-vault")
                .await
                .is_err()
        );
        assert!(check_secret_references(&pool, AuthType::ApiKey, "sk-plain")
            .await
            .is_ok());
        assert!(check_secret_references(
            &pool,
            AuthType::OnepasswordServiceAccount,
            &unique_reference()
        )
        .await
        .is_err());

        pool.close().await;
        teardown_test_db(&db).await;
    }
}
