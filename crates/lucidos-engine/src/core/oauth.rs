use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Full OAuth account row including tokens (internal use only)
#[derive(Clone, sqlx::FromRow)]
pub struct OAuthAccount {
    pub id: Uuid,
    pub provider: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_expiry: Option<DateTime<Utc>>,
    /// What the provider GRANTED. See [`OAuthAccountInfo::desired_scopes`] for
    /// the set that was asked for.
    pub scopes: String,
    pub desired_scopes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// Manual `Debug` so an account never leaks its tokens through `{:?}`. Refresh
// token presence stays visible; the value does not.
impl std::fmt::Debug for OAuthAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthAccount")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("email", &self.email)
            .field("display_name", &self.display_name)
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("token_expiry", &self.token_expiry)
            .field("scopes", &self.scopes)
            .field("desired_scopes", &self.desired_scopes)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// OAuth account info without tokens (safe for API responses)
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct OAuthAccountInfo {
    pub id: Uuid,
    pub provider: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    /// What the provider GRANTED.
    pub scopes: String,
    /// What the account was ASKED for, accumulated across every authorization.
    ///
    /// *Reconnect* re-requests this, never `scopes`. `prepare_oauth_flow`
    /// merges the request with the existing grant, so asking for the granted
    /// set again could not recover a scope the provider had refused.
    ///
    /// `None` for an account connected before the column existed. The caller
    /// falls back to something never narrower than the granted set.
    pub desired_scopes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// OAuth client credential requests
// ---------------------------------------------------------------------------
//
// The engine ships NO hardcoded endpoint table. Known providers, their
// endpoints and their typical scopes live in
// `system-knowhow/oauth-providers.md`, which the agent reads and maintains.
// Whatever endpoints the agent passes here are persisted into the
// per-credential JSON. That JSON is the single source of truth both
// `prepare_oauth_flow` and `refresh_oauth_if_needed` read back.

/// Endpoint and scope values used to pre-fill the credential modal. Whatever
/// is present lands in the request's `defaults` block, which tells the modal to
/// pre-fill (and stop requiring) the endpoint fields. All-`None` means no
/// `defaults` block, so the modal expands its endpoint section for manual entry.
#[derive(Debug, Default, Clone)]
pub struct OAuthClientOverrides {
    pub base_url: Option<String>,
    pub auth_url: Option<String>,
    pub token_url: Option<String>,
    pub userinfo_url: Option<String>,
    /// `"POST"` when the provider's userinfo endpoint refuses GET (Dropbox).
    /// Omit for the OIDC default. See [`UserinfoMethod`].
    pub userinfo_method: Option<String>,
    /// Extra authorization-URL parameters in `key=value&key=value` form, e.g.
    /// the provider's own spelling of "issue a refresh token". Omit to keep
    /// [`DEFAULT_AUTHORIZE_PARAMS`]. See [`AuthorizeParams`].
    pub authorize_params: Option<String>,
    pub scopes: Option<String>,
    /// Loopback callback URI to register with this provider. Omit for the
    /// default (`127.0.0.1`); supply the `localhost` form only for a provider
    /// that won't accept the IP literal. See `default_redirect_uri` and the
    /// oauth-providers knowhow.
    pub redirect_uri: Option<String>,
}

/// Normalize the credential name for a provider's OAuth client registration.
///
/// Lowercased, so `Dropbox` and `dropbox` cannot address two registrations. A
/// leading `oauth:` is stripped because agents and knowhow in the wild still
/// write that spelling, and either form must reach the same row.
pub fn client_provider_name(name: &str) -> String {
    let name = name.trim().to_lowercase();
    // A bare `oauth:` would strip to nothing. Keep the input so the caller's
    // emptiness check rejects it, rather than manufacturing a row named "".
    match name.strip_prefix("oauth:") {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => name,
    }
}

/// Build the credential-request JSON the frontend modal opens for an
/// `oauth_client` flow. Single source of truth for service/prompt/base_url
/// shape — both the LLM tool path and the OAuth re-auth API path call here.
pub fn oauth_client_request(provider: &str, overrides: &OAuthClientOverrides) -> serde_json::Value {
    let base_url = overrides
        .base_url
        .clone()
        .unwrap_or_else(|| format!("https://{provider}.com"));
    let mut request = serde_json::json!({
        "service": client_provider_name(provider),
        "prompt": format!("Enter your OAuth client credentials for {provider}."),
        "base_url": base_url,
        "auth_type": "oauth_client",
    });
    // Only keys the caller supplied go into `defaults`, never present-but-null,
    // so the modal can treat "key absent" as "not pre-filled".
    let mut defaults = serde_json::Map::new();
    for (key, value) in [
        ("auth_url", &overrides.auth_url),
        ("token_url", &overrides.token_url),
        ("userinfo_url", &overrides.userinfo_url),
        ("userinfo_method", &overrides.userinfo_method),
        ("authorize_params", &overrides.authorize_params),
        ("scopes", &overrides.scopes),
        ("redirect_uri", &overrides.redirect_uri),
    ] {
        if let Some(v) = value {
            defaults.insert(key.to_string(), serde_json::Value::String(v.clone()));
        }
    }
    if !defaults.is_empty() {
        request["defaults"] = serde_json::Value::Object(defaults);
    }
    request
}

impl OAuthClientOverrides {
    /// Prefill from an *OAuth provider registry* row, so the credential modal
    /// asks only for the Client ID.
    ///
    /// This is the whole of the registry's authority: it seeds a credential at
    /// write time and never participates in a flow. `prepare_oauth_flow` still
    /// reads endpoints back out of the stored credential, so a credential keeps
    /// fully describing its own authorization.
    ///
    /// `scopes` comes from the caller, never the row: the scope set is a
    /// property of what the connection is FOR, not of the provider.
    pub fn from_registry(row: &crate::core::oauth_registry::OAuthProviderRow) -> Self {
        Self {
            base_url: Some(row.base_url.clone()),
            auth_url: Some(row.auth_url.clone()),
            token_url: Some(row.token_url.clone()),
            userinfo_url: row.userinfo_url.clone(),
            userinfo_method: row.userinfo_method.clone(),
            authorize_params: row.authorize_params.clone(),
            scopes: None,
            redirect_uri: row.redirect_uri.clone(),
        }
    }
}

/// The credential fields [`prepare_oauth_flow`] hard-requires, in the order it
/// reads them. A caller can then tell "this credential cannot drive a flow"
/// from "this flow failed".
const REQUIRED_FLOW_FIELDS: [&str; 3] = ["client_id", "auth_url", "token_url"];

/// Which required fields a stored `oauth_client` secret is missing.
///
/// Empty means the credential can drive a flow. Anything else is the list the
/// repair form reopens for. Without it the user reaches Connect and meets a
/// bare *"Missing auth_url"* toast, one screen away from the cause.
///
/// A secret that is not a JSON object counts as missing everything: there is no
/// recoverable client id inside an unparseable blob.
pub fn missing_flow_fields(auth_value: &str) -> Vec<&'static str> {
    let parsed = serde_json::from_str::<serde_json::Value>(auth_value).ok();
    let present = |key: &str| {
        parsed
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .is_some_and(|s| !s.is_empty())
    };
    REQUIRED_FLOW_FIELDS
        .into_iter()
        .filter(|key| !present(key))
        .collect()
}

/// The credential request that REPAIRS an existing `oauth_client` rather than
/// creating one.
///
/// Two additions the create path has no use for. `existing_credential_id`
/// updates the row rather than adding a second registration for the provider,
/// and `missing` tells the form which fields it reopened for. The stored
/// `client_id` rides along in `defaults` so a repair does not ask for it twice.
pub fn oauth_client_repair_request(
    provider: &str,
    overrides: &OAuthClientOverrides,
    credential_id: Uuid,
    client_id: Option<&str>,
    missing: &[&str],
) -> serde_json::Value {
    let mut request = oauth_client_request(provider, overrides);
    request["existing_credential_id"] = serde_json::json!(credential_id.to_string());
    request["missing"] = serde_json::json!(missing);
    request["prompt"] = serde_json::json!(format!(
        "Finish the OAuth client registration for {provider}. Connecting needs {}.",
        join_human(missing)
    ));
    if let Some(client_id) = client_id.map(str::trim).filter(|s| !s.is_empty()) {
        request["defaults"]["client_id"] = serde_json::json!(client_id);
    }
    request
}

/// "a" / "a and b" / "a, b and c".
fn join_human(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Determine the provider name from an API URL.
pub fn provider_for_url(url: &str) -> Option<&'static str> {
    if url.contains(".googleapis.com") || url.contains("google.com/") {
        Some("google")
    } else if url.contains("graph.microsoft.com") || url.contains("login.microsoftonline.com") {
        Some("microsoft")
    } else if url.contains("api.github.com") {
        Some("github")
    } else if url.contains("dropboxapi.com") || url.contains("dropbox.com") {
        Some("dropbox")
    } else if url.contains("api.spotify.com") || url.contains("accounts.spotify.com") {
        Some("spotify")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// OAuthStore — database operations
// ---------------------------------------------------------------------------

/// Store for managing OAuth accounts in the database.
///
/// **No caller can skip the event.** [`Self::connect`] and [`Self::delete`] are
/// the only reachable mutators of an account's existence; the raw row writes
/// are private to this module. `OAuthAccount{Connected,Deleted}` is what
/// reloads the Settings Accounts list on every device.
///
/// [`Self::update_tokens`] is the one deliberate exception: a token rotation is
/// not a user-visible change (see its doc).
///
/// Same shape as `RepositoryStore`. See `core::announced_surfaces` and
/// `docs/adr/0032-a-state-write-owns-its-announcement.md`.
pub struct OAuthStore;

impl OAuthStore {
    /// Insert or update an OAuth account row (upsert on provider+email).
    /// Uses a separate conflict clause when email is NULL because PostgreSQL
    /// treats NULL != NULL, so `UNIQUE(provider, email)` never fires for NULLs.
    /// A partial unique index `oauth_accounts_provider_no_email` covers that case.
    ///
    /// **Private on purpose**: [`Self::connect`] is the reachable mutator, and
    /// it emits. See the type-level doc.
    #[allow(clippy::too_many_arguments)]
    async fn upsert_row(
        pool: &PgPool,
        provider: &str,
        email: Option<&str>,
        display_name: Option<&str>,
        access_token: &str,
        refresh_token: Option<&str>,
        token_expiry: Option<DateTime<Utc>>,
        scopes: &str,
        desired_scopes: &str,
    ) -> Result<Uuid, sqlx::Error> {
        let result = if email.is_some() {
            sqlx::query_scalar::<_, Uuid>(
                r#"
                INSERT INTO oauth_accounts (provider, email, display_name, access_token, refresh_token, token_expiry, scopes, desired_scopes)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (provider, email) DO UPDATE SET
                    display_name = EXCLUDED.display_name,
                    access_token = EXCLUDED.access_token,
                    refresh_token = COALESCE(EXCLUDED.refresh_token, oauth_accounts.refresh_token),
                    token_expiry = EXCLUDED.token_expiry,
                    scopes = EXCLUDED.scopes,
                    desired_scopes = EXCLUDED.desired_scopes,
                    updated_at = NOW()
                RETURNING id
                "#,
            )
            .bind(provider)
            .bind(email)
            .bind(display_name)
            .bind(access_token)
            .bind(refresh_token)
            .bind(token_expiry)
            .bind(scopes)
            .bind(desired_scopes)
            .fetch_one(pool)
            .await?
        } else {
            sqlx::query_scalar::<_, Uuid>(
                r#"
                INSERT INTO oauth_accounts (provider, email, display_name, access_token, refresh_token, token_expiry, scopes, desired_scopes)
                VALUES ($1, NULL, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (provider) WHERE email IS NULL DO UPDATE SET
                    display_name = EXCLUDED.display_name,
                    access_token = EXCLUDED.access_token,
                    refresh_token = COALESCE(EXCLUDED.refresh_token, oauth_accounts.refresh_token),
                    token_expiry = EXCLUDED.token_expiry,
                    scopes = EXCLUDED.scopes,
                    desired_scopes = EXCLUDED.desired_scopes,
                    updated_at = NOW()
                RETURNING id
                "#,
            )
            .bind(provider)
            .bind(display_name)
            .bind(access_token)
            .bind(refresh_token)
            .bind(token_expiry)
            .bind(scopes)
            .bind(desired_scopes)
            .fetch_one(pool)
            .await?
        };

        Ok(result)
    }

    /// Get an OAuth account by ID (includes tokens)
    pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<OAuthAccount>, sqlx::Error> {
        sqlx::query_as::<_, OAuthAccount>(
            r#"
            SELECT id, provider, email, display_name, access_token,
                   refresh_token, token_expiry, scopes, desired_scopes,
                   created_at, updated_at
            FROM oauth_accounts
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    /// Resolve the active OAuth account for a provider: the most-recently
    /// CONNECTED one.
    ///
    /// When two accounts share a provider (an old narrow-scope connection plus
    /// a newer broad one), a fresh connect must win. Otherwise the stale row
    /// permanently shadows the broader token and re-connecting cannot take
    /// effect.
    ///
    /// `created_at`, never `updated_at`: a re-connect is an in-place upsert
    /// that leaves `created_at` alone, while every token refresh bumps
    /// `updated_at`. Ordering by `updated_at` would keep the last-*used*
    /// account winning, which is the stale one.
    pub async fn get_by_provider(
        pool: &PgPool,
        provider: &str,
    ) -> Result<Option<OAuthAccount>, sqlx::Error> {
        sqlx::query_as::<_, OAuthAccount>(
            r#"
            SELECT id, provider, email, display_name, access_token,
                   refresh_token, token_expiry, scopes, desired_scopes,
                   created_at, updated_at
            FROM oauth_accounts
            WHERE provider = $1
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(provider)
        .fetch_optional(pool)
        .await
    }

    /// Refresh an account's tokens in place.
    ///
    /// Deliberately silent, and registered as the one `oauth_accounts`
    /// exemption in `core::announced_surfaces`. A token rotation changes
    /// nothing the user can see, and announcing one would put a row on the
    /// timeline each time a token neared expiry.
    pub async fn update_tokens(
        pool: &PgPool,
        id: Uuid,
        access_token: &str,
        token_expiry: Option<DateTime<Utc>>,
        refresh_token: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE oauth_accounts
            SET access_token = $2,
                token_expiry = $3,
                refresh_token = COALESCE($4, refresh_token),
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(access_token)
        .bind(token_expiry)
        .bind(refresh_token)
        .execute(pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// List all OAuth accounts without tokens (safe for API)
    pub async fn list(pool: &PgPool) -> Result<Vec<OAuthAccountInfo>, sqlx::Error> {
        sqlx::query_as::<_, OAuthAccountInfo>(
            r#"
            SELECT id, provider, email, display_name, scopes, desired_scopes,
                   created_at, updated_at
            FROM oauth_accounts
            ORDER BY provider ASC, created_at ASC
            "#,
        )
        .fetch_all(pool)
        .await
    }

    /// List all OAuth accounts including tokens (for env injection into scripts)
    pub async fn list_all_with_tokens(pool: &PgPool) -> Result<Vec<OAuthAccount>, sqlx::Error> {
        sqlx::query_as::<_, OAuthAccount>(
            r#"
            SELECT id, provider, email, display_name, access_token,
                   refresh_token, token_expiry, scopes, desired_scopes,
                   created_at, updated_at
            FROM oauth_accounts
            ORDER BY provider ASC, created_at ASC
            "#,
        )
        .fetch_all(pool)
        .await
    }

    /// Delete an OAuth account row. **Private on purpose**: [`Self::delete`] is
    /// the reachable mutator, and it emits.
    async fn delete_row(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM oauth_accounts WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Store a freshly authorized OAuth account and announce it. The only way
    /// to connect one.
    ///
    /// Announces on a re-authorization too (the upsert path), because
    /// re-connecting is how scopes are granted: the account's capabilities
    /// changed even though the row already existed.
    #[allow(clippy::too_many_arguments)] // one arg per token column, plus the bus and actor
    pub async fn connect(
        pool: &PgPool,
        event_bus: &EventBus,
        provider: &str,
        email: Option<&str>,
        display_name: Option<&str>,
        access_token: &str,
        refresh_token: Option<&str>,
        token_expiry: Option<DateTime<Utc>>,
        scopes: &str,
        desired_scopes: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<Uuid, sqlx::Error> {
        let id = Self::upsert_row(
            pool,
            provider,
            email,
            display_name,
            access_token,
            refresh_token,
            token_expiry,
            scopes,
            desired_scopes,
        )
        .await?;
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::OAuthAccountConnected {
                    account_id: id.to_string(),
                    provider: provider.to_string(),
                    email: email.map(str::to_string),
                    actor,
                }),
                "[OAuth] OAuthAccountConnected",
            )
            .await;
        Ok(id)
    }

    /// Delete an OAuth account and announce it. The only way to disconnect one.
    ///
    /// `OAuthAccountDeleted` fires only when a row was actually deleted, so a
    /// repeated or racing disconnect announces once.
    pub async fn delete(
        pool: &PgPool,
        event_bus: &EventBus,
        id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let removed = Self::delete_row(pool, id).await?;
        if removed {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::OAuthAccountDeleted {
                        account_id: id.to_string(),
                        actor,
                    }),
                    "[OAuth] OAuthAccountDeleted",
                )
                .await;
        }
        Ok(removed)
    }
}

/// User-facing message for "this OAuth provider was requested but no account
/// is connected for it". One source of truth so the proxy `script_handshake`
/// layer's production code and its test stub stay in lockstep.
pub fn provider_not_connected_msg(provider: &str) -> String {
    format!(
        "script_handshake requires OAuth provider '{}' but no account is connected; \
         user must connect it first via connect_oauth_account",
        provider
    )
}

/// Why `get_account_with_fresh_token` could not return a usable account.
/// Each caller maps these to its own HTTP status and error message.
pub enum AccountLookupError {
    NotConnected,
    DbError(BoxError),
    RefreshFailed(BoxError),
}

/// One-shot "give me a valid access token for this provider": loads the
/// account row and refreshes the token if it's expired or expiring soon.
/// Used by `proxy_script_layer::DbOAuthLookup` and the
/// `/api/v1/oauth/{provider}/access-token` endpoint.
pub async fn get_account_with_fresh_token(
    pool: &PgPool,
    provider: &str,
) -> Result<OAuthAccount, AccountLookupError> {
    let mut account = OAuthStore::get_by_provider(pool, provider)
        .await
        .map_err(|e| AccountLookupError::DbError(Box::new(e)))?
        .ok_or(AccountLookupError::NotConnected)?;
    refresh_oauth_if_needed(pool, &mut account)
        .await
        .map_err(AccountLookupError::RefreshFailed)?;
    Ok(account)
}

/// Build `OAUTH_*` environment variables from connected OAuth accounts.
///
/// The provider name goes through [`crate::core::env_var_segment`], the same
/// transform `CRED_*` uses, so any provider lands as a legal shell identifier.
/// Shared by subprocess injection and the proxy `script_handshake` layer, so
/// the names stay identical across entry points.
pub fn account_env_vars(accounts: Vec<OAuthAccount>) -> Vec<(String, String)> {
    let mut env_vars = Vec::new();
    for account in accounts {
        let prefix = format!("OAUTH_{}", crate::core::env_var_segment(&account.provider));
        env_vars.push((format!("{}_ACCESS_TOKEN", prefix), account.access_token));
        if let Some(email) = account.email {
            env_vars.push((format!("{}_EMAIL", prefix), email));
        }
    }
    env_vars
}

// ---------------------------------------------------------------------------
// Loopback callback endpoint — the single source of the redirect URI
// ---------------------------------------------------------------------------
//
// The authorization request and the token exchange MUST send a byte-identical
// `redirect_uri` (OAuth 2.0 §4.1.3), matching what the provider has registered.
// Both legs therefore read the ONE value `resolve_redirect_uri` returns, never
// a rebuilt literal.

/// Port the temporary callback listener binds. Fixed (not ephemeral) because
/// the URI has to be registered with the provider ahead of time.
const CALLBACK_PORT: u16 = 14981;
/// Path the callback listener answers on.
const CALLBACK_PATH: &str = "/oauth/callback";
/// Host form advertised unless the credential overrides it. The loopback IP is
/// the default because some providers reject the name `localhost` outright.
const DEFAULT_CALLBACK_HOST: &str = "127.0.0.1";
/// Every loopback host form the listener can receive on. These are the only
/// values a credential's `redirect_uri` may take: the port and path belong to
/// the listener, so any other value produces a flow that hangs until the
/// timeout.
///
/// Which form a provider will accept is a *provider* fact and lives in
/// `system-knowhow/oauth-providers.md`, not here.
const CALLBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];

fn callback_uri_for_host(host: &str) -> String {
    format!("http://{}:{}{}", host, CALLBACK_PORT, CALLBACK_PATH)
}

/// The redirect URI advertised when a credential doesn't override it.
///
/// Public because the Connect form offers it for copying into the provider's
/// console: it has to be registered character for character, so the one place
/// that knows it must be the one that states it.
pub fn default_redirect_uri() -> String {
    callback_uri_for_host(DEFAULT_CALLBACK_HOST)
}

/// Every redirect URI the local listener can serve, in preference order.
fn accepted_redirect_uris() -> Vec<String> {
    CALLBACK_HOSTS
        .iter()
        .copied()
        .map(callback_uri_for_host)
        .collect()
}

/// Resolve the redirect URI for a flow from the credential JSON.
///
/// Absent or blank `redirect_uri` => the default loopback-IP form, so every
/// already-connected provider keeps the exact URI it was registered with. A
/// supplied value must be one of `accepted_redirect_uris()`; anything else is
/// rejected here rather than producing a browser redirect the listener never
/// receives.
fn resolve_redirect_uri(client_config: &serde_json::Value) -> Result<String, BoxError> {
    let configured = client_config["redirect_uri"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(uri) = configured else {
        return Ok(default_redirect_uri());
    };
    let accepted = accepted_redirect_uris();
    if accepted.iter().any(|candidate| candidate == uri) {
        return Ok(uri.to_string());
    }
    Err(format!(
        "redirect_uri '{}' is not one of Lucidos's own callback URLs — the local listener \
         only receives on port {} at {}. Use one of: {}",
        uri,
        CALLBACK_PORT,
        CALLBACK_PATH,
        accepted.join(", ")
    )
    .into())
}

// ---------------------------------------------------------------------------
// Client authentication — confidential vs public
// ---------------------------------------------------------------------------

/// A PKCE verifier/challenge pair (RFC 7636).
///
/// The verifier is a one-time secret: it is the only thing standing between an
/// intercepted authorization code and a token, so it must never be logged.
struct Pkce {
    verifier: String,
    challenge: String,
}

// Manual `Debug` so the verifier can't leak through `{:?}`, matching the
// treatment of access/refresh tokens elsewhere in this module.
impl std::fmt::Debug for Pkce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &"<redacted>")
            .field("challenge", &self.challenge)
            .finish()
    }
}

impl Pkce {
    fn generate() -> Self {
        let verifier = random_url_token();
        let challenge = Self::challenge_for(&verifier);
        Self {
            verifier,
            challenge,
        }
    }

    /// `BASE64URL-NOPAD(SHA256(ASCII(verifier)))`, the `S256` method.
    fn challenge_for(verifier: &str) -> String {
        use sha2::{Digest, Sha256};
        base64_url_nopad(&Sha256::digest(verifier.as_bytes()))
    }
}

fn base64_url_nopad(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 32 CSPRNG bytes, base64url-encoded to 43 characters that need no escaping in
/// a URL. Shared by both unguessable values an authorization request carries:
/// the PKCE verifier (RFC 7636 wants 43..=128 unreserved characters, and 32
/// bytes is its recommended entropy) and the `state` nonce.
fn random_url_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64_url_nopad(&bytes)
}

/// The `state` parameter for one authorization: an unguessable value sent on the
/// authorization request and required back on the callback (RFC 6749 §4.1.2
/// makes echoing it mandatory when it was sent).
///
/// It does two jobs, and the second is why it is not optional.
///
/// 1. **CSRF.** The callback listener is a plain loopback socket, so any local
///    process or browsed page can issue `GET
///    127.0.0.1:<port>/oauth/callback?code=...` while a flow is open.
/// 2. **Identity.** A new authorization supersedes the previous one, so a
///    redirect from an abandoned flow can reach a listener that never issued
///    it (`docs/adr/0068-oauth-callback-port-has-one-owner.md`).
///
/// Compared with `==` rather than in constant time on purpose: this is a nonce
/// the legitimate redirect carries, not a secret an attacker guesses byte by
/// byte.
fn generate_oauth_state() -> String {
    random_url_token()
}

/// How the client proves itself when redeeming the authorization code.
///
/// Derived SOLELY from whether the credential carries a `client_secret`. That
/// is OAuth's own confidential/public distinction, so the engine never needs to
/// know which provider it is talking to.
///
/// A desktop app that ships no secret is a *public* client (RFC 8252), and the
/// redemption is authenticated with PKCE instead. Providers reject a secret
/// from a public client and reject a secret-less redemption from a confidential
/// one, so the two shapes must not be mixed.
enum ClientAuth {
    /// The credential has a `client_secret`: send it, and send no PKCE.
    Confidential(String),
    /// No `client_secret`: omit it and authenticate with PKCE.
    Public(Pkce),
}

// Manual `Debug` so neither the client secret nor the PKCE verifier leaks.
impl std::fmt::Debug for ClientAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Confidential(_) => f.write_str("Confidential(<redacted>)"),
            Self::Public(pkce) => f.debug_tuple("Public").field(pkce).finish(),
        }
    }
}

impl ClientAuth {
    fn from_secret(client_secret: Option<&str>) -> Self {
        match client_secret.map(str::trim).filter(|s| !s.is_empty()) {
            Some(secret) => Self::Confidential(secret.to_string()),
            None => Self::Public(Pkce::generate()),
        }
    }

