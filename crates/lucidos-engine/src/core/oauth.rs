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
    /// This is what *Reconnect* re-requests. Re-requesting `scopes` instead
    /// could only ever ask for the set the account already held, because
    /// `prepare_oauth_flow` merges the request with the existing grant, so an
    /// account a provider had narrowed could never recover the difference. That
    /// is the button the engine's own Dropbox permission error sends the user
    /// to.
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
/// This is what remains of `client_service_name`, which used to manufacture the
/// name `oauth:<provider>` because `credentials.service_name` was the table's
/// only unique key and a bare `google` could not be both an API key and an app
/// registration. `auth_type` is the discriminator now
/// (`20260805134838_drop_credential_name_prefixes_use_auth_type.sql`), so the
/// name is just the provider and the whole canonicalization apparatus is gone.
///
/// Two things survive, and both are about a caller getting it wrong rather than
/// about the key:
///
/// * **Lowercasing**, so `Dropbox` and `dropbox` cannot address two
///   registrations. `connect_oauth_account` already lowercases its argument.
/// * **Stripping a leading `oauth:`**, because agents and knowhow in the wild
///   still say `oauth:<provider>`: the chat system prompt said it for as long as
///   the tool has existed, and on 2026-08-05 an agent passed a bare `dropbox`
///   even so. A caller passing either spelling must land on the same row rather
///   than create a second one, which was the incident.
pub fn client_provider_name(name: &str) -> String {
    let name = name.trim().to_lowercase();
    // A bare `oauth:` strips to nothing, and an empty service name is not a
    // credential anyone can address. Keep the input so the caller's own
    // emptiness check (or the store's NOT NULL) rejects it visibly, rather than
    // manufacturing a row named "".
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
    // Only the keys the caller actually supplied go into `defaults` — never as
    // present-but-null — so the modal can treat "key absent" as "not pre-filled".
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
    /// `scopes` comes from the caller, not the row, because the scope set is a
    /// property of what the connection is FOR (a backup, a mailbox) rather than
    /// of the provider.
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
/// reads them, so a caller can tell "this credential cannot drive a flow" from
/// "this flow failed".
const REQUIRED_FLOW_FIELDS: [&str; 3] = ["client_id", "auth_url", "token_url"];

/// Which required fields a stored `oauth_client` secret is missing.
///
/// Empty means the credential can drive a flow. Anything else is the list the
/// user is about to be shown, because reaching `prepare_oauth_flow` in this
/// state produces a bare *"Missing auth_url in OAuth credentials"* toast with no
/// way forward: the endpoint inputs carried no `required` attribute and the form
/// only pair-validated them, so a credential saved with both blank was accepted
/// and failed on the NEXT press of Connect, one screen away from the cause.
///
/// A secret that is not a JSON object counts as missing everything. There is no
/// recoverable client id inside an unparseable blob, and reopening the form
/// prefilled from the registry is a better answer than a toast about JSON.
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
/// Same modal, same registry prefill, two additions the create path has no use
/// for: `existing_credential_id`, so the save updates the row instead of
/// creating a second one for the same provider (a name plus an auth type is the
/// credential's identity, and a duplicate pair is the 2026-08-05 incident), and
/// `missing`, so the form can say which fields it reopened for. The stored
/// `client_id` rides along in `defaults` because the user already supplied it
/// and asking twice is how a repair starts feeling like a punishment.
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

/// "a" / "a and b" / "a, b and c". Used only for the repair prompt.
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
/// `OAUTH_{PROVIDER}_EMAIL` (if known). The provider name goes through
/// [`crate::core::env_var_segment`], the same transform `CRED_*` uses, so any
/// provider lands as a legal identifier in shell.
///
/// Used by both subprocess injection (`build_script_env_vars` for
/// run_python / run_bash / scheduled scripts) and the proxy
/// `script_handshake` layer's `oauth_providers` field, so the env-var
/// names stay identical across all entry points.
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
    /// 32 random bytes base64url-encoded — 43 characters, all from RFC 7636's
    /// unreserved set, which is the low end of the spec's 43..=128 range and
    /// the recommended entropy.
    fn generate() -> Self {
        let verifier = random_url_token();
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

/// 32 CSPRNG bytes, base64url-encoded to 43 characters that need no escaping in
/// a URL. The shared generator behind both unguessable values this module puts
/// on an authorization request: the PKCE verifier (RFC 7636 wants 43..=128 from
/// the unreserved set, and 32 bytes is its recommended entropy) and the `state`
/// nonce.
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
/// It does two jobs here, and the second is why it is not optional.
///
/// 1. **CSRF.** The callback listener is a plain loopback socket, so during the
///    flow's window ANY local process, and any web page the user happens to be
///    browsing, can issue `GET 127.0.0.1:<port>/oauth/callback?code=...` and have
///    the engine redeem a code it did not ask for. Requiring the value back is
///    the standard defense.
/// 2. **Identity.** A new authorization supersedes the previous one (see
///    [`release_callback_port`]), so a redirect from an abandoned flow can arrive
///    at a listener that never issued it. Without `state` the two are
///    indistinguishable and the stale code would be redeemed against the new
///    flow's client and PKCE verifier, failing in a way that names nothing.
///
/// Compared with `==` rather than in constant time on purpose: the value is not
/// a secret the attacker is trying to *guess* one byte at a time, it is a nonce
/// the legitimate redirect carries and a forged one does not.
fn generate_oauth_state() -> String {
    random_url_token()
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

/// The extra authorization-URL parameters a provider needs, beyond the ones the
/// protocol itself defines.
///
/// This exists because "ask for a refresh token" has no standard spelling.
/// Google reads `access_type=offline` (plus `prompt=consent` to re-issue the
/// refresh token on a repeat authorization); Dropbox reads
/// `token_access_type=offline` and returns a four-hour access token and NO
/// refresh token without it. Those two strings were a single hardcoded literal
/// until 2026-08-05, which meant every Dropbox connection was silently
/// unrefreshable: `refresh_oauth_if_needed` could only report "OAuth token
/// expired but no refresh token available", so a scheduled backup worked on the
/// evening it was set up and never again.
///
/// So the value is credential data, one per registration, documented per
/// provider in `system-knowhow/oauth-providers.md`. No provider name appears in
/// this module (CLAUDE.md § "No provider-specific instructions in code"); the
/// registry that knows them is knowhow the agent reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthorizeParams(Vec<(String, String)>);

/// What a credential with no `authorize_params` sends. Google's spelling,
/// because every credential stored before the key existed was authorized with
/// exactly this and Google needs both halves to re-issue a refresh token.
/// Absent must therefore mean "unchanged", never "nothing".
pub const DEFAULT_AUTHORIZE_PARAMS: &str = "access_type=offline&prompt=consent";

/// The opt-out, for a provider strict enough to reject a parameter it does not
/// know. Without it, [`DEFAULT_AUTHORIZE_PARAMS`] would be unavoidable.
const AUTHORIZE_PARAMS_NONE: &str = "none";

/// Parameters the flow itself owns. A credential is agent- and user-writable, so
/// letting it set these would let a stored value rewrite the loopback
/// `redirect_uri` the callback listener is bound to, or narrow the `scope` the
/// caller asked for, from a field that reads like provider trivia.
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
    /// means send nothing extra. Both halves of each pair are percent-decoded
    /// here and re-encoded on the way out, so a value carrying `&` or `=`
    /// survives the round trip as one value instead of splitting into further
    /// parameters.
    ///
    /// Errors rather than dropping a bad pair: this runs before the browser
    /// opens, and a silently ignored parameter would surface as a provider
    /// behaving inexplicably (no refresh token, an unexpected consent screen)
    /// with nothing to point at.
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

/// Percent-decode one half of an `authorize_params` pair, leaving it as written
/// when it is not valid encoding (a bare `%` is far likelier to be a literal
/// than a typo worth failing the whole flow over).
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

/// Which scopes an authorization asked for and did not get, in the order they
/// were requested.
///
/// **Exact token set difference**, not containment: both sides split on
/// whitespace and a requested scope is missing iff no granted token equals it.
/// That is deliberately the same rule as `missingScopes` in
/// `components/settings/oauthConnectForm.ts`, which drives the shortfall line on
/// the account row, so the agent and the Accounts panel cannot disagree about
/// whether one account is short.
///
/// **Not the same question as [`crate::core::backup::missing_scopes`]**, which
/// answers whether a backup provider can upload. Its `required_scopes` are
/// substring MATCHERS (Google Drive's whole requirement is the fragment `drive`,
/// which has to match `https://www.googleapis.com/auth/drive.file`), so it uses
/// containment on purpose. Containment here would report a genuinely refused
/// scope as granted whenever another granted scope happened to contain its name.
///
/// An empty requested set yields no shortfall rather than a false one: nothing
/// was asked for, so nothing can be short. That is the same reading the account
/// row takes of a `desired_scopes` that predates the column.
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
/// **Why a process-level slot rather than a field on some caller.** The port is
/// fixed at [`CALLBACK_PORT`] because the redirect URI has to be registered with
/// the provider ahead of time, which makes "at most one authorization in flight
/// per engine" a fact about the world rather than a policy. The owner therefore
/// belongs next to the code that binds it. Putting it in `AppState` would cover
/// the Settings buttons and miss [`run_oauth_flow`], which the agent's
/// `connect_oauth_account` reaches without passing through the API layer, and
/// which the Backup page's own "Ask Lucidos to set this up" button invites.
///
/// **The bug it fixes.** `prepare_oauth_flow` used to `tokio::spawn` the waiter
/// and drop its `JoinHandle` on the spot. The only handle kept anywhere was the
/// result `oneshot::Receiver`, and dropping a receiver does not cancel a task,
/// so an abandoned authorization held the port for its full 120 second timeout
/// with nothing able to reclaim it. Every retry inside that window died at the
/// bind with a bare `Address already in use (os error 48)`. A user who reloaded
/// the page mid-flow hit it every time, because the handler that would have
/// awaited the flow has already removed its map entry by then, leaving the task
/// both unreachable and alive.
static ACTIVE_CALLBACK_FLOW: std::sync::LazyLock<tokio::sync::Mutex<Option<ActiveCallbackFlow>>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(None));

/// The flow registered in [`ACTIVE_CALLBACK_FLOW`], and whether it still holds
/// the port.
///
/// The two are separate because a flow's task OUTLIVES its ownership of the
/// socket: `wait_for_oauth_callback` takes the listener by value, so the port is
/// released the moment the callback lands (or the timeout fires), and everything
/// after that (the token exchange, the userinfo call, the account write) runs
/// with the port already free. Keying the supersede on the task alone would
/// abort that tail for a port nobody was waiting on, cancelling a redemption the
/// user had already completed and, worse, potentially landing between the
/// account row committing and its `OAuthAccountConnected` event.
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
/// The sender drops without sending only when the task went away, and since the
/// callback port has one owner and a new authorization supersedes the previous
/// one, that is what this almost always is: something the user did on purpose,
/// not an internal fault. Shared by BOTH entry points ([`run_oauth_flow`], which
/// the agent's `connect_oauth_account` uses, and the API's `/oauth/complete`),
/// because the supersede fires on either and they must not describe it
/// differently.
pub const FLOW_SUPERSEDED_MSG: &str =
    "This authorization was canceled, most likely because a newer one was started. \
     Start it again if you still need it.";

/// Cancel the flow that owns the callback port, and wait until its socket is
/// actually closed. Reports whether a live flow was superseded.
///
/// **The await is the whole point.** `JoinHandle::abort` only *requests*
/// cancellation, taking effect when the task next yields, so returning straight
/// after it would let the caller's `bind` race the socket's close and
/// reintroduce the exact `EADDRINUSE` this exists to prevent. Awaiting the
/// handle means the task's future has been dropped, and with it the listener's
/// file descriptors, before the caller proceeds. The expected outcome is
/// `Err(JoinError::cancelled)`, which is not a failure and is discarded.
///
/// A flow that has already released the port is **detached, not aborted**: it is
/// not in the caller's way, and killing it would throw away an authorization the
/// user completed. Dropping its `JoinHandle` lets it finish on its own.
///
/// Takes the slot by reference rather than reading [`ACTIVE_CALLBACK_FLOW`]
/// itself so the release-then-rebind guarantee is testable against a
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
/// `AddrInUse` means a *different* process holds the port. On a machine running
/// Lucidos that is almost always another workspace part-way through connecting
/// an account (workspaces run concurrently by design, each with its own engine,
/// and they all share this one machine-wide port). Say that, because the raw
/// `Address already in use (os error 48)` the user was shown names nothing they
/// can act on. Every other error kind keeps its own text.
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

/// Does a callback query carry the `state` this flow sent?
///
/// Pure, so the matching rule is testable without a socket. `plus_is_space` is
/// false because the value is an opaque base64url token, not form-encoded text:
/// decoding `+` as a space would corrupt a legitimate value. A callback with no
/// `state` at all does not match, which is the same verdict as a wrong one:
/// every conforming provider echoes what it was sent (RFC 6749 §4.1.2), so an
/// absent value means the request did not come from our authorization.
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

/// The Lucidos mark, baked in from the one file that defines it.
///
/// `include_str!` rather than a copy, for the same reason `api/sdk.rs` pulls in
/// `shared-components.css` from this same crate: the artwork has one definition
/// and no second copy to keep in step with a rebrand.
///
/// It is embedded rather than fetched because this whole page is (see
/// [`callback_page`]). The file's own `xmlns` rides along, which is the single
/// URL-shaped string on the page: a namespace IDENTIFIER that no user agent
/// resolves, and `callback_page_fetches_nothing` allows exactly it and nothing
/// else.
const BRAND_MARK: &str = include_str!("../../../lucidos-app/public/favicon.svg");

/// The page the provider's redirect lands on.
///
/// This is the last thing the user sees before coming back to Lucidos, and it
/// has to answer "whose page is this?" on sight. It once did not: two unstyled
/// `<h2>`s on a default-white page, which read as a debug stub. A dark surface
/// fixed that much, but on a flat grey that belonged to no product, and a user
/// landing here in 2026-08-06 still had to ask whether the tab was ours or
/// Dropbox's.
///
/// So it wears the surface the *workspace picker* wears (`styles/picker.css`
/// `.ws-picker`): the mark's own radial gradient scaled to fill the viewport,
/// white on brand blue, and the neutral `--font-sans` stack. That is the repo's
/// existing answer to "what does a standalone Lucidos screen look like", and
/// being unmistakably ours is the whole job here.
///
/// It takes the picker's **arrangement** too, which the first pass did not. Four
/// small elements centered both ways in a full viewport of blue read as a splash
/// rather than a page, and the provider id was a bare trailing word that, in the
/// reporting user's words, "needs to explain itself" (2026-08-06, the same
/// install as the surface fix above). So the shell is top-anchored in a bounded
/// left-aligned column (`.ws-picker`'s `align-items:flex-start` and its `4rem`
/// top padding), the mark and a
/// "Lucidos" wordmark sit together as a horizontal lockup the way
/// `.ws-picker-brand` does, and the provider is a labelled key/value row under a
/// hairline: *Authorized with · dropbox*. The label is state-specific, since a
/// completed and a refused authorization say different things about that
/// provider, and a blank provider drops the row entirely rather than ruling off
/// an empty line. The id itself is rendered verbatim: they are `dropbox` and
/// `ghealth`, and title-casing the second one produces "Ghealth", so the label
/// carries the meaning instead.
///
/// **It fetches nothing.** No stylesheet, script, font or image, which is why
/// the CSS is inline and the mark is [`BRAND_MARK`] rather than an `<img>`. Two
/// reasons, both load-bearing: this is served by a one-shot loopback listener
/// that has no engine URL in hand, so a link would trade a certain render for a
/// conditional one at the end of the flow; and a redirect landing page that
/// phones anywhere is a privacy surface. Pinned by
/// `callback_page_fetches_nothing`. The token values are copied from
/// `crates/lucidos-app/src/styles/global/base.css` and `styles/picker.css`, the
/// same bounded duplication `api/sdk_iframe.css` already carries.
///
/// **`provider` is the only interpolated value and it is engine-side** (it comes
/// from the tool call / the credential name, never from the callback query).
/// Nothing the provider sends in the redirect is rendered: echoing an
/// attacker-controllable `error_description` into HTML we serve would be an
/// injection sink for no benefit, so the failure page says "return to Lucidos"
/// and the real reason goes to the engine. Keep it that way.
///
/// Note the timing: this is written at callback receipt, BEFORE the code is
/// exchanged, so it cannot claim the account is connected or name it. "Finishing
/// the connection" is the honest tense.
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
/// `expected_state` is the nonce this flow put on its authorization request (see
/// [`generate_oauth_state`]). A callback that does not echo it back exactly is
/// answered and SKIPPED, never returned and never failed on: it is either a
/// forged request or the redirect of a flow this one superseded, and in both
/// cases the authorization the user is completing right now is still on its way.
/// Failing here instead would let anything that can reach the loopback port
/// cancel a legitimate authorization.
async fn wait_for_oauth_callback(
    listener: CallbackListener,
    provider: &str,
    expected_state: &str,
) -> Result<String, BoxError> {
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

        // Not this flow's redirect: skip it and keep waiting, for the reason on
        // this function. The BROWSER still gets the real failure page rather
        // than the probe's empty body, because the likeliest sender is a human
        // finishing a consent screen this flow superseded, and "nothing was
        // connected" is both true for that tab and exactly what the styled
        // callback page exists to say. Nothing from the query is rendered, so
        // the page's injection contract is unchanged.
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
/// A named struct rather than a tuple because the two scope sets are the whole
/// point of reporting an authorization and are indistinguishable positionally:
/// the caller that used to destructure this bound the granted set as
/// `_merged_scopes` and threw it away, so a provider that refused part of the
/// request was reported to the agent as an unqualified success.
pub struct OAuthFlowOutcome {
    pub email: Option<String>,
    pub display_name: Option<String>,
    /// What the provider actually GRANTED. Falls back to [`Self::requested_scopes`]
    /// when the token response carried no `scope` at all, which is what a
    /// provider that grants exactly what it was asked for typically does.
    pub granted_scopes: String,
    /// What this flow ASKED for: the caller's scopes merged with everything the
    /// account already held and had ever been asked for. The difference from
    /// [`Self::granted_scopes`] is the shortfall
    /// ([`missing_requested_scopes`]).
    pub requested_scopes: String,
}

/// Outcome of an OAuth token exchange, or the reason it failed.
pub type OAuthFlowResult = Result<OAuthFlowOutcome, String>;

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
///
/// `initiator` is the device that started the flow, carried through to the
/// `OAuthAccountConnected` event so the frontend can bring THAT device back to
/// the front when the authorization lands (see `handleOAuthAccountConnected`).
/// `None` for an engine-internal flow, which the frontend reads as "not mine".
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
    // The `desired` half is what makes *Reconnect* able to recover a scope the
    // provider refused. Merging only against the GRANTED set (which is all this
    // did before) meant a reconnect passing that same granted set computed
    // `granted UNION granted`, so an account a provider had narrowed stayed
    // narrow forever, and the engine's own Dropbox permission error pointed the
    // user at exactly that button.
    let existing_account = OAuthStore::get_by_provider(pool, provider).await?;
    let merged_scopes = match existing_account {
        Some(ref acct) => {
            let held = merge_scopes(&acct.scopes, acct.desired_scopes.as_deref().unwrap_or(""));
            merge_scopes(&held, scopes)
        }
        None => scopes.to_string(),
    };

    // Look up client credentials
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
    let userinfo_method = UserinfoMethod::parse(client_config["userinfo_method"].as_str());
    // Resolved here, with the other credential reads, so a malformed value
    // fails before the loopback listener binds and the browser opens.
    let authorize_params = AuthorizeParams::parse(client_config["authorize_params"].as_str())?;

    // The ONE redirect URI for this flow. Resolved before anything else so a
    // bad override fails fast instead of producing a browser redirect the
    // listener never receives.
    let redirect_uri = resolve_redirect_uri(&client_config)?;

    // This flow's nonce. Generated before the listener binds so the URL and the
    // listener are handed the same value by construction.
    let state = generate_oauth_state();

    // The callback port has ONE owner (see `ACTIVE_CALLBACK_FLOW`). Take the
    // lock before releasing the previous flow and hold it past the bind and the
    // spawn, so two callers cannot both find the slot empty and race for the
    // socket. Starting an authorization always supersedes an abandoned one: the
    // user pressing the button is stating what they want now, and the older
    // flow is by construction one whose browser tab they walked away from.
    let mut active_flow = ACTIVE_CALLBACK_FLOW.lock().await;
    if release_callback_port(&mut active_flow).await {
        crate::log!(
            "[OAuth] Superseded an authorization still waiting on port {}",
            CALLBACK_PORT
        );
    }

    // Start the temporary loopback listener BEFORE returning the URL, so the
    // callback can't arrive before we're listening.
    let listener = CallbackListener::bind(CALLBACK_PORT)
        .await
        .map_err(|e| callback_bind_error(CALLBACK_PORT, e))?;

    // Build authorization URL with merged scopes, from the same redirect_uri
    // the exchange below will send.
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

    // Spawn background task to wait for callback and complete the exchange
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let pool = pool.clone();
    let event_bus = event_bus.clone();
    let provider = provider.to_string();
    let initiator = initiator.clone();
    let holds_port = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let task_holds_port = holds_port.clone();

    let task = tokio::spawn(async move {
        let result = async {
            // Wait for callback (with 120s timeout)
            let waited = tokio::time::timeout(
                std::time::Duration::from_secs(120),
                wait_for_oauth_callback(listener, &provider, &state),
            )
            .await;

            // The listener is gone by now whichever way that resolved: the
            // future owns it, so both the completed and the timed-out path have
            // already closed the sockets. Publish that BEFORE the token
            // exchange, which is slow (a network round trip) and must not be
            // abortable by a supersede: nothing is waiting on the port any more,
            // and this flow may already have redeemed the user's consent.
            task_holds_port.store(false, std::sync::atomic::Ordering::Release);

            let code = waited
                .map_err(|_| "OAuth authorization timed out after 120 seconds".to_string())?
                .map_err(|e| format!("OAuth callback error: {}", e))?;

            // Exchange code for tokens. `exchange_code` already names the leg
            // and carries the provider's own error text, so it is NOT re-wrapped.
            let token_resp = exchange_code(&token_url, &client_id, &auth, &code, &redirect_uri)
                .await
                .map_err(|e| e.to_string())?;

            // Fetch userinfo (best-effort — failures downgrade to None inside)
            let (email, display_name) = if let Some(ref url) = userinfo_url {
                fetch_userinfo(url, &token_resp.access_token, userinfo_method).await
            } else {
                (None, None)
            };

            // Calculate token expiry
            let token_expiry = token_resp
                .expires_in
                .map(|secs| chrono::Utc::now() + chrono::Duration::seconds(secs as i64));

            // Use actually granted scopes from the token response, fall back to what we requested
            let granted_scopes = token_resp.scope.as_deref().unwrap_or(&merged_scopes);

            // Store account with granted scopes AND the set that was requested.
            // Recording the request is what lets a later *Reconnect* ask for a
            // scope this authorization did not get: the difference between these
            // two arguments IS the shortfall, and before this it was computed,
            // used to build one URL, and thrown away.
            //
            // `connect` announces OAuthAccountConnected from inside the write
            // path, so every device reloads its Accounts list without waiting
            // for a page refresh.
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
                // Carried out of the flow rather than recomputed by the caller:
                // this is the set the authorization URL was actually built
                // from, and anything derived later from the stored account
                // could disagree with it.
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
/// **The engine does not open browsers.** This used to `std::process::Command`
/// out to macOS `open`, which was wrong twice over: it ignored the user's
/// in-app-browser preference (the authorization page appeared in the system
/// browser on a machine configured for the panel, 2026-08-05) and it silently
/// did nothing at all on Linux, where the headless tarball also runs. Deciding
/// where a URL is displayed belongs to the client, which knows the platform and
/// the preference, so the caller supplies an opener. Today's only caller emits a
/// `NavigationRequested` scoped to the device whose prompt started the turn, and
/// the frontend's `openUrl` picks the panel, the OS opener, or a new tab.
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
    // A failed hand-off is fatal to the flow, not best-effort: nothing will ever
    // reach the callback, so waiting out the 120s timeout would only turn a
    // precise error into "authorization timed out".
    open_auth_url(&prepared.auth_url).await?;

    // Wait for the background task to complete
    prepared
        .result_rx
        .await
        .map_err(|_| FLOW_SUPERSEDED_MSG)?
        .map_err(|e| e.into())
}

/// Whether a provider's userinfo endpoint is fetched with GET or POST.
///
/// GET is the OIDC norm and stays the default, so every credential written
/// before this existed keeps working untouched. Dropbox is why the alternative
/// exists: `users/get_current_account` is POST-only, so it was recorded as
/// having *no* userinfo endpoint at all, and a connected Dropbox account
/// reported itself as "unknown" (the agent resorted to a raw `curl` to find out
/// whose account it was, 2026-08-05).
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
/// Two shapes in the wild: OIDC's flat `"name": "Jane Doe"`, and a nested object
/// (Dropbox returns `{"name": {"display_name": …, "given_name": …}}`). Reading
/// only the flat form left the nested case as no name at all.
fn userinfo_display_name(body: &serde_json::Value) -> Option<String> {
    let name = body.get("name")?;
    if let Some(flat) = name.as_str() {
        return Some(flat.to_string());
    }
    name.get("display_name")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Fetch user info (email, name) from a userinfo endpoint.
/// Returns (email, display_name). Best-effort: any error along the way
/// (network, non-success status, JSON parse) is logged and downgraded to
/// `(None, None)` so the OAuth flow can complete without optional metadata.
async fn fetch_userinfo(
    userinfo_url: &str,
    access_token: &str,
    method: UserinfoMethod,
) -> (Option<String>, Option<String>) {
    let client = bounded_http_client();
    let request = match method {
        UserinfoMethod::Get => client.get(userinfo_url),
        // No body and no `Content-Type`, deliberately. Dropbox rejects an empty
        // body with a JSON content type ("could not decode input as JSON") and
        // rejects `{}` as well ("expected null, got value"); omitting the header
        // entirely is the shape it accepts, and it is also the most neutral
        // thing to send any other POST userinfo endpoint.
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
