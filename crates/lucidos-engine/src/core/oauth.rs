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
    pub scopes: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// Manual `Debug` so an account never leaks its tokens through `{:?}` in a log
// line or error message. The token *presence* (Some/None for the refresh
// token) stays visible because it's useful for debugging without exposing the
// secret value itself.
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
    pub scopes: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// OAuth client credential requests
// ---------------------------------------------------------------------------
//
// The engine ships NO hardcoded endpoint table. The registry of known OAuth
// providers — auth/token/userinfo URLs, typical scopes, and alias rules like
// "ghealth" -> Google's endpoints — lives in `system-knowhow/oauth-providers.md`,
// which the agent reads and maintains. When the agent requests an `oauth_client`
// credential it passes the endpoints it looked up via `OAuthClientOverrides`;
// they pre-fill the modal and are persisted into the per-credential JSON, which
// is the single source of truth for endpoints. Both `prepare_oauth_flow` and
// `refresh_oauth_if_needed` read the URLs back from that JSON; legacy rows for
// the formerly-hardcoded providers are backfilled by migration
// `20260531..._backfill_oauth_endpoint_urls.sql`.

/// Caller-supplied OAuth endpoint + base/scopes values used to pre-fill the
/// credential modal. Every field is optional; whatever is present lands in the
/// request's `defaults` block, which is what tells the modal to pre-fill (and
/// stop requiring) the endpoint fields. All-`None` => no `defaults` block, so
/// the modal expands its endpoint section for manual entry.
#[derive(Debug, Default, Clone)]
pub struct OAuthClientOverrides {
    pub base_url: Option<String>,
    pub auth_url: Option<String>,
    pub token_url: Option<String>,
    pub userinfo_url: Option<String>,
    pub scopes: Option<String>,
    /// Loopback callback URI to register with this provider. Omit for the
    /// default (`127.0.0.1`); supply the `localhost` form only for a provider
    /// that won't accept the IP literal. See `default_redirect_uri` and the
    /// oauth-providers knowhow.
    pub redirect_uri: Option<String>,
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
        "service": format!("oauth:{provider}"),
        "prompt": format!("Enter your OAuth client credentials for {provider}."),
        "base_url": base_url,
        "auth_type": "oauth_client",
    });
    // Only the keys the caller actually supplied go into `defaults` — never as
    // present-but-null — so the modal can treat "key absent" as "not pre-filled".
    let mut defaults = serde_json::Map::new();
    for (key, value) in [
        ("auth_url", &overrides.auth_url),
        ("token_url", &overrides.token_url),
        ("userinfo_url", &overrides.userinfo_url),
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
/// Same shape as `RepositoryStore`; see `core::announced_surfaces`.
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
    ) -> Result<Uuid, sqlx::Error> {
        let result = if email.is_some() {
            sqlx::query_scalar::<_, Uuid>(
                r#"
                INSERT INTO oauth_accounts (provider, email, display_name, access_token, refresh_token, token_expiry, scopes)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (provider, email) DO UPDATE SET
                    display_name = EXCLUDED.display_name,
                    access_token = EXCLUDED.access_token,
                    refresh_token = COALESCE(EXCLUDED.refresh_token, oauth_accounts.refresh_token),
                    token_expiry = EXCLUDED.token_expiry,
                    scopes = EXCLUDED.scopes,
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
            .fetch_one(pool)
            .await?
        } else {
            sqlx::query_scalar::<_, Uuid>(
                r#"
                INSERT INTO oauth_accounts (provider, email, display_name, access_token, refresh_token, token_expiry, scopes)
                VALUES ($1, NULL, $2, $3, $4, $5, $6)
                ON CONFLICT (provider) WHERE email IS NULL DO UPDATE SET
                    display_name = EXCLUDED.display_name,
                    access_token = EXCLUDED.access_token,
                    refresh_token = COALESCE(EXCLUDED.refresh_token, oauth_accounts.refresh_token),
                    token_expiry = EXCLUDED.token_expiry,
                    scopes = EXCLUDED.scopes,
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
                   refresh_token, token_expiry, scopes,
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
    /// CONNECTED one (`created_at DESC`).
    ///
    /// When two accounts share a provider (e.g. an old narrow-scope `drive.file`
    /// connection plus a newer full-`drive` one), a fresh connect must win —
    /// otherwise the stale connection permanently shadows the broader token and
    /// re-connecting can never take effect (the cause of the silent
    /// `403 insufficient authentication scopes` on shared-drive endpoints).
    ///
    /// `created_at` (not `updated_at`) is the right key: a re-connect of an
    /// existing account is an in-place upsert that leaves `created_at` alone,
    /// while `updated_at` is also bumped by every token refresh — so
    /// `updated_at DESC` would keep whichever account was last *used* winning,
    /// which is exactly the stale account in the buggy state this fixes.
    pub async fn get_by_provider(
        pool: &PgPool,
        provider: &str,
    ) -> Result<Option<OAuthAccount>, sqlx::Error> {
        sqlx::query_as::<_, OAuthAccount>(
            r#"
            SELECT id, provider, email, display_name, access_token,
                   refresh_token, token_expiry, scopes,
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

    /// Update tokens after a refresh
    /// Refresh an account's tokens in place.
    ///
    /// Deliberately silent, and registered as the one `oauth_accounts`
    /// exemption in `core::announced_surfaces`: a token rotation changes
    /// nothing the user can see, and announcing every refresh would put an
    /// events row on the timeline each time a token neared expiry.
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
            SELECT id, provider, email, display_name, scopes,
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
                   refresh_token, token_expiry, scopes,
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
    /// This emit is the half that was missing: `OAuthAccountDeleted` already
    /// existed and the frontend reloads its Accounts list on it, so
    /// disconnecting refreshed every client while connecting refreshed none,
    /// and nothing recorded that an account had been connected at all.
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

/// Why `get_account_with_fresh_token` couldn't return a usable account.
/// Each caller maps these to its preferred HTTP status / error message
/// (the proxy script-handshake layer maps `NotConnected` to BAD_GATEWAY,
/// the access-token endpoint maps it to NOT_FOUND).
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
/// For each account: `OAUTH_{PROVIDER}_ACCESS_TOKEN` (always),
/// `OAUTH_{PROVIDER}_EMAIL` (if known). Provider name is uppercased and
/// `-` / `.` / space → `_` so a hyphenated provider lands as a legal
/// identifier in shell.
///
/// Used by both subprocess injection (`build_script_env_vars` for
/// run_python / run_bash / scheduled scripts) and the proxy
/// `script_handshake` layer's `oauth_providers` field, so the env-var
/// names stay identical across all entry points.
pub fn account_env_vars(accounts: Vec<OAuthAccount>) -> Vec<(String, String)> {
    let mut env_vars = Vec::new();
    for account in accounts {
        let provider = account
            .provider
            .to_uppercase()
            .replace(['-', ' ', '.'], "_");
        let prefix = format!("OAUTH_{}", provider);
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
// `redirect_uri` (OAuth 2.0 §4.1.3), and it must match what the provider has
// registered. Both legs therefore read the ONE value `resolve_redirect_uri`
// returns — never a rebuilt literal.

/// Port the temporary callback listener binds. Fixed (not ephemeral) because
/// the URI has to be registered with the provider ahead of time.
const CALLBACK_PORT: u16 = 14981;
/// Path the callback listener answers on.
const CALLBACK_PATH: &str = "/oauth/callback";
/// Host form advertised unless the credential overrides it. The loopback IP is
/// the default because some providers reject the name `localhost` outright.
const DEFAULT_CALLBACK_HOST: &str = "127.0.0.1";
/// Every loopback host form the listener can actually receive on — it binds
/// both IPv4 and IPv6 loopback, and `localhost` resolves to one of them. These
/// are the only values a credential's `redirect_uri` may take: the port and
/// path belong to the listener, so any other value can only produce a flow that
/// hangs until the timeout.
///
/// Which form a provider will accept is a *provider* fact and lives in
/// `system-knowhow/oauth-providers.md`, not here.
const CALLBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];

fn callback_uri_for_host(host: &str) -> String {
    format!("http://{}:{}{}", host, CALLBACK_PORT, CALLBACK_PATH)
}

/// The redirect URI advertised when a credential doesn't override it.
fn default_redirect_uri() -> String {
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
    /// 32 random bytes base64url-encoded — 43 characters, all from RFC 7636's
    /// unreserved set, which is the low end of the spec's 43..=128 range and
    /// the recommended entropy.
    fn generate() -> Self {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let verifier = base64_url_nopad(&bytes);
        let challenge = Self::challenge_for(&verifier);
        Self {
            verifier,
            challenge,
        }
    }

    /// `BASE64URL-NOPAD(SHA256(ASCII(verifier)))` — the `S256` method.
    fn challenge_for(verifier: &str) -> String {
        use sha2::{Digest, Sha256};
        base64_url_nopad(&Sha256::digest(verifier.as_bytes()))
    }
}

fn base64_url_nopad(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// How the client proves itself when redeeming the authorization code.
///
/// Derived SOLELY from whether the credential carries a `client_secret` — this
/// is OAuth's own confidential/public distinction, and keying on it means the
/// engine never needs to know which provider it is talking to.
///
/// A desktop app that ships no secret is a *public* client (RFC 8252), and the
/// redemption is authenticated with PKCE instead. Providers reject a secret
/// from a public client and reject a secret-less redemption from a confidential
/// one, so the two shapes must not be mixed.
enum ClientAuth {
    /// The credential has a `client_secret`: send it, and send no PKCE. This is
    /// the shape every existing Lucidos connection uses.
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

    /// Label for logs — never includes the secret or the verifier.
    fn kind(&self) -> &'static str {
        match self {
            Self::Confidential(_) => "confidential",
            Self::Public(_) => "public+PKCE",
        }
    }
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
    auth: &ClientAuth,
) -> String {
    // Some authorization endpoints already carry a query (Azure AD B2C pins its
    // user flow with `?p=…`), so appending a second `?` would corrupt the URL.
    let separator = if auth_url.contains('?') { '&' } else { '?' };
    let mut url = format!(
        "{}{}client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent",
        auth_url,
        separator,
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(scopes),
    );
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

/// Bounded, shared HTTP client for the OAuth token/userinfo endpoints. A bare
/// `reqwest::Client::new()` has NO timeout, and `refresh_oauth_if_needed` runs
/// on the email-send path — a stalled identity provider would hang the whole
/// send request. Same unbounded-network-op class as `SMTP_SEND_TIMEOUT` /
/// `IMAP_OP_TIMEOUT` in `core/email*.rs`. Built once (`LazyLock`) so repeated
/// calls reuse the connection pool and the rustls context — the repo precedent
/// for process-wide clients (`api/proxy.rs`, `engine/http/workspace_client.rs`).
fn bounded_http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            // Only fails on TLS-backend init — where the old bare
            // `Client::new()` would panic identically.
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
/// Microsoft adds a numeric `error_codes` array (the `AADSTS…` numbers). Pulling
/// those out is the difference between "Token exchange failed (400)" and
/// "invalid_request — AADSTS90023: The provided value for the input parameter
/// 'redirect_uri' is not valid", which is the only thing that makes this flow
/// debuggable from the UI. A body that isn't an OAuth error object (HTML error
/// page, empty response) falls back to the raw text rather than being dropped.
///
/// `operation` names the leg ("Token exchange" / "Token refresh") so the caller
/// does NOT re-wrap the result — a second prefix produced the unreadable
/// "Token exchange failed: Token exchange failed (400): {…}".
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
///
/// The confidential ordering (grant_type, code, client_id, client_secret,
/// redirect_uri) is preserved exactly as it has always been sent; the public
/// variant drops `client_secret` and appends `code_verifier`.
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
/// PKCE has no role here — it authenticates the one-time code redemption, not
/// later refreshes — so this takes the secret directly. A public client must
/// omit `client_secret` on the refresh too, or the provider rejects the request
/// the same way it would reject the redemption.
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
        // No expiry stored but refresh token exists — can't verify validity,
        // so refresh proactively. Providers without expiring tokens (e.g. GitHub)
        // also lack refresh tokens, so this won't trigger for them.
        None => account.refresh_token.is_some(),
    }
}

/// Check if an OAuth account's token is expired (or expiring within 60s),
/// and if so, look up client credentials, refresh the token, and persist
/// the new tokens. Returns the valid access token.
///
/// Mutates `account.access_token` in place on successful refresh.
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

    let cred_service = format!("oauth:{}", account.provider);
    let client_cred = super::CredentialStore::get(pool, &cred_service)
        .await?
        .ok_or_else(|| format!("No client credentials found for {}", cred_service))?;

    let config: serde_json::Value = serde_json::from_str(&client_cred.auth_value)?;
    let cid = config["client_id"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Missing client_id in {} credentials", cred_service))?;
    // Blank/absent => public client: the refresh must omit `client_secret` too,
    // exactly as the code redemption did.
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

/// Merge existing scopes with requested scopes, deduplicating.
/// Scopes are space-separated strings.
fn merge_scopes(existing: &str, requested: &str) -> String {
    let mut all: Vec<&str> = existing.split_whitespace().collect();
    for scope in requested.split_whitespace() {
        if !all.contains(&scope) {
            all.push(scope);
        }
    }
    all.join(" ")
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
    async fn bind(port: u16) -> Result<Self, BoxError> {
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

/// Cap on the buffered request line. Authorization codes run to kilobytes
/// (Microsoft's especially), so the ceiling is generous — it exists only to
/// stop a client that never sends a newline from growing the buffer forever.
const MAX_REQUEST_LINE_BYTES: usize = 64 * 1024;

/// How long one connection gets to send its request line before we abandon it
/// and go back to waiting.
///
/// A browser sends immediately after connecting, but a speculative preconnect
/// can open a socket and hold it idle. Without this bound, that idle socket
/// would block the real redirect sitting behind it in the accept queue until
/// the caller's 120s timeout killed the whole flow.
const CALLBACK_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Read the HTTP request line — everything before the first newline.
///
/// This MUST loop: a single read returns only whatever has arrived so far, and
/// the authorization code sits in the request line, so a code split across TCP
/// reads (or one longer than a fixed buffer) is silently truncated by a
/// read-once implementation. A truncated code is then rejected by the provider
/// as a malformed request, long after the browser reported success.
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
            break; // EOF without a newline — use whatever arrived.
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
/// human-readable text (`error_description`) with `+` for space, but an
/// authorization code is an opaque RFC 3986 query value where `+` is a literal
/// character — decoding it as a space would corrupt the code.
fn query_param(query: &str, key: &str, plus_is_space: bool) -> Option<String> {
    let raw = query
        .split('&')
        .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))?;
    let raw = if plus_is_space {
        raw.replace('+', " ")
    } else {
        raw.to_string()
    };
    // A malformed escape shouldn't hide the value — fall back to the raw text.
    Some(
        urlencoding::decode(&raw)
            .map(|decoded| decoded.into_owned())
            .unwrap_or(raw),
    )
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
/// caller — but log it, so a regression here surfaces.
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

/// Wait for the provider's redirect and extract the authorization code.
async fn wait_for_oauth_callback(listener: CallbackListener) -> Result<String, BoxError> {
    loop {
        let mut stream = listener.accept().await?;
        // A connection that misbehaves is discarded, never fatal — a reset or
        // idle preconnect must not fail an authorization the user completed.
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
        // probes, speculative preconnects). Treating one of those as the
        // callback would consume the accept and strand the flow until it times
        // out, so answer them and keep waiting — the caller's timeout bounds
        // this loop.
        if path != CALLBACK_PATH || query.is_empty() {
            respond_to_browser(&mut stream, "404 Not Found", "<html><body></body></html>").await;
            continue;
        }

        let result = parse_callback_query(query);
        let (status, body) = match result {
            Ok(_) => (
                "200 OK",
                "<html><body><h2>Authorization successful!</h2>\
                 <p>You can close this tab and return to Lucidos.</p></body></html>",
            ),
            // The provider's reason goes to the engine, not into this page —
            // echoing an attacker-controllable query value into HTML we serve
            // would be an injection sink for no benefit.
            Err(_) => (
                "400 Bad Request",
                "<html><body><h2>Authorization failed</h2>\
                 <p>Return to Lucidos for the details.</p></body></html>",
            ),
        };
        respond_to_browser(&mut stream, status, body).await;
        return result;
    }
}

/// Outcome of an OAuth token exchange: (email, display_name, scopes).
pub type OAuthFlowResult = Result<(Option<String>, Option<String>, String), String>;

/// Result of preparing an OAuth flow — contains the auth URL and a receiver
/// that resolves when the background flow completes.
pub struct PreparedOAuthFlow {
    pub auth_url: String,
    pub result_rx: tokio::sync::oneshot::Receiver<OAuthFlowResult>,
}

/// Prepare an OAuth flow: look up client credentials, bind the callback listener,
/// build the authorization URL, and spawn a background task that waits for the
/// callback, exchanges the code, and stores the account.
///
/// The caller is responsible for opening `auth_url` (e.g. in the user's browser).
/// Await `result_rx` to get the flow outcome.
pub async fn prepare_oauth_flow(
    pool: &PgPool,
    event_bus: &EventBus,
    provider: &str,
    scopes: &str,
) -> Result<PreparedOAuthFlow, BoxError> {
    use crate::core::CredentialStore;

    // Merge requested scopes with any existing scopes for this provider
    let existing_account = OAuthStore::get_by_provider(pool, provider).await?;
    let merged_scopes = if let Some(ref acct) = existing_account {
        merge_scopes(&acct.scopes, scopes)
    } else {
        scopes.to_string()
    };

    // Look up client credentials
    let cred_service = format!("oauth:{}", provider);
    let client_cred = CredentialStore::get(pool, &cred_service)
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
    // A blank/absent client_secret is not an error — it is how a *public*
    // client is expressed (see `ClientAuth`). A desktop app that can't keep a
    // secret authenticates the redemption with PKCE instead.
    let auth = ClientAuth::from_secret(client_config["client_secret"].as_str());

    // Endpoints come from the per-credential JSON — the single source of truth.
    // The agent pre-fills them from `system-knowhow/oauth-providers.md` at
    // request time; legacy rows are backfilled by migration.
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

    // The ONE redirect URI for this flow. Resolved before anything else so a
    // bad override fails fast instead of producing a browser redirect the
    // listener never receives.
    let redirect_uri = resolve_redirect_uri(&client_config)?;

    // Start the temporary loopback listener BEFORE returning the URL, so the
    // callback can't arrive before we're listening.
    let listener = CallbackListener::bind(CALLBACK_PORT).await?;

    // Build authorization URL with merged scopes, from the same redirect_uri
    // the exchange below will send.
    let auth_request_url =
        build_authorize_url(&auth_url, &client_id, &redirect_uri, &merged_scopes, &auth);

    crate::log!(
        "[OAuth] Prepared {} authorization URL ({} client), listener on port {}, redirect_uri {}",
        provider,
        auth.kind(),
        CALLBACK_PORT,
        redirect_uri
    );

    // Spawn background task to wait for callback and complete the exchange
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let pool = pool.clone();
    let event_bus = event_bus.clone();
    let provider = provider.to_string();

    tokio::spawn(async move {
        let result = async {
            // Wait for callback (with 120s timeout)
            let code = tokio::time::timeout(
                std::time::Duration::from_secs(120),
                wait_for_oauth_callback(listener),
            )
            .await
            .map_err(|_| "OAuth authorization timed out after 120 seconds".to_string())?
            .map_err(|e| format!("OAuth callback error: {}", e))?;

            // Exchange code for tokens. `exchange_code` already names the leg
            // and carries the provider's own error text, so it is NOT re-wrapped.
            let token_resp = exchange_code(&token_url, &client_id, &auth, &code, &redirect_uri)
                .await
                .map_err(|e| e.to_string())?;

            // Fetch userinfo (best-effort — failures downgrade to None inside)
            let (email, display_name) = if let Some(ref url) = userinfo_url {
                fetch_userinfo(url, &token_resp.access_token).await
            } else {
                (None, None)
            };

            // Calculate token expiry
            let token_expiry = token_resp
                .expires_in
                .map(|secs| chrono::Utc::now() + chrono::Duration::seconds(secs as i64));

            // Use actually granted scopes from the token response, fall back to what we requested
            let granted_scopes = token_resp.scope.as_deref().unwrap_or(&merged_scopes);

            // Store account with granted scopes. `connect` announces
            // OAuthAccountConnected from inside the write path, so every device
            // reloads its Accounts list without waiting for a page refresh.
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
                None,
            )
            .await
            .map_err(|e| format!("Failed to store OAuth account: {}", e))?;

            crate::log!(
                "[OAuth] Connected {} account: {} (scopes: {})",
                provider,
                email.as_deref().unwrap_or("unknown"),
                granted_scopes
            );

            Ok((email, display_name, granted_scopes.to_string()))
        }
        .await;

        let _ = result_tx.send(result);
    });

    Ok(PreparedOAuthFlow {
        auth_url: auth_request_url,
        result_rx,
    })
}

/// Run the full OAuth flow end-to-end (used by LLM tool calls where the backend
/// opens the browser directly). Calls `prepare_oauth_flow` then opens the URL.
pub async fn run_oauth_flow(
    pool: &PgPool,
    event_bus: &EventBus,
    provider: &str,
    scopes: &str,
) -> Result<(Option<String>, Option<String>, String), BoxError> {
    let prepared = prepare_oauth_flow(pool, event_bus, provider, scopes).await?;

    // Open browser (macOS — only used from LLM tool calls, not from frontend)
    crate::log!("[OAuth] Opening browser for {} authorization", provider);
    if let Err(e) = std::process::Command::new("open")
        .arg(&prepared.auth_url)
        .spawn()
    {
        crate::log!("[OAuth] Failed to spawn browser via 'open': {}", e);
    }

    // Wait for the background task to complete
    prepared
        .result_rx
        .await
        .map_err(|_| "OAuth flow task was dropped")?
        .map_err(|e| e.into())
}

/// Fetch user info (email, name) from a userinfo endpoint.
/// Returns (email, display_name). Best-effort: any error along the way
/// (network, non-success status, JSON parse) is logged and downgraded to
/// `(None, None)` so the OAuth flow can complete without optional metadata.
async fn fetch_userinfo(
    userinfo_url: &str,
    access_token: &str,
) -> (Option<String>, Option<String>) {
    let client = bounded_http_client();
    let resp = match client
        .get(userinfo_url)
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
    let name = body.get("name").and_then(|v| v.as_str()).map(String::from);

    (email, name)
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