    fn client_secret(&self) -> Option<&str> {
        match self {
            Self::Confidential(secret) => Some(secret),
            Self::Public(_) => None,
        }
    }

    fn code_challenge(&self) -> Option<&str> {
        match self {
            Self::Confidential(_) => None,
            Self::Public(pkce) => Some(&pkce.challenge),
        }
    }

    fn code_verifier(&self) -> Option<&str> {
        match self {
            Self::Confidential(_) => None,
            Self::Public(pkce) => Some(&pkce.verifier),
        }
    }

    /// Label for logs. Never includes the secret or the verifier.
    fn kind(&self) -> &'static str {
        match self {
            Self::Confidential(_) => "confidential",
            Self::Public(_) => "public+PKCE",
        }
    }
}

/// The extra authorization-URL parameters a provider needs, beyond the ones the
/// protocol itself defines.
///
/// This exists because "ask for a refresh token" has no standard spelling. A
/// provider that never sees its own spelling issues no refresh token at all.
///
/// So the value is credential data, one per registration, documented per
/// provider in `system-knowhow/oauth-providers.md`. No provider-specific
/// BEHAVIOR is coded here (CLAUDE.md § "No provider-specific instructions in
/// code"): the flow sends whatever the credential stores. The one place this
/// module branches on a provider is [`provider_for_url`], which is a deliberate
/// exception and is not extended.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthorizeParams(Vec<(String, String)>);

