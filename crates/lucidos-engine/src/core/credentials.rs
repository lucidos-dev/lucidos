use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::fmt;
use uuid::Uuid;

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
            _ => Self::Unknown,
        }
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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Full credential including the secret value
#[derive(Clone)]
pub struct Credential {
    pub id: Uuid,
    pub service_name: String,
    /// See [`CredentialInfo::base_urls`]. Matched by
    /// [`credential_scope_covers`], which asks whether ANY member covers a URL.
    pub base_urls: Vec<String>,
    pub auth_type: AuthType,
    pub auth_value: String,
    pub auth_header: String,
    /// Optional custom env var name for the injected secret (e.g. `GITHUB_TOKEN`
    /// instead of the default `CRED_<NAME>`). `None` = default `CRED_` form.
    pub env_var_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// Manual `Debug` so the secret `auth_value` (API key, bearer token, password,
// or the oauth client_secret JSON blob) never leaks through `{:?}`. Everything
// else stays visible so the struct is still useful for debugging.
impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("id", &self.id)
            .field("service_name", &self.service_name)
            .field("base_urls", &self.base_urls)
            .field("auth_type", &self.auth_type)
            .field("auth_value", &"<redacted>")
            .field("auth_header", &self.auth_header)
            .field("env_var_name", &self.env_var_name)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// Full credential DB row tuple (with secret). `env_var_name` is the trailing
/// nullable column added by `20260618124350_add_env_var_name_to_credentials`.
type CredentialRow = (
    Uuid,
    String,
    Vec<String>,
    String,
    String,
    String,
    DateTime<Utc>,
    DateTime<Utc>,
    Option<String>,
);

/// Columns selected for a full credential (with secret), in `CredentialRow` order.
const CREDENTIAL_COLUMNS: &str =
    "id, service_name, base_urls, auth_type, auth_value, auth_header, created_at, updated_at, env_var_name";

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
        auth_value,
        auth_header,
        env_var_name,
        created_at,
        updated_at,
    }
}

/// Credential info DB row tuple (without secret).
type CredentialInfoRow = (
    Uuid,
    String,
    Vec<String>,
    String,
    String,
    DateTime<Utc>,
    DateTime<Utc>,
    Option<String>,
);