/// What a credential with no `authorize_params` sends. Absent means
/// "unchanged", never "nothing": a credential written before the key existed
/// was authorized with exactly these two.
pub const DEFAULT_AUTHORIZE_PARAMS: &str = "access_type=offline&prompt=consent";

/// The opt-out, for a provider strict enough to reject a parameter it does not
/// know. Without it, [`DEFAULT_AUTHORIZE_PARAMS`] would be unavoidable.
const AUTHORIZE_PARAMS_NONE: &str = "none";

/// Parameters the flow itself owns. A credential is agent- and user-writable.
/// Letting it set these would let a field that reads like provider trivia
/// rewrite the loopback `redirect_uri`, or narrow the requested `scope`.
const RESERVED_AUTHORIZE_KEYS: &[&str] = &[
    "client_id",
    "redirect_uri",
    "response_type",
    "scope",
    "state",
    "code_challenge",
    "code_challenge_method",
];

impl AuthorizeParams {
    /// Parse the credential's `authorize_params`, in `key=value&key=value` form.
    ///
    /// Absent or blank means [`DEFAULT_AUTHORIZE_PARAMS`]; the literal `none`
    /// means send nothing extra. Both halves of a pair are percent-decoded here
    /// and re-encoded on the way out, so a value carrying `&` or `=` survives
    /// as one value.
    ///
    /// Errors rather than dropping a bad pair. A silently ignored parameter
    /// would surface as a provider behaving inexplicably, with nothing to point
    /// at.
    pub fn parse(raw: Option<&str>) -> Result<Self, String> {
        let raw = raw.map(str::trim).unwrap_or_default();
        let raw = if raw.is_empty() {
            DEFAULT_AUTHORIZE_PARAMS
        } else if raw.eq_ignore_ascii_case(AUTHORIZE_PARAMS_NONE) {
            return Ok(Self(Vec::new()));
        } else {
            raw
        };

        let mut pairs = Vec::new();
        for part in raw.split('&').filter(|p| !p.trim().is_empty()) {
            let (key, value) = part.split_once('=').ok_or_else(|| {
                format!("authorize_params entry '{part}' is not in key=value form")
            })?;
            let key = decode_param(key.trim());
            let value = decode_param(value.trim());
            if key.is_empty() {
                return Err(format!("authorize_params entry '{part}' has an empty key"));
            }
            if RESERVED_AUTHORIZE_KEYS
                .iter()
                .any(|r| key.eq_ignore_ascii_case(r))
            {
                return Err(format!(
                    "authorize_params may not set '{key}': the OAuth flow owns it"
                ));
            }
            pairs.push((key, value));
        }
        Ok(Self(pairs))
    }

    /// Append the pairs to an authorization URL that already carries a query.
    fn append_to(&self, url: &mut String) {
        for (key, value) in &self.0 {
            url.push('&');
            url.push_str(&urlencoding::encode(key));
            url.push('=');
            url.push_str(&urlencoding::encode(value));
        }
    }
}

/// Percent-decode one half of an `authorize_params` pair. Invalid encoding is
/// left as written: a bare `%` is likelier to be a literal than a typo worth
/// failing the whole flow over.
fn decode_param(raw: &str) -> String {
    urlencoding::decode(raw)
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| raw.to_string())
}

/// Build the provider's authorization URL.
///
/// `code_challenge` is appended only for a public client, so a confidential
/// flow produces exactly the URL it always has.
fn build_authorize_url(
    auth_url: &str,
    client_id: &str,
    redirect_uri: &str,
    scopes: &str,
    state: &str,
    auth: &ClientAuth,
    extra: &AuthorizeParams,
) -> String {
    // Some authorization endpoints already carry a query (Azure AD B2C pins its
    // user flow with `?p=…`), so appending a second `?` would corrupt the URL.
    let separator = if auth_url.contains('?') { '&' } else { '?' };
    let mut url = format!(
        "{}{}client_id={}&redirect_uri={}&response_type=code&scope={}&state={}",
        auth_url,
        separator,
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(scopes),
        urlencoding::encode(state),
    );
    extra.append_to(&mut url);
    if let Some(challenge) = auth.code_challenge() {
        url.push_str(&format!(
            "&code_challenge={}&code_challenge_method=S256",
            urlencoding::encode(challenge)
        ));
    }
    url
}

// ---------------------------------------------------------------------------
// Token exchange & refresh (HTTP)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
}

// Manual `Debug` so a token-exchange/refresh response never leaks its tokens
// through `{:?}`. Non-secret fields (expiry, type, scope) stay visible.
impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenResponse")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_in", &self.expires_in)
            .field("token_type", &self.token_type)
            .field("scope", &self.scope)
            .finish()
    }
}

/// Bounded, shared HTTP client for the OAuth token and userinfo endpoints.
///
/// A bare `reqwest::Client::new()` has NO timeout, and `refresh_oauth_if_needed`
/// runs on the email-send path, where a stalled identity provider would hang
/// the whole send. Built once so repeated calls reuse the connection pool and
/// the rustls context.
fn bounded_http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            // Only fails on TLS-backend init.
            .expect("OAuth HTTP client builder")
    });
    &CLIENT
}

/// Longest raw provider body echoed into an error. A token endpoint that fails
/// behind a proxy can answer with a whole HTML page; the first couple of
/// kilobytes always carry the useful part.
const MAX_ERROR_BODY_CHARS: usize = 2000;

/// Render a non-success token-endpoint response into one human-readable line.
///
/// OAuth 2.0 §5.2 error bodies are JSON with `error` / `error_description`;
/// Microsoft adds a numeric `error_codes` array. Pulling those out is what
/// makes the flow debuggable from the UI. A body that is not an OAuth error
/// object falls back to the raw text rather than being dropped.
///
/// `operation` names the leg ("Token exchange" / "Token refresh"), so the
/// caller must NOT re-wrap the result: a second prefix reads as "Token exchange
/// failed: Token exchange failed (400)".
fn describe_token_error(operation: &str, status: reqwest::StatusCode, body: &str) -> String {
    let body = body.trim();
    let mut parts: Vec<String> = Vec::new();
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(error) = json["error"].as_str() {
            parts.push(error.to_string());
        }
        if let Some(description) = json["error_description"].as_str() {
            parts.push(description.to_string());
        }
        if let Some(codes) = json["error_codes"].as_array().filter(|c| !c.is_empty()) {
            let codes: Vec<String> = codes.iter().map(|c| c.to_string()).collect();
            parts.push(format!("error_codes: [{}]", codes.join(", ")));
        }
    }
    if !parts.is_empty() {
        return format!("{} failed ({}): {}", operation, status, parts.join(" — "));
    }
    if body.is_empty() {
        return format!(
            "{} failed ({}) with an empty response body",
            operation, status
        );
    }
    format!(
        "{} failed ({}): {}",
        operation,
        status,
        &body[..body.floor_char_boundary(MAX_ERROR_BODY_CHARS)]
    )
}

/// POST a token-endpoint form and parse the response, surfacing the provider's
/// own error text on failure. Shared by the code exchange and the refresh so
/// both legs report failures identically.
async fn post_token_request(
    operation: &str,
    token_url: &str,
    form: &[(&str, &str)],
) -> Result<TokenResponse, BoxError> {
    let client = bounded_http_client();
    let resp = client
        .post(token_url)
        .header("Accept", "application/json")
        .form(form)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(describe_token_error(operation, status, &body).into());
    }

    let token: TokenResponse = resp.json().await?;
    Ok(token)
}