/// Parse a credential info row tuple from the database (without secret).
fn parse_credential_info(row: CredentialInfoRow) -> CredentialInfo {
    let (id, service_name, base_urls, auth_type, auth_header, created_at, updated_at, env_var_name) =
        row;
    CredentialInfo {
        id,
        service_name,
        base_urls,
        auth_type: AuthType::parse(&auth_type),
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
    /// Defensive double-write — the migration owns this CREATE TABLE
    /// (see `20260517160627_consolidate_init_schema_tables.sql`). Slated
    /// for removal in `harden-init-schema-tables-vs-migrations-pattern-finish`.
    ///
    /// Reachable only as a no-op: `sqlx::migrate!()` runs at construction and
    /// creates the table, and this fires afterwards behind `IF NOT EXISTS`. It
    /// still mirrors the current constraint shape rather than the pre-2026-08-05
    /// `service_name TEXT UNIQUE`, because a dead copy that contradicts the live
    /// schema is exactly the drift that made a prefix necessary in the first
    /// place: the next reader takes it for the contract.
    pub async fn init_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS credentials (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                service_name TEXT NOT NULL,
                base_urls TEXT[] NOT NULL DEFAULT '{}',
                auth_type TEXT NOT NULL,
                auth_value TEXT NOT NULL,
                auth_header TEXT DEFAULT 'Authorization',
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                CONSTRAINT credentials_service_name_auth_type_key UNIQUE (service_name, auth_type)
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS credentials_service_name_not_oauth_key \
             ON credentials (service_name) WHERE auth_type <> 'oauth_client'",
        )
        .execute(pool)
        .await?;

        Ok(())
    }

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
    #[allow(clippy::too_many_arguments)] // one arg per credential column
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
                updated_at = NOW()
            RETURNING id, (xmax = 0) AS created
            "#
        ))
        .bind(service_name)
        .bind(&base_urls)
        .bind(auth_type.to_string())
        .bind(auth_value)
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
    /// Still returns at most one row: the same index keeps a name globally
    /// unique across every other type.
    pub async fn get(pool: &PgPool, service_name: &str) -> Result<Option<Credential>, sqlx::Error> {
        let result = sqlx::query_as::<_, CredentialRow>(&format!(
            "SELECT {CREDENTIAL_COLUMNS} FROM credentials \
             WHERE service_name = $1 AND auth_type <> 'oauth_client'"
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

    /// List all credentials (without secrets)
    pub async fn list(pool: &PgPool) -> Result<Vec<CredentialInfo>, sqlx::Error> {
        let results = sqlx::query_as::<_, CredentialInfoRow>(
            r#"
            SELECT id, service_name, base_urls, auth_type, auth_header, created_at, updated_at, env_var_name
            FROM credentials
            ORDER BY service_name ASC
            "#,
        )
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
        sqlx::query_scalar::<_, String>(
            "DELETE FROM credentials WHERE id = $1 RETURNING service_name",
        )
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
        // a NULL bind leaves the column as it stands. `auth_value` gets two
        // static queries instead, because a NULL there is the value the user
        // just typed. `env_var_name` is always set (NULL clears it back to the
        // `CRED_` default).
        match auth_value {
            Some(value) => {
                sqlx::query_scalar::<_, String>(
                    r#"
                    UPDATE credentials
                    SET base_urls = COALESCE($2::text[], base_urls), auth_type = $3, auth_header = $4, auth_value = $5, env_var_name = $6, updated_at = NOW()
                    WHERE id = $1
                    RETURNING service_name
                    "#,
                )
                .bind(id)
                .bind(&base_urls)
                .bind(auth_type.to_string())
                .bind(auth_header)
                .bind(value)
                .bind(env_var_name)
                .fetch_optional(pool)
                .await
            }
            None => {
                sqlx::query_scalar::<_, String>(
                    r#"
                    UPDATE credentials
                    SET base_urls = COALESCE($2::text[], base_urls), auth_type = $3, auth_header = $4, env_var_name = $5, updated_at = NOW()
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
            "UPDATE credentials SET base_urls = $2, updated_at = NOW() \
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

    /// Give a credential that carries no scope the one its `apis.json` entry
    /// uses, and announce it. Returns whether the row changed.
    ///
    /// Once per credential, at startup, so the proxy's scope check has
    /// something to enforce for a row that predates it (ADR 0144). The
    /// `WHERE` clause is what makes it once: a row that already has a scope is
    /// never rewritten, so this cannot walk a user's own correction back. It
    /// also never APPENDS to a scope set: an entry naming an already-scoped
    /// credential says nothing about whether the user wants both hosts, and
    /// `apis.json` is writable over the API.
    pub async fn infer_scope_if_empty(
        pool: &PgPool,
        event_bus: &EventBus,
        service_name: &str,
        base_url: &str,
    ) -> Result<bool, sqlx::Error> {
        // Through the same speller as every other write, so the documented
        // "one speller" holds. A malformed `apis.json` value is declined here
        // rather than stored as a scope the gate can only ever refuse.
        let inferred = match normalized_base_urls(vec![base_url.to_string()]) {
            Ok(urls) => urls,
            Err(reason) => {
                log!("[Credentials] not scoping '{service_name}': {reason}");
                return Ok(false);
            }
        };
        let [base_url] = inferred.as_slice() else {
            return Ok(false);
        };
        let updated: Option<String> = sqlx::query_scalar(
            "UPDATE credentials SET base_urls = ARRAY[$1], updated_at = NOW() \
             WHERE service_name = $2 AND cardinality(base_urls) = 0 \
             RETURNING service_name",
        )
        .bind(base_url)
        .bind(service_name)
        .fetch_optional(pool)
        .await?;
        if updated.is_none() {
            return Ok(false);
        }
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::CredentialScopeInferred {
                    service_name: service_name.to_string(),
                    base_url: base_url.to_string(),
                    actor: None,
                }),
                "[Credentials] CredentialScopeInferred",
            )
            .await;
        Ok(true)
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
        // is longest-matching-scope-first, so which one won was arbitrary.
        let results = sqlx::query_as::<_, CredentialRow>(&format!(
            "SELECT {CREDENTIAL_COLUMNS} FROM credentials \
             WHERE auth_type <> 'oauth_client' \
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

    let base_path = base.path().trim_end_matches('/');
    if base_path.is_empty() {
        return true;
    }

    let request_path = request.path();
    request_path == base_path
        || request_path
            .strip_prefix(base_path)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Build CRED_* environment variables from a list of credentials.
/// - `password` type: emits `CRED_{NAME}_USERNAME` and `CRED_{NAME}_PASSWORD`
///   from the JSON-encoded auth_value. The custom `env_var_name` does NOT apply
///   to password credentials — the username/password split has no single name.
/// - other (single-value) types (api_key, bearer, basic, …): always emits the
///   canonical `CRED_{NAME}`, AND — when a custom `env_var_name` is set — emits
///   the same secret a SECOND time under that name (e.g. `GITHUB_TOKEN`). The
///   custom name is an ADDITIONAL alias, not a replacement, so existing scripts /
///   knowhow that reference `CRED_{NAME}` keep working while a tool that expects
///   an exact variable name also gets it.
///
/// - `oauth_client` type: **skipped entirely**. See below.
///
/// `{NAME}` is the credential's `service_name` run through
/// [`crate::core::env_var_segment`] (uppercased, every character outside
/// `[A-Z0-9_]` replaced by `_`).
///
/// **An OAuth client registration is never part of the blanket fan-out.** Its
/// `auth_value` is a `{client_id, client_secret, auth_url, ...}` JSON blob that
/// only the OAuth flow consumes, and that flow reads the credentials table
/// directly, so including it broadcast a `client_secret` into the environment of
/// every `run_bash` / `run_python` / scheduled script for no reader. Excluding
/// it is also what keeps `CRED_<NAME>` unambiguous now that `oauth_client` is
/// the one type allowed to shadow a name: the remaining types stay globally
/// unique under the partial index, so two credentials can never contend for one
/// variable.
///
/// That is a rule about the FAN-OUT, not about the credential. A caller holding
/// one credential the user named explicitly (a `script_handshake` layer's
/// `credential` field) calls [`credential_env_vars_for`] instead and does get
/// it: neither the broadcast nor the ambiguity applies to a single named row,
/// and silently injecting nothing there would be a configured secret going
/// missing with no error.
pub fn credential_env_vars(credentials: Vec<Credential>) -> Vec<(String, String)> {
    credentials
        .into_iter()
        .filter(|c| c.auth_type != AuthType::OauthClient)
        .flat_map(credential_env_vars_for)
        .collect()
}

/// The `CRED_*` variables for ONE credential, with no type filtering. See
/// [`credential_env_vars`] for the shape per auth type and for why the
/// list-taking version skips `oauth_client` while this one does not.
pub fn credential_env_vars_for(cred: Credential) -> Vec<(String, String)> {
    let mut env_vars = Vec::new();
    {
        let prefix = format!("CRED_{}", crate::core::env_var_segment(&cred.service_name));

        if cred.auth_type == AuthType::Password {
            match serde_json::from_str::<serde_json::Value>(&cred.auth_value) {
                Ok(parsed) => {
                    let username = parsed["username"].as_str().unwrap_or("");
                    let password = parsed["password"].as_str().unwrap_or("");
                    env_vars.push((format!("{}_USERNAME", prefix), username.to_string()));
                    env_vars.push((format!("{}_PASSWORD", prefix), password.to_string()));
                }
                Err(e) => {
                    // Malformed JSON in a Password credential means a script
                    // expecting CRED_<NAME>_USERNAME / _PASSWORD will see them
                    // missing with no signal — log so the user has something to
                    // diagnose against instead of silently failing in the script.
                    log!(
                        "[Credentials] {} has invalid password JSON, skipping env injection: {}",
                        cred.service_name,
                        e
                    );
                }
            }
        } else {
            // Always emit the canonical CRED_<NAME>. When a custom name is set,
            // ALSO emit the secret under that name (an additional alias) so
            // existing references to CRED_<NAME> keep working.
            let value = cred.auth_value;
            let custom = cred
                .env_var_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != prefix.as_str());
            if let Some(custom) = custom {
                env_vars.push((custom.to_string(), value.clone()));
            }
            env_vars.push((prefix, value));
        }
    }
    env_vars
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

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

    fn make_cred(service_name: &str, auth_type: AuthType, auth_value: &str) -> Credential {
        Credential {
            id: Uuid::nil(),
            service_name: service_name.to_string(),
            base_urls: Vec::new(),
            auth_type,
            auth_value: auth_value.to_string(),
            auth_header: "Authorization".to_string(),
            env_var_name: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn custom_env_var_name_adds_alias_alongside_cred_prefix() {
        let mut cred = make_cred("github", AuthType::Bearer, "ghp_xxx");
        cred.env_var_name = Some("GITHUB_TOKEN".to_string());
        let env: std::collections::HashMap<_, _> =
            credential_env_vars(vec![cred]).into_iter().collect();
        // Additive: the custom name is an EXTRA alias, the canonical CRED_<NAME>
        // is still emitted so existing references keep working.
        assert_eq!(env.get("GITHUB_TOKEN").map(String::as_str), Some("ghp_xxx"));
        assert_eq!(env.get("CRED_GITHUB").map(String::as_str), Some("ghp_xxx"));
    }

    #[test]
    fn blank_custom_env_var_name_emits_only_cred_prefix() {
        let mut cred = make_cred("github", AuthType::Bearer, "ghp_xxx");
        cred.env_var_name = Some("   ".to_string());
        let pairs = credential_env_vars(vec![cred]);
        assert_eq!(pairs.len(), 1, "blank custom name adds no alias");
        assert_eq!(pairs[0].0, "CRED_GITHUB");
        assert_eq!(pairs[0].1, "ghp_xxx");
    }

    #[test]
    fn custom_name_equal_to_prefix_is_not_duplicated() {
        let mut cred = make_cred("github", AuthType::Bearer, "ghp_xxx");
        cred.env_var_name = Some("CRED_GITHUB".to_string());
        let pairs = credential_env_vars(vec![cred]);
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
        let env: std::collections::HashMap<_, _> =
            credential_env_vars(vec![cred]).into_iter().collect();
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
        let env: std::collections::HashMap<_, _> =
            credential_env_vars(vec![cred]).into_iter().collect();
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
        let env: std::collections::HashMap<_, _> =
            credential_env_vars(vec![cred]).into_iter().collect();
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
        let env: std::collections::HashMap<_, _> =
            credential_env_vars(vec![cred]).into_iter().collect();
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
        let env: std::collections::HashMap<_, _> =
            credential_env_vars(vec![cred]).into_iter().collect();
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
        let env: std::collections::HashMap<_, _> =
            credential_env_vars(vec![cred]).into_iter().collect();
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
        let env: std::collections::HashMap<_, _> = credential_env_vars(vec![
            make_cred("oauth:google", AuthType::ApiKey, "client-blob"),
            make_cred("email:user@example.com", AuthType::EmailPassword, "pw"),
        ])
        .into_iter()
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

    #[test]
    fn debug_redacts_credential_secret() {
        let cred = make_cred(
            "oauth:google",
            AuthType::OauthClient,
            r#"{"client_id":"abc","client_secret":"super-secret-value"}"#,
        );
        let dbg = format!("{:?}", cred);
        // The secret `auth_value` must never appear in a `{:?}` rendering.
        assert!(!dbg.contains("super-secret-value"), "secret leaked: {dbg}");
        assert!(
            dbg.contains("auth_value: \"<redacted>\""),
            "expected redacted auth_value: {dbg}"
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
        assert_eq!(found.auth_value, "the-real-key");

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
        assert_eq!(found.auth_value, "narrow-key");

        let outside = CredentialStore::find_by_url(&pool, "https://api.acme.test/v2/things")
            .await
            .unwrap()
            .expect("only the wide credential reaches here");
        assert_eq!(outside.auth_value, "wide-key");

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

        let live = CredentialStore::get_oauth_client(&pool, "dropbox")
            .await
            .unwrap()
            .expect("the provider still resolves");
        assert_eq!(
            live.auth_value, "{\"client_id\":\"live\"}",
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
}