/// Form pairs for the authorization-code redemption (OAuth 2.0 §4.1.3).
fn exchange_form<'a>(
    code: &'a str,
    client_id: &'a str,
    redirect_uri: &'a str,
    auth: &'a ClientAuth,
) -> Vec<(&'static str, &'a str)> {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
    ];
    if let Some(secret) = auth.client_secret() {
        form.push(("client_secret", secret));
    }
    form.push(("redirect_uri", redirect_uri));
    if let Some(verifier) = auth.code_verifier() {
        form.push(("code_verifier", verifier));
    }
    form
}

/// Form pairs for a refresh-token grant (OAuth 2.0 §6).
///
/// PKCE has no role here: it authenticates the one-time code redemption, not
/// later refreshes. A public client must still omit `client_secret`, or the
/// provider rejects the refresh as it would reject the redemption.
fn refresh_form<'a>(
    refresh_token: &'a str,
    client_id: &'a str,
    client_secret: Option<&'a str>,
) -> Vec<(&'static str, &'a str)> {
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }
    form
}

/// Exchange an authorization code for tokens.
async fn exchange_code(
    token_url: &str,
    client_id: &str,
    auth: &ClientAuth,
    code: &str,
    redirect_uri: &str,
) -> Result<TokenResponse, BoxError> {
    let form = exchange_form(code, client_id, redirect_uri, auth);
    post_token_request("Token exchange", token_url, &form).await
}

/// Refresh an access token using a refresh token.
async fn refresh_access_token(
    token_url: &str,
    client_id: &str,
    client_secret: Option<&str>,
    refresh_token: &str,
) -> Result<TokenResponse, BoxError> {
    let form = refresh_form(refresh_token, client_id, client_secret);
    post_token_request("Token refresh", token_url, &form).await
}

/// Returns true if the account's token needs refreshing (expired, expiring
/// within 60 seconds, or no expiry with a refresh token available).
pub fn token_needs_refresh(account: &OAuthAccount) -> bool {
    match account.token_expiry {
        Some(exp) => exp < Utc::now() + chrono::Duration::seconds(60),
        // No expiry stored, so validity cannot be checked: refresh proactively.
        // A provider whose tokens do not expire issues no refresh token either,
        // so this never triggers for one.
        None => account.refresh_token.is_some(),
    }
}

/// Refresh an expired or nearly-expired token and persist it, mutating
/// `account` in place. A token that is still good is left alone.
pub async fn refresh_oauth_if_needed(
    pool: &PgPool,
    account: &mut OAuthAccount,
) -> Result<(), BoxError> {
    if !token_needs_refresh(account) {
        return Ok(());
    }

    let refresh_token = match account.refresh_token {
        Some(ref rt) => rt.clone(),
        None => return Err("OAuth token expired but no refresh token available".into()),
    };

    let cred_service = client_provider_name(&account.provider);
    let client_cred = super::CredentialStore::get_oauth_client(pool, &cred_service)
        .await?
        .ok_or_else(|| format!("No client credentials found for {}", cred_service))?;

    let config: serde_json::Value = serde_json::from_str(&client_cred.auth_value)?;
    let cid = config["client_id"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Missing client_id in {} credentials", cred_service))?;
    // Blank or absent means a public client, whose refresh must omit
    // `client_secret` exactly as its code redemption did.
    let csec = config["client_secret"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let turl = config["token_url"]
        .as_str()
        .ok_or_else(|| format!("Missing token_url in {} credentials", cred_service))?
        .to_string();

    crate::log!(
        "[OAuth] Refreshing {} token for {}",
        account.provider,
        account.email.as_deref().unwrap_or("unknown")
    );
    let new_tokens = refresh_access_token(&turl, cid, csec, &refresh_token).await?;
    let new_expiry = new_tokens
        .expires_in
        .map(|s| Utc::now() + chrono::Duration::seconds(s as i64));
    OAuthStore::update_tokens(
        pool,
        account.id,
        &new_tokens.access_token,
        new_expiry,
        new_tokens.refresh_token.as_deref(),
    )
    .await?;
    account.access_token = new_tokens.access_token;
    account.token_expiry = new_expiry;
    if let Some(ref rt) = new_tokens.refresh_token {
        account.refresh_token = Some(rt.clone());
    }
    crate::log!(
        "[OAuth] Successfully refreshed {} token, expires in {}s",
        account.provider,
        new_tokens.expires_in.unwrap_or(0)
    );

    Ok(())
}

/// Force-refresh an OAuth token regardless of expiry.
/// Use when a 401 suggests the token is invalid despite not appearing expired.
pub async fn force_refresh_oauth(
    pool: &PgPool,
    account: &mut OAuthAccount,
) -> Result<(), BoxError> {
    account.token_expiry = Some(Utc::now() - chrono::Duration::seconds(1));
    refresh_oauth_if_needed(pool, account).await
}

/// Merge two space-separated scope strings, deduplicating.
fn merge_scopes(existing: &str, requested: &str) -> String {
    let mut all: Vec<&str> = existing.split_whitespace().collect();
    for scope in requested.split_whitespace() {
        if !all.contains(&scope) {
            all.push(scope);
        }
    }
    all.join(" ")
}

/// Which scopes an authorization asked for and did not get, in the order they
/// were requested.
///
/// **Exact token set difference**, never containment. This is the same rule as
/// `missingScopes` in `components/settings/oauthConnectForm.ts`, so the agent
/// and the Accounts panel cannot disagree about whether an account is short.
///
/// **Not the same question as [`crate::core::backup::missing_scopes`]**, which
/// asks whether a backup provider can upload. Its `required_scopes` are
/// substring MATCHERS, so it uses containment on purpose. Containment here
/// would report a refused scope as granted whenever another granted scope
/// contained its name.
///
/// An empty requested set yields no shortfall: nothing was asked for, so
/// nothing can be short.
pub fn missing_requested_scopes(requested: &str, granted: &str) -> Vec<String> {
    let held: std::collections::HashSet<&str> = granted.split_whitespace().collect();
    requested
        .split_whitespace()
        .filter(|scope| !held.contains(scope))
        .map(str::to_string)
        .collect()
}

/// Temporary loopback listener for the OAuth callback.
///
/// Binds **both** loopback families on the same port. IPv4 is required; IPv6 is
/// best-effort so a host with IPv6 disabled still completes the flow. Both are
/// needed because `localhost` resolves to `::1` first on an IPv6-enabled host,
/// so an IPv4-only socket silently never receives a `localhost` callback.
struct CallbackListener {
    listeners: Vec<tokio::net::TcpListener>,
}

impl CallbackListener {
    /// Returns the raw `io::Error` rather than a `BoxError` so the caller can
    /// branch on its kind: `AddrInUse` is the one failure with a remedy the user
    /// can act on, and it needs different words from every other bind failure
    /// (see [`callback_bind_error`]).
    async fn bind(port: u16) -> std::io::Result<Self> {
        let v4 = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
        // Resolve the port actually bound before adding the second socket — the
        // caller may pass 0 (tests), and both families must land on one port.
        let port = v4.local_addr()?.port();
        let mut listeners = vec![v4];
        match tokio::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, port)).await {
            Ok(v6) => listeners.push(v6),
            Err(e) => crate::log!(
                "[OAuth] IPv6 loopback [::1]:{} unavailable, listening on IPv4 only: {}",
                port,
                e
            ),
        }
        Ok(Self { listeners })
    }

    /// Addresses actually bound — lets the tests drive each loopback family.
    #[cfg(test)]
    fn local_addrs(&self) -> Vec<std::net::SocketAddr> {
        self.listeners
            .iter()
            .filter_map(|listener| listener.local_addr().ok())
            .collect()
    }

    /// Accept the next connection on whichever family it arrives.
    /// `TcpListener::accept` is cancel-safe, so dropping the losing futures is fine.
    async fn accept(&self) -> std::io::Result<tokio::net::TcpStream> {
        let accepts: Vec<_> = self
            .listeners
            .iter()
            .map(|listener| Box::pin(listener.accept()))
            .collect();
        let (result, _, _) = futures::future::select_all(accepts).await;
        result.map(|(stream, _)| stream)
    }
}

/// The task that currently owns the loopback callback port, if any.
///
/// One process-level owner, superseded by whichever authorization starts next.
/// See `docs/adr/0068-oauth-callback-port-has-one-owner.md`.
static ACTIVE_CALLBACK_FLOW: std::sync::LazyLock<tokio::sync::Mutex<Option<ActiveCallbackFlow>>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(None));

/// The flow registered in [`ACTIVE_CALLBACK_FLOW`], and whether it still holds
/// the port.
///
/// The two are separate because a flow's task OUTLIVES its ownership of the
/// socket: the token exchange, the userinfo call and the account write all run
/// with the port already released.
struct ActiveCallbackFlow {
    task: tokio::task::JoinHandle<()>,
    /// Set once at registration, cleared by the flow itself the instant it stops
    /// holding the listener. Ordered so that `false` implies the sockets are
    /// already closed: the store happens after the future that owns them has
    /// resolved and dropped them.
    holds_port: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// What a caller is told when its flow's result channel closed with no result.
///
/// The sender drops without sending only when the task went away, which is
/// almost always a supersede: something the user did, not an internal fault.
/// Shared by BOTH entry points, because either can be superseded and they must
/// not describe it differently.
pub const FLOW_SUPERSEDED_MSG: &str =
    "This authorization was canceled, most likely because a newer one was started. \
     Start it again if you still need it.";

/// Cancel the flow that owns the callback port, and wait until its socket is
/// actually closed. Reports whether a live flow was superseded.
///
/// **The await is the whole point.** `JoinHandle::abort` only *requests*
/// cancellation, so returning straight after it would let the caller's `bind`
/// race the socket's close. The expected outcome is `Err(JoinError::cancelled)`,
/// which is not a failure and is discarded.
///
/// A flow that has already released the port is **detached, not aborted**: it is
/// not in the caller's way, and killing it would throw away an authorization the
/// user completed.
///
/// Takes the slot by reference rather than reading [`ACTIVE_CALLBACK_FLOW`]
/// itself, so the release-then-rebind guarantee is testable against a
/// caller-supplied slot.
async fn release_callback_port(slot: &mut Option<ActiveCallbackFlow>) -> bool {
    let Some(flow) = slot.take() else {
        return false;
    };
    if !flow.holds_port.load(std::sync::atomic::Ordering::Acquire) {
        return false;
    }
    flow.task.abort();
    let _ = flow.task.await;
    true
}

/// Explain a callback-listener bind failure.
///
/// By the time this runs the engine has already released its own flow, so
/// `AddrInUse` means a *different* process holds the port. Workspaces run
/// concurrently and share this one machine-wide port, so that is almost always
/// another workspace part-way through connecting an account. Say so: the raw
/// `Address already in use` names nothing the user can act on. Every other
/// error kind keeps its own text.
fn callback_bind_error(port: u16, e: std::io::Error) -> BoxError {
    if e.kind() != std::io::ErrorKind::AddrInUse {
        return e.into();
    }
    format!(
        "the OAuth callback port {port} is already in use. Lucidos has released its own \
         authorization, so another program on this machine is holding it, most often another \
         Lucidos workspace part-way through connecting an account. Finish or abandon that one, \
         then try again."
    )
    .into()
}

/// Cap on the buffered request line. Authorization codes run to kilobytes, so
/// the ceiling is generous: it exists only to stop a client that never sends a
/// newline from growing the buffer forever.
const MAX_REQUEST_LINE_BYTES: usize = 64 * 1024;

/// How long one connection gets to send its request line before we abandon it
/// and go back to waiting.
///
/// A browser sends immediately after connecting, but a speculative preconnect
/// can open a socket and hold it idle. Unbounded, that idle socket blocks the
/// real redirect behind it in the accept queue until the flow times out.
const CALLBACK_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Read the HTTP request line — everything before the first newline.
///
/// This MUST loop. A single read returns only what has arrived so far, and the
/// authorization code sits in the request line. A read-once implementation
/// therefore truncates a code split across TCP reads, and the provider rejects
/// it long after the browser reported success.
async fn read_request_line(stream: &mut tokio::net::TcpStream) -> Result<String, BoxError> {
    use tokio::io::AsyncReadExt;

    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(newline) = buf.iter().position(|byte| *byte == b'\n') {
            buf.truncate(newline);
            break;
        }
        if buf.len() > MAX_REQUEST_LINE_BYTES {
            return Err(format!(
                "OAuth callback request line exceeded {} bytes",
                MAX_REQUEST_LINE_BYTES
            )
            .into());
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break; // EOF without a newline: use whatever arrived.
        }
        buf.extend_from_slice(&chunk[..read]);
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Look up a query parameter and percent-decode it.
///
/// `plus_is_space` decides how `+` is treated. Providers form-encode
/// human-readable text with `+` for space. An authorization code is an opaque
/// RFC 3986 query value where `+` is literal, so decoding it as a space would
/// corrupt the code.
fn query_param(query: &str, key: &str, plus_is_space: bool) -> Option<String> {
    let raw = query
        .split('&')
        .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))?;
    let raw = if plus_is_space {
        raw.replace('+', " ")
    } else {
        raw.to_string()
    };
    // A malformed escape must not hide the value: fall back to the raw text.
    Some(
        urlencoding::decode(&raw)
            .map(|decoded| decoded.into_owned())
            .unwrap_or(raw),
    )
}

/// Does a callback query carry the `state` this flow sent?
///
/// `plus_is_space` is false because the value is an opaque base64url token, not
/// form-encoded text. A callback with no `state` gets the same verdict as one
/// with a wrong `state`: every conforming provider echoes what it was sent
/// (RFC 6749 §4.1.2), so an absent value did not come from our authorization.
fn callback_state_matches(query: &str, expected: &str) -> bool {
    query_param(query, "state", false).is_some_and(|got| got == expected)
}

/// Turn a callback query string into the authorization code, or into the
/// provider's own reason for refusing.
///
/// A denial arrives as `?error=…&error_description=…` with no `code`. Reporting
/// that as "no authorization code" hides the one piece of information the user
/// needs (OAuth 2.0 §4.1.2.1).
fn parse_callback_query(query: &str) -> Result<String, BoxError> {
    if let Some(code) = query_param(query, "code", false).filter(|c| !c.is_empty()) {
        return Ok(code);
    }
    if let Some(error) = query_param(query, "error", true).filter(|e| !e.is_empty()) {
        let description = query_param(query, "error_description", true).filter(|d| !d.is_empty());
        return Err(match description {
            Some(description) => {
                format!(
                    "provider refused authorization: {} ({})",
                    error, description
                )
            }
            None => format!("provider refused authorization: {}", error),
        }
        .into());
    }
    Err("No authorization code in callback".into())
}

/// Best-effort browser response. The flow's outcome reaches the user through
/// the engine, so a browser that has already navigated away must not fail the
/// caller. Log it anyway, so a regression here surfaces.
async fn respond_to_browser(stream: &mut tokio::net::TcpStream, status: &str, body: &str) {
    use tokio::io::AsyncWriteExt;

    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{}",
        status,
        body.len(),
        body
    );
    let write = async {
        stream.write_all(response.as_bytes()).await?;
        stream.shutdown().await
    };
    if let Err(e) = write.await {
        crate::log!(
            "[OAuth] Failed to write callback response to browser: {}",
            e
        );
    }
}

/// The Lucidos mark, baked in from the one file that defines it.
///
/// `include_str!` rather than a copy, so the artwork has one definition and no
/// second copy to keep in step with a rebrand. It is embedded rather than
/// fetched because the whole page is (see [`callback_page`]). The file's own
/// `xmlns` rides along, and `callback_page_fetches_nothing` allows exactly that
/// one namespace identifier.
const BRAND_MARK: &str = include_str!("../../../lucidos-app/public/favicon.svg");

/// The page the provider's redirect lands on.
///
/// It wears the surface and arrangement of the *workspace picker*
/// (`styles/picker.css` `.ws-picker`), so it is recognisably ours on sight. Its
/// token values are the same bounded duplication `api/sdk_iframe.css` carries.
///
/// **It fetches nothing.** No stylesheet, script, font or image, which is why
/// the CSS is inline and the mark is [`BRAND_MARK`] rather than an `<img>`. A
/// one-shot loopback listener has no engine URL in hand, so a link would trade
/// a certain render for a conditional one. A landing page that phones anywhere
/// is also a privacy surface. Pinned by `callback_page_fetches_nothing`.
///
/// **`provider` is the only interpolated value, and it is engine-side.** It
/// comes from the tool call or the credential name, never from the callback
/// query. Echoing an attacker-controllable `error_description` here would be an
/// injection sink for no benefit, so the real reason goes to the engine.
///
/// Written at callback receipt, BEFORE the code is exchanged, so it cannot
/// claim the account is connected.
fn callback_page(provider: &str, ok: bool) -> String {
    let (heading, detail, provider_label) = if ok {
        (
            "Authorization complete",
            "Lucidos is finishing the connection. You can close this tab.",
            "Authorized with",
        )
    } else {
        (
            "Authorization failed",
            "Nothing was connected. Return to Lucidos for the details.",
            "Tried to connect",
        )
    };
    // `provider` is a bare identifier (`dropbox`, `ghealth`), but escape it
    // anyway so this stays safe if a caller ever passes something richer.
    let provider = provider
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let title = if provider.is_empty() {
        "Lucidos".to_string()
    } else {
        format!("Lucidos {provider}")
    };
    // No value, no row: a flow whose provider name never reached this far would
    // otherwise draw the hairline and the label over nothing.
    let footer = if provider.is_empty() {
        String::new()
    } else {
        format!("<dl><dt>{provider_label}</dt><dd>{provider}</dd></dl>")
    };
    format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title><style>\
         :root{{color-scheme:dark}}\
         body{{margin:0;min-height:100vh;display:flex;align-items:flex-start;\
         justify-content:center;padding:4rem 1.5rem 3rem;color:#fff;\
         font:1rem/1.5 system-ui,-apple-system,BlinkMacSystemFont,'Segoe UI',\
         Roboto,Helvetica,Arial,sans-serif;\
         background:radial-gradient(60% 50% at 80% 0%,rgba(150,200,255,.45),transparent 70%),\
         radial-gradient(130% 120% at 28% 12%,#4a97ee 0%,#1f6fce 50%,#0c52ad 100%);\
         background-attachment:fixed}}\
         main{{width:100%;max-width:30rem}}\
         .brand{{display:flex;align-items:center;gap:.875rem;margin:0 0 2.5rem}}\
         .brand svg{{flex:0 0 auto;width:3rem;height:3rem;display:block;\
         border-radius:.825rem;filter:drop-shadow(0 .45rem 1.05rem rgba(3,33,80,.55))}}\
         .brand span{{font-size:1.375rem;font-weight:600;line-height:1;\
         letter-spacing:-.01em;color:rgba(255,255,255,.92)}}\
         h1{{font-size:2rem;font-weight:700;line-height:1.2;\
         letter-spacing:-.02em;margin:0 0 .625rem}}\
         p{{margin:0;font-size:1.0625rem;color:rgba(255,255,255,.78);\
         text-wrap:pretty}}\
         dl{{display:flex;align-items:baseline;justify-content:space-between;\
         gap:1rem;margin:2rem 0 0;padding-top:1rem;\
         border-top:1px solid rgba(255,255,255,.18);font-size:.8125rem}}\
         dt{{color:rgba(255,255,255,.55)}}\
         dd{{margin:0;font-weight:500}}\
         </style></head><body><main>\
         <div class=\"brand\">{BRAND_MARK}<span>Lucidos</span></div>\
         <h1>{heading}</h1><p>{detail}</p>\
         {footer}\
         </main></body></html>"
    )
}

/// Wait for the provider's redirect and extract the authorization code.
///
/// `expected_state` is the nonce this flow put on its authorization request
/// (see [`generate_oauth_state`]). A callback that does not echo it back is
/// answered and SKIPPED, never returned and never failed on: the authorization
/// the user is completing right now is still on its way. Failing here would let
/// anything that can reach the loopback port cancel a legitimate flow.
async fn wait_for_oauth_callback(
    listener: CallbackListener,
    provider: &str,
    expected_state: &str,
) -> Result<String, BoxError> {
    loop {
        let mut stream = listener.accept().await?;
        // A misbehaving connection is discarded, never fatal: a reset or idle
        // preconnect must not fail an authorization the user completed.
        let request_line =
            match tokio::time::timeout(CALLBACK_READ_TIMEOUT, read_request_line(&mut stream)).await
            {
                Ok(Ok(line)) => line,
                Ok(Err(e)) => {
                    crate::log!("[OAuth] Discarding unreadable callback connection: {}", e);
                    continue;
                }
                Err(_) => {
                    crate::log!(
                        "[OAuth] Callback connection sent nothing within {}s, discarding",
                        CALLBACK_READ_TIMEOUT.as_secs()
                    );
                    continue;
                }
            };
        let target = request_line.split_whitespace().nth(1).unwrap_or("");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));

        // Browsers open extra connections to the callback origin (favicon
        // probes, speculative preconnects). Treating one as the callback would
        // consume the accept and strand the flow. Answer them and keep waiting:
        // the caller's timeout bounds this loop.
        if path != CALLBACK_PATH || query.is_empty() {
            respond_to_browser(&mut stream, "404 Not Found", "<html><body></body></html>").await;
            continue;
        }

        // Not this flow's redirect: skip it and keep waiting. The BROWSER still
        // gets the real failure page, not the probe's empty body: the likeliest
        // sender is a human finishing a consent screen this flow superseded.
        // Nothing from the query is rendered, so the page's injection contract
        // is unchanged.
        if !callback_state_matches(query, expected_state) {
            crate::log!(
                "[OAuth] Ignoring a {} callback that did not carry this flow's state",
                provider
            );
            let body = callback_page(provider, false);
            respond_to_browser(&mut stream, "400 Bad Request", &body).await;
            continue;
        }

        let result = parse_callback_query(query);
        // The provider's own reason goes to the engine, never into this page.
        // See `callback_page` for why nothing from `query` is rendered.
        let status = if result.is_ok() {
            "200 OK"
        } else {
            "400 Bad Request"
        };
        let body = callback_page(provider, result.is_ok());
        respond_to_browser(&mut stream, status, &body).await;
        return result;
    }
}

/// What a completed OAuth token exchange produced.
///
/// A named struct rather than a tuple, because the two scope sets are the whole
/// point of reporting an authorization and are indistinguishable positionally.
pub struct OAuthFlowOutcome {
    pub email: Option<String>,
    pub display_name: Option<String>,
    /// What the provider GRANTED. Falls back to [`Self::requested_scopes`] when
    /// the token response carried no `scope`, which is what a provider granting
    /// exactly what it was asked for typically does.
    pub granted_scopes: String,
    /// What this flow ASKED for: the caller's scopes merged with everything the
    /// account already held and had ever been asked for. The difference from
    /// [`Self::granted_scopes`] is the shortfall
    /// ([`missing_requested_scopes`]).
    pub requested_scopes: String,
}

/// Outcome of an OAuth token exchange, or the reason it failed.
pub type OAuthFlowResult = Result<OAuthFlowOutcome, String>;

/// Result of preparing an OAuth flow: the auth URL, plus a receiver that
/// resolves when the background flow completes.
pub struct PreparedOAuthFlow {
    pub auth_url: String,
    pub result_rx: tokio::sync::oneshot::Receiver<OAuthFlowResult>,
}

/// Prepare an OAuth flow, spawning the background task that waits for the
/// callback, exchanges the code and stores the account.
///
/// The caller opens `auth_url` and awaits `result_rx` for the outcome.
///
/// `initiator` is the device that started the flow. It rides the
/// `OAuthAccountConnected` event, so the frontend can bring THAT device back to
/// the front when the authorization lands. `None` for an engine-internal flow,
/// which the frontend reads as "not mine".
pub async fn prepare_oauth_flow(
    pool: &PgPool,
    event_bus: &EventBus,
    provider: &str,
    scopes: &str,
    initiator: Option<MessageOrigin>,
) -> Result<PreparedOAuthFlow, BoxError> {
    use crate::core::CredentialStore;

    // What this flow will ASK for: everything the account already holds, plus
    // everything it has ever been asked for, plus what this caller wants. Never
    // narrower than any of the three.
    //
    // The `desired` half is what lets *Reconnect* recover a refused scope.
    // Merging only against the GRANTED set computes `granted UNION granted`, so
    // an account a provider had narrowed would stay narrow forever.
    let existing_account = OAuthStore::get_by_provider(pool, provider).await?;
    let merged_scopes = match existing_account {
        Some(ref acct) => {
            let held = merge_scopes(&acct.scopes, acct.desired_scopes.as_deref().unwrap_or(""));
            merge_scopes(&held, scopes)
        }
        None => scopes.to_string(),
    };

    let cred_service = client_provider_name(provider);
    let client_cred = CredentialStore::get_oauth_client(pool, &cred_service)
        .await?
        .ok_or_else(|| format!("No OAuth client credentials found for {}", cred_service))?;

    let client_config: serde_json::Value = serde_json::from_str(&client_cred.auth_value)
        .map_err(|e| format!("Invalid OAuth client credentials JSON: {}", e))?;
    let client_id = client_config["client_id"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("Missing client_id in OAuth credentials")?
        .to_string();
    // A blank or absent client_secret is not an error: it is how a *public*
    // client is expressed (see `ClientAuth`).
    let auth = ClientAuth::from_secret(client_config["client_secret"].as_str());

    let auth_url = client_config["auth_url"]
        .as_str()
        .ok_or("Missing auth_url in OAuth credentials")?
        .to_string();
    let token_url = client_config["token_url"]
        .as_str()
        .ok_or("Missing token_url in OAuth credentials")?
        .to_string();
    let userinfo_url = client_config["userinfo_url"]
        .as_str()
        .map(|s| s.to_string());
    let userinfo_method = UserinfoMethod::parse(client_config["userinfo_method"].as_str());
    // Parsed with the other credential reads, so a malformed value fails before
    // the loopback listener binds and the browser opens.
    let authorize_params = AuthorizeParams::parse(client_config["authorize_params"].as_str())?;

    // The ONE redirect URI for this flow, resolved before anything else so a
    // bad override cannot produce a redirect the listener never receives.
    let redirect_uri = resolve_redirect_uri(&client_config)?;

    // This flow's nonce, generated before the listener binds so the URL and the
    // listener are handed the same value by construction.
    let state = generate_oauth_state();

    // Hold the lock past the release, the bind and the spawn, so two callers
    // cannot both find the slot empty and race for the socket. See
    // `docs/adr/0068-oauth-callback-port-has-one-owner.md`.
    let mut active_flow = ACTIVE_CALLBACK_FLOW.lock().await;
    if release_callback_port(&mut active_flow).await {
        crate::log!(
            "[OAuth] Superseded an authorization still waiting on port {}",
            CALLBACK_PORT
        );
    }

    // Bind BEFORE returning the URL, so the callback cannot arrive before
    // anything is listening for it.
    let listener = CallbackListener::bind(CALLBACK_PORT)
        .await
        .map_err(|e| callback_bind_error(CALLBACK_PORT, e))?;

    // From the same `redirect_uri` the exchange below will send.
    let auth_request_url = build_authorize_url(
        &auth_url,
        &client_id,
        &redirect_uri,
        &merged_scopes,
        &state,
        &auth,
        &authorize_params,
    );

    crate::log!(
        "[OAuth] Prepared {} authorization URL ({} client), listener on port {}, redirect_uri {}",
        provider,
        auth.kind(),
        CALLBACK_PORT,
        redirect_uri
    );

    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let pool = pool.clone();
    let event_bus = event_bus.clone();
    let provider = provider.to_string();
    let initiator = initiator.clone();
    let holds_port = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let task_holds_port = holds_port.clone();

    let task = tokio::spawn(async move {
        let result = async {
            let waited = tokio::time::timeout(
                std::time::Duration::from_secs(120),
                wait_for_oauth_callback(listener, &provider, &state),
            )
            .await;

            // The future owns the listener, so both the completed and the
            // timed-out path have already closed the sockets. Publish that
            // BEFORE the token exchange, which is a network round trip and must
            // not be abortable by a supersede.
            task_holds_port.store(false, std::sync::atomic::Ordering::Release);

            let code = waited
                .map_err(|_| "OAuth authorization timed out after 120 seconds".to_string())?
                .map_err(|e| format!("OAuth callback error: {}", e))?;

            // `exchange_code` already names the leg and carries the provider's
            // own error text, so it is NOT re-wrapped.
            let token_resp = exchange_code(&token_url, &client_id, &auth, &code, &redirect_uri)
                .await
                .map_err(|e| e.to_string())?;

            let (email, display_name) = if let Some(ref url) = userinfo_url {
                fetch_userinfo(url, &token_resp.access_token, userinfo_method).await
            } else {
                (None, None)
            };

            let token_expiry = token_resp
                .expires_in
                .map(|secs| chrono::Utc::now() + chrono::Duration::seconds(secs as i64));

            // What the provider granted, falling back to what was requested.
            let granted_scopes = token_resp.scope.as_deref().unwrap_or(&merged_scopes);

            // Both scope sets are stored. Their difference IS the shortfall a
            // later *Reconnect* re-requests.
            OAuthStore::connect(
                &pool,
                &event_bus,
                &provider,
                email.as_deref(),
                display_name.as_deref(),
                &token_resp.access_token,
                token_resp.refresh_token.as_deref(),
                token_expiry,
                granted_scopes,
                &merged_scopes,
                initiator,
            )
            .await
            .map_err(|e| format!("Failed to store OAuth account: {}", e))?;

            crate::log!(
                "[OAuth] Connected {} account: {} (scopes: {})",
                provider,
                email.as_deref().unwrap_or("unknown"),
                granted_scopes
            );

            Ok(OAuthFlowOutcome {
                email,
                display_name,
                granted_scopes: granted_scopes.to_string(),
                // The set the authorization URL was built from. Anything
                // derived later from the stored account could disagree.
                requested_scopes: merged_scopes.clone(),
            })
        }
        .await;

        let _ = result_tx.send(result);
    });

    // Register the new owner before releasing the lock, so the next flow has
    // something to supersede and the port is never orphaned again.
    *active_flow = Some(ActiveCallbackFlow { task, holds_port });
    drop(active_flow);

    Ok(PreparedOAuthFlow {
        auth_url: auth_request_url,
        result_rx,
    })
}

/// Run the full OAuth flow end-to-end, handing `auth_url` to `open_auth_url` at
/// the moment the loopback listener is already bound.
///
/// **The engine does not open browsers.** Deciding where a URL is displayed
/// belongs to the client, which knows the platform and the user's
/// in-app-browser preference, so the caller supplies an opener.
///
/// The opener runs AFTER `prepare_oauth_flow` has bound the listener, so the
/// callback can never arrive before something is listening for it.
pub async fn run_oauth_flow<F>(
    pool: &PgPool,
    event_bus: &EventBus,
    provider: &str,
    scopes: &str,
    initiator: Option<MessageOrigin>,
    open_auth_url: F,
) -> Result<OAuthFlowOutcome, BoxError>
where
    F: AsyncFnOnce(&str) -> Result<(), BoxError>,
{
    let prepared = prepare_oauth_flow(pool, event_bus, provider, scopes, initiator).await?;

    crate::log!(
        "[OAuth] Handing {} authorization URL to the client to open",
        provider
    );
    // A failed hand-off is fatal to the flow, not best-effort: nothing will
    // ever reach the callback, so waiting out the timeout would only turn a
    // precise error into "authorization timed out".
    open_auth_url(&prepared.auth_url).await?;

    prepared
        .result_rx
        .await
        .map_err(|_| FLOW_SUPERSEDED_MSG)?
        .map_err(|e| e.into())
}

/// Whether a provider's userinfo endpoint is fetched with GET or POST.
///
/// GET is the OIDC norm and the default, so a credential that omits the key
/// keeps working untouched. The alternative exists because some providers serve
/// their userinfo equivalent as POST-only.
///
/// Read from the credential's optional `userinfo_method` key. Chosen explicitly
/// rather than sniffed from a 400-then-retry: a silent method fallback would
/// hide a genuinely broken endpoint behind a second request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserinfoMethod {
    Get,
    Post,
}

impl UserinfoMethod {
    /// Parse the credential's `userinfo_method`. Absent, blank, or unrecognized
    /// all mean GET: the endpoint's method is not worth failing a completed
    /// authorization over, and every pre-existing credential omits the key.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim).unwrap_or_default().to_ascii_uppercase() {
            m if m == "POST" => Self::Post,
            _ => Self::Get,
        }
    }
}

/// The display name in a userinfo response.
///
/// Two shapes in the wild: OIDC's flat `"name": "Jane Doe"`, and a nested
/// object carrying `display_name`. Read both, or the nested case yields no
/// name at all.
fn userinfo_display_name(body: &serde_json::Value) -> Option<String> {
    let name = body.get("name")?;
    if let Some(flat) = name.as_str() {
        return Some(flat.to_string());
    }
    name.get("display_name")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Fetch `(email, display_name)` from a userinfo endpoint.
///
/// Best-effort: any error is logged and downgraded to `(None, None)`, so the
/// OAuth flow completes without this optional metadata.
async fn fetch_userinfo(
    userinfo_url: &str,
    access_token: &str,
    method: UserinfoMethod,
) -> (Option<String>, Option<String>) {
    let client = bounded_http_client();
    let request = match method {
        UserinfoMethod::Get => client.get(userinfo_url),
        // No body and no `Content-Type`, deliberately. A JSON content type with
        // an empty body is rejected, and so is `{}`. Omitting the header is
        // both what such endpoints accept and the most neutral thing to send.
        UserinfoMethod::Post => client.post(userinfo_url),
    };
    let resp = match request
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            crate::log!(@OAuth, "Userinfo fetch failed: {}", e);
            return (None, None);
        }
    };

    if !resp.status().is_success() {
        crate::log!(@OAuth, "Userinfo returned status {}", resp.status());
        return (None, None);
    }

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            crate::log!(@OAuth, "Userinfo parse failed: {}", e);
            return (None, None);
        }
    };

    let email = body.get("email").and_then(|v| v.as_str()).map(String::from);
    let name = userinfo_display_name(&body);

    (email, name)
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
