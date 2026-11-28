use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::secret_store::{OAuthAccessToken, OAuthRefreshToken, Secret};
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

pub use crate::core::oauth_callback::CallbackRoute;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone)]
pub struct OAuthAccount {
    pub id: Uuid,
    pub provider: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_expiry: Option<DateTime<Utc>>,
    pub scopes: String,
    pub desired_scopes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// INVARIANT: Manual `FromRow`: both tokens are sealed at rest, and open as they decode.
impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for OAuthAccount {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row as _;
        Ok(Self {
            id: row.try_get("id")?,
            provider: row.try_get("provider")?,
            email: row.try_get("email")?,
            display_name: row.try_get("display_name")?,
            access_token: row
                .try_get::<Secret<OAuthAccessToken>, _>("access_token")?
                .into_inner(),
            refresh_token: row
                .try_get::<Option<Secret<OAuthRefreshToken>>, _>("refresh_token")?
                .map(Secret::into_inner),
            token_expiry: row.try_get("token_expiry")?,
            scopes: row.try_get("scopes")?,
            desired_scopes: row.try_get("desired_scopes")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

// INVARIANT: Manual `Debug` so an account never leaks its tokens through `{:?}`. Refresh
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

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct OAuthAccountInfo {
    pub id: Uuid,
    pub provider: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub scopes: String,
    /// INVARIANT: What the account was ASKED for, accumulated across every authorization.
    ///
    /// *Reconnect* re-requests this, never `scopes`. `prepare_oauth_flow`
    /// merges the request with the existing grant, so asking for the granted
    /// set again could not recover a scope the provider had refused.
    ///
    /// `None` for an account connected before the column existed. The caller
    /// falls back to something never narrower than the granted set.
    pub desired_scopes: Option<String>,
    pub has_refresh_token: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const HAS_REFRESH_TOKEN_SQL: &str = "(COALESCE(refresh_token, '') <> '')";

/// INVARIANT: The refresh token a write may store: `None` for an absent or empty one.
///
/// Every token write goes through this. Each write keeps the stored token only
/// when handed NULL. An empty answer would otherwise erase a working token and
/// end renewal.
fn usable_refresh_token(token: Option<&str>) -> Option<&str> {
    token.filter(|t| !t.is_empty())
}

pub struct ConnectedAccount {
    pub id: Uuid,
    /// INVARIANT: Read from the ROW, never from the token response. A re-authorization
    /// returning no refresh token keeps the one the account already had, and
    /// renewal still works (see `upsert_row`'s `COALESCE`). Both the tool
    /// result and the Accounts card answer `offline_access` from this.
    pub has_refresh_token: bool,
}

#[derive(Debug, Default, Clone)]
pub struct OAuthClientOverrides {
    pub base_url: Option<String>,
    pub auth_url: Option<String>,
    pub token_url: Option<String>,
    pub userinfo_url: Option<String>,
    pub userinfo_method: Option<String>,
    pub authorize_params: Option<String>,
    pub redirect_uri: Option<String>,
}

pub fn client_provider_name(name: &str) -> String {
    let name = name.trim().to_lowercase();
    match name.strip_prefix("oauth:") {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => name,
    }
}

pub fn oauth_client_request(provider: &str, overrides: &OAuthClientOverrides) -> serde_json::Value {
    let base_url = overrides
        .base_url
        .clone()
        .unwrap_or_else(|| format!("https://{provider}.com"));
    let mut request = serde_json::json!({
        "service": client_provider_name(provider),
        "prompt": format!("Enter your OAuth client credentials for {provider}."),
        "base_urls": [base_url],
        "auth_type": "oauth_client",
    });
    let mut defaults = serde_json::Map::new();
    for (key, value) in [
        ("auth_url", &overrides.auth_url),
        ("token_url", &overrides.token_url),
        ("userinfo_url", &overrides.userinfo_url),
        ("userinfo_method", &overrides.userinfo_method),
        ("authorize_params", &overrides.authorize_params),
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
    pub fn from_registry(row: &crate::core::oauth_registry::OAuthProviderRow) -> Self {
        Self {
            base_url: Some(row.base_url.clone()),
            auth_url: Some(row.auth_url.clone()),
            token_url: Some(row.token_url.clone()),
            userinfo_url: row.userinfo_url.clone(),
            userinfo_method: row.userinfo_method.clone(),
            authorize_params: row.authorize_params.clone(),
            redirect_uri: row.redirect_uri.clone(),
        }
    }
}

const REQUIRED_FLOW_FIELDS: [&str; 3] = ["client_id", "auth_url", "token_url"];

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

pub fn client_secret_repeats_client_id(client_config: &serde_json::Value) -> bool {
    let trimmed = |key: &str| client_config[key].as_str().map(str::trim);
    match (trimmed("client_id"), trimmed("client_secret")) {
        (Some(id), Some(secret)) => !id.is_empty() && id == secret,
        _ => false,
    }
}

pub fn repeated_secret_message(provider: &str) -> String {
    format!(
        "The client secret saved for {provider} is identical to its client ID, so {provider} \
         would refuse it. Re-enter the client secret from the provider's console, or clear the \
         field to connect as a public client with PKCE where the provider allows it."
    )
}

const DEFAULT_SIGN_IN_SCOPES: &str = "openid email profile";

/// INVARIANT: The *sign-in scopes* for the provider behind `credential_value`.
///
/// `named_row`, the registry row the provider's own name resolves to, decides
/// first, so a hand-typed `auth_url` on another host cannot lose its scopes.
/// Otherwise the row whose authorization host matches the credential's
/// `auth_url` decides, so a *derived provider* on the same endpoints gets the
/// same answer. A row without `sign_in_scopes`, and a credential no row
/// matches, get [`DEFAULT_SIGN_IN_SCOPES`].
pub fn sign_in_scopes_for(
    named_row: Option<&crate::core::oauth_registry::OAuthProviderRow>,
    credential_value: &str,
    rows: &[crate::core::oauth_registry::OAuthProviderRow],
) -> String {
    named_row
        .or_else(|| {
            serde_json::from_str::<serde_json::Value>(credential_value)
                .ok()
                .and_then(|v| v["auth_url"].as_str().map(str::to_string))
                .and_then(|auth_url| crate::core::oauth_registry::row_for_auth_url(rows, &auth_url))
        })
        .and_then(|row| row.sign_in_scopes.clone())
        .unwrap_or_else(|| DEFAULT_SIGN_IN_SCOPES.to_string())
}

pub fn oauth_client_repair_request(
    provider: &str,
    overrides: &OAuthClientOverrides,
    credential_id: Uuid,
    client_id: Option<&str>,
    missing: &[&str],
) -> serde_json::Value {
    let mut request = oauth_client_request(provider, overrides);
    // INVARIANT: A repair carries NO scope, and dropping the key is what says so. The row
    // already exists, so its stored `base_urls` are the answer and the form
    // seeds from those. The overrides here come from the registry, or from the
    // `https://{provider}.com` guess for a derived name the registry misses.
    // Sending either would have the form save it over whatever the user
    // actually scoped the credential to.
    request.as_object_mut().map(|r| r.remove("base_urls"));
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

pub fn oauth_client_secret_repair_request(
    provider: &str,
    overrides: &OAuthClientOverrides,
    credential_id: Uuid,
    client_id: Option<&str>,
) -> serde_json::Value {
    let mut request =
        oauth_client_repair_request(provider, overrides, credential_id, client_id, &[]);
    request["missing"] = serde_json::json!([REPEATED_SECRET_FIELD]);
    request["prompt"] = serde_json::json!(repeated_secret_message(provider));
    request
}

pub const REPEATED_SECRET_FIELD: &str = "client_secret";

fn join_human(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// INVARIANT: True when `host` is `domain` itself, or a subdomain of it.
///
/// Anchored at a label boundary, which is the whole point. `notgithub.com`
/// strips to `not`, which does not end in a dot, and
/// `api.github.com.attacker.example` does not end with the domain at all. A
/// bare `contains` admits both. The caller attaches the user's bearer token to
/// whatever this says yes to, so never loosen it.
fn host_is(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

/// INVARIANT: Determine the provider name from an API URL.
///
/// Classification reads the parsed HOST, never the URL string. Substring
/// matching let `https://api.github.com.attacker.example/` read as GitHub, and
/// the caller then sent the user's real GitHub token to the attacker. Two arms
/// carried no host anchor at all, so they also matched inside a path or query,
/// and `https://evil.example/?next=google.com/` collected a Google bearer.
///
/// Fails closed. An unparseable URL, a URL with no host, and any scheme other
/// than `https` all return `None`, so a token cannot leave over cleartext.
///
/// See `docs/plans/2026-08-25-oauth-host-classification-and-proxy-prefix-containment.md`.
pub fn provider_for_url(url: &str) -> Option<&'static str> {
    let parsed = reqwest::Url::parse(url.trim()).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();

    if host_is(&host, "googleapis.com") || host_is(&host, "google.com") {
        Some("google")
    } else if host_is(&host, "graph.microsoft.com") || host_is(&host, "login.microsoftonline.com") {
        Some("microsoft")
    } else if host_is(&host, "api.github.com") {
        Some("github")
    } else if host_is(&host, "dropboxapi.com") || host_is(&host, "dropbox.com") {
        Some("dropbox")
    } else if host_is(&host, "api.spotify.com") || host_is(&host, "accounts.spotify.com") {
        Some("spotify")
    } else {
        None
    }
}

const SECRET_BEARING_ENDPOINTS: [&str; 3] = ["auth_url", "token_url", "userinfo_url"];

fn is_loopback_host(host: &str) -> bool {
    let unbracketed = host.trim_start_matches('[').trim_end_matches(']');
    match unbracketed.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => host.eq_ignore_ascii_case("localhost"),
    }
}

fn is_secure_endpoint(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        || (url.scheme() == "http" && url.host_str().is_some_and(is_loopback_host))
}

fn require_secure_endpoint(credential: &str, field: &str, url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url.trim())
        .map_err(|_| format!("The {field} of the {credential} OAuth client is not a valid URL"))?;
    if is_secure_endpoint(&parsed) {
        return Ok(());
    }
    Err(format!(
        "The {field} of the {credential} OAuth client must use https, not {}://. \
         Over anything else its secrets would travel in cleartext.",
        parsed.scheme()
    ))
}

fn check_client_endpoints_in(credential: &str, config: &serde_json::Value) -> Result<(), String> {
    for field in SECRET_BEARING_ENDPOINTS {
        let url = config.get(field).and_then(|v| v.as_str()).map(str::trim);
        if let Some(url) = url.filter(|s| !s.is_empty()) {
            require_secure_endpoint(credential, field, url)?;
        }
    }
    Ok(())
}

/// INVARIANT: Refuse an `oauth_client` secret about to be saved whose endpoints would send
/// a secret over cleartext. The flow checks again on read, so a row written
/// before this check still cannot leak.
///
/// A value that is not a JSON object passes. It names no endpoint, and
/// [`missing_flow_fields`] already sends it to the repair form.
pub fn check_client_endpoints(credential: &str, auth_value: &str) -> Result<(), String> {
    match serde_json::from_str::<serde_json::Value>(auth_value) {
        Ok(config) => check_client_endpoints_in(credential, &config),
        Err(_) => Ok(()),
    }
}

pub struct OAuthStore;

impl OAuthStore {
    /// INVARIANT: Insert or update an OAuth account row (upsert on provider+email).
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
    ) -> Result<ConnectedAccount, sqlx::Error> {
        let refresh_token = usable_refresh_token(refresh_token);
        let result = if email.is_some() {
            sqlx::query_as::<_, (Uuid, bool)>(
                &format!(r#"
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
                RETURNING id, {HAS_REFRESH_TOKEN_SQL}
                "#),
            )
            .bind(provider)
            .bind(email)
            .bind(display_name)
            .bind(Secret::<OAuthAccessToken>::new(access_token))
            .bind(refresh_token.map(Secret::<OAuthRefreshToken>::new))
            .bind(token_expiry)
            .bind(scopes)
            .bind(desired_scopes)
            .fetch_one(pool)
            .await?
        } else {
            sqlx::query_as::<_, (Uuid, bool)>(
                &format!(r#"
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
                RETURNING id, {HAS_REFRESH_TOKEN_SQL}
                "#),
            )
            .bind(provider)
            .bind(display_name)
            .bind(Secret::<OAuthAccessToken>::new(access_token))
            .bind(refresh_token.map(Secret::<OAuthRefreshToken>::new))
            .bind(token_expiry)
            .bind(scopes)
            .bind(desired_scopes)
            .fetch_one(pool)
            .await?
        };

        Ok(ConnectedAccount {
            id: result.0,
            has_refresh_token: result.1,
        })
    }

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

    /// INVARIANT: Resolve the active OAuth account for a provider: the most-recently
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

    /// INVARIANT: Refresh an account's tokens in place.
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
        .bind(Secret::<OAuthAccessToken>::new(access_token))
        .bind(token_expiry)
        .bind(usable_refresh_token(refresh_token).map(Secret::<OAuthRefreshToken>::new))
        .execute(pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn list(pool: &PgPool) -> Result<Vec<OAuthAccountInfo>, sqlx::Error> {
        sqlx::query_as::<_, OAuthAccountInfo>(&format!(
            r#"
            SELECT id, provider, email, display_name, scopes, desired_scopes,
                   {HAS_REFRESH_TOKEN_SQL} AS has_refresh_token,
                   created_at, updated_at
            FROM oauth_accounts
            ORDER BY provider ASC, created_at ASC
            "#
        ))
        .fetch_all(pool)
        .await
    }

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

    async fn delete_row(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM oauth_accounts WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// INVARIANT: Store a freshly authorized OAuth account and announce it. The only way
    /// to connect one.
    ///
    /// Announces on a re-authorization too (the upsert path), because
    /// re-connecting is how scopes are granted: the account's capabilities
    /// changed even though the row already existed.
    #[allow(clippy::too_many_arguments)]
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
    ) -> Result<ConnectedAccount, sqlx::Error> {
        let stored = Self::upsert_row(
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
                    account_id: stored.id.to_string(),
                    provider: provider.to_string(),
                    email: email.map(str::to_string),
                    actor,
                }),
                "[OAuth] OAuthAccountConnected",
            )
            .await;
        Ok(stored)
    }

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

pub fn provider_not_connected_msg(provider: &str) -> String {
    format!(
        "script_handshake requires OAuth provider '{}' but no account is connected; \
         user must connect it first via connect_oauth_account",
        provider
    )
}

pub enum AccountLookupError {
    NotConnected,
    DbError(BoxError),
    RefreshFailed(String),
}

pub async fn get_account_with_fresh_token(
    pool: &PgPool,
    provider: &str,
) -> Result<OAuthAccount, AccountLookupError> {
    let mut account = OAuthStore::get_by_provider(pool, provider)
        .await
        .map_err(|e| AccountLookupError::DbError(Box::new(e)))?
        .ok_or(AccountLookupError::NotConnected)?;
    refresh_or_reconnect(pool, &mut account)
        .await
        .map_err(AccountLookupError::RefreshFailed)?;
    Ok(account)
}

/// INVARIANT: [`refresh_oauth_if_needed`], with a failure turned into a message that
/// names the account and tells the user to reconnect it.
///
/// A caller that gets `Err` must not send `account.access_token`. The engine
/// cannot vouch for that token, and the provider's bare 401 names no cause.
pub async fn refresh_or_reconnect(pool: &PgPool, account: &mut OAuthAccount) -> Result<(), String> {
    refresh_oauth_if_needed(pool, account).await.map_err(|e| {
        crate::log_legacy!(
            "[OAuth] Token refresh failed for {} ({}): {}",
            account.provider,
            account.id,
            e
        );
        format!(
            "The {} token for {} could not be refreshed: {e}. If this keeps happening, reconnect the account in Settings → Accounts.",
            account.provider,
            account_label(account)
        )
    })
}

fn account_label(account: &OAuthAccount) -> String {
    match account.email.as_deref().or(account.display_name.as_deref()) {
        Some(name) => format!("the account {name}"),
        None => format!("the connected {} account", account.provider),
    }
}

/// INVARIANT: The OAuth access token an email account presents, refreshed first.
///
/// `Ok(None)` only for an account with no OAuth link, which uses its stored
/// password. Every other failure refuses, so IMAP and SMTP never skip XOAUTH2
/// silently.
pub async fn linked_account_token(
    pool: &PgPool,
    oauth_account_id: Option<Uuid>,
) -> Result<Option<String>, String> {
    let Some(oauth_id) = oauth_account_id else {
        return Ok(None);
    };
    let mut account = match OAuthStore::get_by_id(pool, oauth_id).await {
        Ok(Some(account)) => account,
        Ok(None) => {
            return Err(format!(
                "This email account is linked to OAuth account {oauth_id}, which no longer \
                 exists. Reconnect it in Settings → Accounts."
            ))
        }
        Err(e) => {
            crate::log_legacy!(
                "[OAuth] Linked account lookup failed for {}: {}",
                oauth_id,
                e
            );
            return Err(format!("Could not read the linked OAuth account: {e}"));
        }
    };
    refresh_or_reconnect(pool, &mut account).await?;
    Ok(Some(account.access_token))
}

pub fn account_env_vars(accounts: Vec<OAuthAccount>) -> Vec<(String, String)> {
    let mut env_vars = Vec::new();
    for account in accounts {
        let [token_name, email_name] = account_env_names(&account.provider);
        env_vars.push((token_name, account.access_token));
        if let Some(email) = account.email {
            env_vars.push((email_name, email));
        }
    }
    env_vars
}

pub fn account_env_names(provider: &str) -> [String; 2] {
    let prefix = format!("OAUTH_{}", crate::core::env_var_segment(provider));
    [format!("{prefix}_ACCESS_TOKEN"), format!("{prefix}_EMAIL")]
}

pub(crate) use lucidos_gateway_constants::{
    OAUTH_CALLBACK_PATH as CALLBACK_PATH, OAUTH_CALLBACK_PORT as CALLBACK_PORT,
};
/// INVARIANT: Host form advertised unless the credential overrides it. The loopback IP is
/// the default because some providers reject the name `localhost` outright.
const DEFAULT_CALLBACK_HOST: &str = "127.0.0.1";
const CALLBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];

fn callback_uri_for_host(host: &str) -> String {
    format!("http://{}:{}{}", host, CALLBACK_PORT, CALLBACK_PATH)
}

pub fn default_redirect_uri() -> String {
    callback_uri_for_host(DEFAULT_CALLBACK_HOST)
}

fn accepted_redirect_uris() -> Vec<String> {
    CALLBACK_HOSTS
        .iter()
        .copied()
        .map(callback_uri_for_host)
        .collect()
}

/// INVARIANT: Resolve the redirect URI for a flow from the credential JSON.
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

fn redirect_uri_for(
    provider: &str,
    route: &CallbackRoute,
    client_config: &serde_json::Value,
) -> Result<String, BoxError> {
    match route {
        CallbackRoute::Gateway(browser) if !browser.local => {
            web_redirect_uri_for(provider, browser, client_config)
        }
        _ => resolve_redirect_uri(client_config),
    }
}

fn web_redirect_uri_for(
    provider: &str,
    browser: &crate::core::oauth_callback::BrowserContext,
    client_config: &serde_json::Value,
) -> Result<String, BoxError> {
    let Some(uri) = browser
        .web_redirect_uri()
        .filter(|uri| uri.starts_with("https://"))
    else {
        return Err(format!(
            "{provider} can return only to Lucidos over https, so open it at an https address, \
             or on a computer running Lucidos."
        )
        .into());
    };
    if !web_redirect_uris(client_config).contains(&uri) {
        return Err(format!(
            "Register {uri} on a Web client, and add it under Web redirect URIs on the \
             {provider} credential."
        )
        .into());
    }
    if ClientAuth::from_secret(client_config["client_secret"].as_str())
        .client_secret()
        .is_none()
    {
        return Err(format!(
            "A web redirect URI needs a Web client with a client secret. Add the secret to the \
             {provider} credential."
        )
        .into());
    }
    Ok(uri)
}

fn web_redirect_uris(client_config: &serde_json::Value) -> Vec<String> {
    client_config["web_redirect_uris"]
        .as_array()
        .map(|uris| {
            uris.iter()
                .filter_map(|uri| uri.as_str())
                .map(|uri| uri.trim().to_string())
                .filter(|uri| !uri.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// INVARIANT: A PKCE verifier/challenge pair (RFC 7636).
///
/// The verifier is a one-time secret: it is the only thing standing between an
/// intercepted authorization code and a token, so it must never be logged.
pub(crate) struct Pkce {
    verifier: String,
    challenge: String,
}

impl std::fmt::Debug for Pkce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &"<redacted>")
            .field("challenge", &self.challenge)
            .finish()
    }
}

impl Pkce {
    pub(crate) fn generate() -> Self {
        let verifier = random_url_token();
        let challenge = Self::challenge_for(&verifier);
        Self {
            verifier,
            challenge,
        }
    }

    pub(crate) fn challenge_for(verifier: &str) -> String {
        use sha2::{Digest, Sha256};
        base64_url_nopad(&Sha256::digest(verifier.as_bytes()))
    }

    pub(crate) fn verifier(&self) -> &str {
        &self.verifier
    }

    pub(crate) fn challenge(&self) -> &str {
        &self.challenge
    }
}

fn base64_url_nopad(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn random_url_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64_url_nopad(&bytes)
}

/// INVARIANT: The `state` parameter for one authorization: an unguessable value sent on the
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

enum ClientAuth {
    Confidential(String),
    Public(Pkce),
}

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

    fn kind(&self) -> &'static str {
        match self {
            Self::Confidential(_) => "confidential",
            Self::Public(_) => "public+PKCE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthorizeParams(Vec<(String, String)>);

/// INVARIANT: What a credential with no `authorize_params` sends. Absent means
/// "unchanged", never "nothing": a credential written before the key existed
/// was authorized with exactly these two.
pub const DEFAULT_AUTHORIZE_PARAMS: &str = "access_type=offline&prompt=consent";

const AUTHORIZE_PARAMS_NONE: &str = "none";

/// INVARIANT: Parameters the flow itself owns. A credential is agent- and user-writable.
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

    fn append_to(&self, url: &mut String) {
        for (key, value) in &self.0 {
            url.push('&');
            url.push_str(&urlencoding::encode(key));
            url.push('=');
            url.push_str(&urlencoding::encode(value));
        }
    }
}

fn decode_param(raw: &str) -> String {
    urlencoding::decode(raw)
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| raw.to_string())
}

fn build_authorize_url(
    auth_url: &str,
    client_id: &str,
    redirect_uri: &str,
    scopes: &str,
    state: &str,
    auth: &ClientAuth,
    extra: &AuthorizeParams,
) -> String {
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

#[derive(Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
}

const MAX_TOKEN_LIFETIME_SECS: i64 = 100 * 365 * 24 * 60 * 60;

impl TokenResponse {
    /// INVARIANT: When the access token expires, counted from `now`. `None` when the
    /// provider gave no lifetime.
    ///
    /// `expires_in` is the provider's number, and a "never expires" sentinel
    /// overflows chrono's date range, which panics. So the lifetime is clamped
    /// to `MAX_TOKEN_LIFETIME_SECS` first.
    pub fn expiry_from(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let secs = i64::try_from(self.expires_in?)
            .unwrap_or(i64::MAX)
            .min(MAX_TOKEN_LIFETIME_SECS);
        Some(now + chrono::Duration::seconds(secs))
    }
}

// INVARIANT: Manual `Debug` so a token-exchange/refresh response never leaks its tokens
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

const MAX_OAUTH_REDIRECTS: usize = 10;

/// INVARIANT: Bounded, shared HTTP client for the OAuth token and userinfo endpoints.
///
/// A bare `reqwest::Client::new()` has NO timeout, and `refresh_oauth_if_needed`
/// runs on the email-send path, where a stalled identity provider would hang
/// the whole send. Built once so repeated calls reuse the connection pool and
/// the rustls context.
///
/// A 307 or 308 replays the POST body, so the client never follows a redirect
/// to an endpoint [`is_secure_endpoint`] refuses.
fn bounded_http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if !is_secure_endpoint(attempt.url()) {
                    let scheme = attempt.url().scheme().to_string();
                    attempt.error(format!(
                        "An OAuth endpoint redirected to {scheme}://, where its secrets \
                         would travel in cleartext, so Lucidos did not follow it."
                    ))
                } else if attempt.previous().len() >= MAX_OAUTH_REDIRECTS {
                    attempt.error("An OAuth endpoint redirected too many times")
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .expect("OAuth HTTP client builder")
    });
    &CLIENT
}

const MAX_ERROR_BODY_CHARS: usize = 2000;

/// INVARIANT: Render a non-success token-endpoint response into one human-readable line.
///
/// OAuth 2.0 §5.2 error bodies are JSON with `error` / `error_description`;
/// Microsoft adds a numeric `error_codes` array. Pulling those out is what
/// makes the flow debuggable from the UI. A body that is not an OAuth error
/// object falls back to the raw text rather than being dropped.
///
/// `operation` names the leg ("Token exchange" / "Token refresh"), so the
/// caller must NOT re-wrap the result: a second prefix reads as "Token exchange
/// failed: Token exchange failed (400)".
///
/// `invalid_client_hint` follows the provider's `error` when that is
/// `invalid_client`: it says how the request authenticated, which a bare
/// "Invalid client_id or client_secret" does not. It comes before the
/// provider's description, so a clamped error toast still shows the fix.
fn describe_token_error(
    operation: &str,
    status: reqwest::StatusCode,
    body: &str,
    invalid_client_hint: Option<&str>,
) -> String {
    let body = body.trim();
    let mut parts: Vec<String> = Vec::new();
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(error) = json["error"].as_str() {
            parts.push(error.to_string());
        }
        if let Some(hint) = invalid_client_hint.filter(|_| json["error"] == "invalid_client") {
            parts.push(hint.to_string());
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

fn redirect_refusal_or(error: reqwest::Error) -> BoxError {
    match std::error::Error::source(&error) {
        Some(reason) if error.is_redirect() => reason.to_string().into(),
        _ => error.into(),
    }
}

/// INVARIANT: POST a token-endpoint form and parse the response, surfacing the provider's
/// own error text on failure. Shared by the code exchange and the refresh so
/// both legs report failures identically.
///
/// Refuses a cleartext `token_url` before anything is sent, whatever path
/// stored it: the form carries the client secret or the refresh token.
async fn post_token_request(
    operation: &str,
    credential: &str,
    token_url: &str,
    form: &[(&str, &str)],
    invalid_client_hint: Option<&str>,
) -> Result<TokenResponse, BoxError> {
    require_secure_endpoint(credential, "token_url", token_url)?;
    let client = bounded_http_client();
    let resp = client
        .post(token_url)
        .header("Accept", "application/json")
        .form(form)
        .send()
        .await
        .map_err(redirect_refusal_or)?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(describe_token_error(operation, status, &body, invalid_client_hint).into());
    }

    let token: TokenResponse = resp.json().await?;
    Ok(token)
}

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

fn invalid_client_hint(client_id: &str, auth: &ClientAuth, redirect_uri: &str) -> String {
    let not_visible_ascii = |value: &str| value.chars().filter(|c| !c.is_ascii_graphic()).count();
    let id_chars = not_visible_ascii(client_id);
    let id_shape = format!(
        "{id_chars} {} outside visible ASCII in the client ID",
        if id_chars == 1 {
            "character"
        } else {
            "characters"
        },
    );
    match auth.client_secret() {
        Some(secret) => format!(
            "Re-enter the client secret from the provider's console, or clear it to use PKCE \
             where the provider allows it. Lucidos sent a client secret and redirect URI \
             {redirect_uri}. It found {id_shape} and {} in the client secret.",
            not_visible_ascii(secret)
        ),
        None => format!(
            "No client secret was sent (PKCE). If the provider requires one, add it from the \
             provider's console. Lucidos sent redirect URI {redirect_uri}. It found {id_shape}."
        ),
    }
}

async fn exchange_code(
    credential: &str,
    token_url: &str,
    client_id: &str,
    auth: &ClientAuth,
    code: &str,
    redirect_uri: &str,
) -> Result<TokenResponse, BoxError> {
    let form = exchange_form(code, client_id, redirect_uri, auth);
    let hint = invalid_client_hint(client_id, auth, redirect_uri);
    post_token_request("Token exchange", credential, token_url, &form, Some(&hint)).await
}

async fn refresh_access_token(
    credential: &str,
    token_url: &str,
    client_id: &str,
    client_secret: Option<&str>,
    refresh_token: &str,
) -> Result<TokenResponse, BoxError> {
    let form = refresh_form(refresh_token, client_id, client_secret);
    post_token_request("Token refresh", credential, token_url, &form, None).await
}

pub fn token_needs_refresh(account: &OAuthAccount) -> bool {
    match account.token_expiry {
        Some(exp) => exp < Utc::now() + chrono::Duration::seconds(60),
        None => account.refresh_token.is_some(),
    }
}

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

    let config: serde_json::Value = serde_json::from_str(&client_cred.secret(pool).await?)?;
    let cid = config["client_id"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Missing client_id in {} credentials", cred_service))?;
    // INVARIANT: Blank or absent means a public client, whose refresh must omit
    // `client_secret` exactly as its code redemption did.
    let csec = config["client_secret"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let turl = config["token_url"]
        .as_str()
        .ok_or_else(|| format!("Missing token_url in {} credentials", cred_service))?
        .to_string();

    crate::log_legacy!(
        "[OAuth] Refreshing {} token for {}",
        account.provider,
        account.email.as_deref().unwrap_or("unknown")
    );
    let new_tokens = refresh_access_token(&cred_service, &turl, cid, csec, &refresh_token).await?;
    let new_expiry = new_tokens.expiry_from(Utc::now());
    let new_refresh_token = usable_refresh_token(new_tokens.refresh_token.as_deref());
    OAuthStore::update_tokens(
        pool,
        account.id,
        &new_tokens.access_token,
        new_expiry,
        new_refresh_token,
    )
    .await?;
    if let Some(rt) = new_refresh_token {
        account.refresh_token = Some(rt.to_string());
    }
    account.access_token = new_tokens.access_token;
    account.token_expiry = new_expiry;
    crate::log_legacy!(
        "[OAuth] Successfully refreshed {} token, expires in {}s",
        account.provider,
        new_tokens.expires_in.unwrap_or(0)
    );

    Ok(())
}

pub async fn force_refresh_oauth(
    pool: &PgPool,
    account: &mut OAuthAccount,
) -> Result<(), BoxError> {
    account.token_expiry = Some(Utc::now() - chrono::Duration::seconds(1));
    refresh_oauth_if_needed(pool, account).await
}

pub fn merge_scopes(existing: &str, requested: &str) -> String {
    let mut all: Vec<&str> = existing.split_whitespace().collect();
    for scope in requested.split_whitespace() {
        if !all.contains(&scope) {
            all.push(scope);
        }
    }
    all.join(" ")
}

/// INVARIANT: Scopes a provider may leave out of the echoed `scope` after granting them,
/// so their absence from it proves nothing.
///
/// These are the OIDC and meta scopes, which name no resource. Microsoft omits
/// all four when the token is issued for a RESOURCE such as
/// `outlook.office.com`: that response lists the resource's own scopes and
/// nothing else. GitHub knows none of them, and a bare Connect asks for three.
const META_SCOPES: [&str; 4] = ["offline_access", "openid", "profile", "email"];

/// INVARIANT: What a grant produced that its echoed scope list cannot say.
///
/// `offline_access` is the one that matters. It asks for a refresh token, and
/// the token itself IS the answer. Diff the echo instead and a resource-scoped
/// Microsoft grant reads as refusing it. That sends the user to the Entra
/// portal to enable something already enabled.
#[derive(Debug, Clone, Copy)]
pub struct GrantEvidence {
    pub refresh_token: bool,
}

/// INVARIANT: Which scopes an authorization asked for and did not get, in the order they
/// were requested.
///
/// **A resource scope is an exact token set difference**, never containment.
/// Same rule as `missingScopes` in `components/settings/oauthConnectForm.ts`,
/// so the agent and the Accounts panel cannot disagree about a shortfall.
///
/// **A meta scope is answered by `evidence`, never by the echo.** A provider
/// may grant one and leave it out of the echoed list, so a difference there
/// means nothing. Only `offline_access` has evidence to read, so the other
/// three stay quiet rather than sending the user to a provider console.
///
/// **Not the same question as [`crate::core::backup::missing_scopes`]**, whose
/// `required_scopes` are substring MATCHERS and use containment on purpose.
///
/// An empty requested set yields no shortfall: nothing was asked for, so
/// nothing can be short.
pub fn missing_requested_scopes(
    requested: &str,
    granted: &str,
    evidence: GrantEvidence,
) -> Vec<String> {
    let held: std::collections::HashSet<&str> = granted.split_whitespace().collect();
    requested
        .split_whitespace()
        .filter(|scope| match *scope {
            "offline_access" => !evidence.refresh_token,
            meta if META_SCOPES.contains(&meta) => false,
            resource => !held.contains(resource),
        })
        .map(str::to_string)
        .collect()
}

/// INVARIANT: Temporary loopback listener for the OAuth callback.
///
/// Binds **both** loopback families on the same port. IPv4 is required; IPv6 is
/// best-effort so a host with IPv6 disabled still completes the flow. Both are
/// needed because `localhost` resolves to `::1` first on an IPv6-enabled host,
/// so an IPv4-only socket silently never receives a `localhost` callback.
struct CallbackListener {
    listeners: Vec<tokio::net::TcpListener>,
}

impl CallbackListener {
    async fn bind(port: u16) -> std::io::Result<Self> {
        let v4 = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
        let port = v4.local_addr()?.port();
        let mut listeners = vec![v4];
        match tokio::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, port)).await {
            Ok(v6) => listeners.push(v6),
            Err(e) => crate::log_legacy!(
                "[OAuth] IPv6 loopback [::1]:{} unavailable, listening on IPv4 only: {}",
                port,
                e
            ),
        }
        Ok(Self { listeners })
    }

    #[cfg(test)]
    fn local_addrs(&self) -> Vec<std::net::SocketAddr> {
        self.listeners
            .iter()
            .filter_map(|listener| listener.local_addr().ok())
            .collect()
    }

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

static ACTIVE_CALLBACK_FLOW: std::sync::LazyLock<tokio::sync::Mutex<Option<ActiveCallbackFlow>>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(None));

/// INVARIANT: The flow registered in [`ACTIVE_CALLBACK_FLOW`], and whether it still holds
/// the port.
///
/// The two are separate because a flow's task OUTLIVES its ownership of the
/// socket: the token exchange, the userinfo call and the account write all run
/// with the port already released.
struct ActiveCallbackFlow {
    task: tokio::task::JoinHandle<()>,
    /// INVARIANT: The *form request* the flow's authorization page answers. A flow
    /// superseded while still listening is resolved by whoever supersedes it,
    /// because the aborted task cannot resolve itself.
    request_id: uuid::Uuid,
    /// INVARIANT: Set once at registration, cleared by the flow itself the instant it stops
    /// holding the listener. Ordered so that `false` implies the sockets are
    /// already closed: the store happens after the future that owns them has
    /// resolved and dropped them.
    holds_port: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

pub const OAUTH_TIMEOUT_MSG: &str =
    "OAuth authorization timed out after 120 seconds. The authorization page may not have \
     opened, or it was left unfinished. Start the connection again to retry.";

pub const FLOW_SUPERSEDED_MSG: &str =
    "This authorization was canceled, most likely because a newer one was started. \
     Start it again if you still need it.";

/// INVARIANT: Cancel the flow that owns the callback port, and wait until its socket is
/// actually closed. Returns the form request of the live flow it superseded.
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
async fn release_callback_port(slot: &mut Option<ActiveCallbackFlow>) -> Option<uuid::Uuid> {
    let flow = slot.take()?;
    if !flow.holds_port.load(std::sync::atomic::Ordering::Acquire) {
        return None;
    }
    flow.task.abort();
    let _ = flow.task.await;
    Some(flow.request_id)
}

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

const MAX_REQUEST_LINE_BYTES: usize = 64 * 1024;

/// INVARIANT: How long one connection gets to send its request line before we abandon it
/// and go back to waiting.
///
/// A browser sends immediately after connecting, but a speculative preconnect
/// can open a socket and hold it idle. Unbounded, that idle socket blocks the
/// real redirect behind it in the accept queue until the flow times out.
const CALLBACK_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// INVARIANT: Read the HTTP request line — everything before the first newline.
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
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

pub(crate) fn query_param(query: &str, key: &str, plus_is_space: bool) -> Option<String> {
    let raw = query
        .split('&')
        .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))?;
    let raw = if plus_is_space {
        raw.replace('+', " ")
    } else {
        raw.to_string()
    };
    Some(
        urlencoding::decode(&raw)
            .map(|decoded| decoded.into_owned())
            .unwrap_or(raw),
    )
}

fn callback_state_matches(query: &str, expected: &str) -> bool {
    query_param(query, "state", false).is_some_and(|got| got == expected)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationGrant {
    pub code: String,
    pub client_id: Option<String>,
}

pub(crate) fn parse_callback_query(query: &str) -> Result<AuthorizationGrant, BoxError> {
    if let Some(code) = query_param(query, "code", false).filter(|c| !c.is_empty()) {
        let client_id = query_param(query, "client_id", false).filter(|c| !c.is_empty());
        return Ok(AuthorizationGrant { code, client_id });
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
        crate::log_legacy!(
            "[OAuth] Failed to write callback response to browser: {}",
            e
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackPageState {
    Connected,
    Pending,
    Failed,
    Refused,
}

impl CallbackPageState {
    pub fn is_error(self) -> bool {
        matches!(self, Self::Failed | Self::Refused)
    }
}

/// INVARIANT: How long the browser's callback request waits for the flow's outcome.
///
/// Shorter than the gateway's wait for the engine, so the engine always answers
/// first and a slow exchange degrades to [`CallbackPageState::Pending`].
pub(crate) const PAGE_SETTLE_WAIT: std::time::Duration =
    std::time::Duration::from_secs(lucidos_engine_socket::OAUTH_CALLBACK_HOP_TIMEOUT.as_secs() - 2);

#[derive(Debug)]
pub struct PageReply(tokio::sync::oneshot::Sender<CallbackPageState>);

pub(crate) struct PageSettled(tokio::sync::oneshot::Receiver<CallbackPageState>);

impl PageReply {
    pub(crate) fn pair() -> (Self, PageSettled) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (Self(tx), PageSettled(rx))
    }

    pub fn settle(self, connected: bool) {
        let state = if connected {
            CallbackPageState::Connected
        } else {
            CallbackPageState::Failed
        };
        let _ = self.0.send(state);
    }
}

impl PageSettled {
    pub(crate) async fn state(self) -> CallbackPageState {
        match tokio::time::timeout(PAGE_SETTLE_WAIT, self.0).await {
            Ok(Ok(state)) => state,
            _ => CallbackPageState::Pending,
        }
    }
}

#[derive(Debug)]
pub struct ReceivedGrant {
    pub grant: AuthorizationGrant,
    pub reply: PageReply,
}

const BRAND_MARK: &str = include_str!("../../../lucidos-app/public/favicon.svg");

/// INVARIANT: The page the provider's redirect lands on.
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
/// The state says how far the flow got when the page was written. `Connected`
/// is only ever written after the account was stored.
pub(crate) fn callback_page(provider: &str, state: CallbackPageState) -> String {
    let (heading, detail, provider_label) = match state {
        CallbackPageState::Connected => (
            "Connected",
            "Lucidos is connected to this account. You can close this tab.",
            "Connected to",
        ),
        CallbackPageState::Pending => (
            "Authorization received",
            "Lucidos is still finishing the connection. Return to Lucidos for the result.",
            "Authorized with",
        ),
        CallbackPageState::Failed => (
            "Connection failed",
            "Nothing was connected. Return to Lucidos for the details.",
            "Tried to connect",
        ),
        CallbackPageState::Refused => (
            "Authorization failed",
            "Nothing was connected. Return to Lucidos for the details.",
            "Tried to connect",
        ),
    };
    let provider = provider
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let title = if provider.is_empty() {
        "Lucidos".to_string()
    } else {
        format!("Lucidos {provider}")
    };
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

/// INVARIANT: Wait for the provider's redirect and extract the authorization code.
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
) -> Result<ReceivedGrant, BoxError> {
    loop {
        let mut stream = listener.accept().await?;
        // INVARIANT: A misbehaving connection is discarded, never fatal: a reset or idle
        // preconnect must not fail an authorization the user completed.
        let request_line =
            match tokio::time::timeout(CALLBACK_READ_TIMEOUT, read_request_line(&mut stream)).await
            {
                Ok(Ok(line)) => line,
                Ok(Err(e)) => {
                    crate::log_legacy!("[OAuth] Discarding unreadable callback connection: {}", e);
                    continue;
                }
                Err(_) => {
                    crate::log_legacy!(
                        "[OAuth] Callback connection sent nothing within {}s, discarding",
                        CALLBACK_READ_TIMEOUT.as_secs()
                    );
                    continue;
                }
            };
        let target = request_line.split_whitespace().nth(1).unwrap_or("");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));

        // INVARIANT: Browsers open extra connections to the callback origin (favicon
        // probes, speculative preconnects). Treating one as the callback would
        // consume the accept and strand the flow. Answer them and keep waiting:
        // the caller's timeout bounds this loop.
        if path != CALLBACK_PATH || query.is_empty() {
            respond_to_browser(&mut stream, "404 Not Found", "<html><body></body></html>").await;
            continue;
        }

        if !callback_state_matches(query, expected_state) {
            crate::log_legacy!(
                "[OAuth] Ignoring a {} callback that did not carry this flow's state",
                provider
            );
            let body = callback_page(provider, CallbackPageState::Refused);
            respond_to_browser(&mut stream, "400 Bad Request", &body).await;
            continue;
        }

        let grant = match parse_callback_query(query) {
            Ok(grant) => grant,
            Err(refusal) => {
                let body = callback_page(provider, CallbackPageState::Refused);
                respond_to_browser(&mut stream, "400 Bad Request", &body).await;
                return Err(refusal);
            }
        };
        let (reply, settled) = PageReply::pair();
        let provider = provider.to_string();
        tokio::spawn(async move {
            let state = settled.state().await;
            let status = if state.is_error() {
                "400 Bad Request"
            } else {
                "200 OK"
            };
            respond_to_browser(&mut stream, status, &callback_page(&provider, state)).await;
        });
        return Ok(ReceivedGrant { grant, reply });
    }
}

enum CallbackWait {
    Listener(CallbackListener),
    Gateway(crate::core::oauth_callback::PendingCallback),
}

impl CallbackWait {
    async fn received(self, provider: &str, state: &str) -> Result<ReceivedGrant, BoxError> {
        match self {
            Self::Listener(listener) => wait_for_oauth_callback(listener, provider, state).await,
            Self::Gateway(pending) => pending.received().await,
        }
    }
}

pub struct OAuthFlowOutcome {
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub granted_scopes: String,
    pub requested_scopes: String,
    pub has_refresh_token: bool,
}

pub type OAuthFlowResult = Result<OAuthFlowOutcome, String>;

fn authorization_outcome<T, E, F>(
    waited: &Result<Result<T, E>, F>,
) -> crate::engine::thread_events::FormRequestOutcome {
    use crate::engine::thread_events::FormRequestOutcome;
    match waited {
        Err(_) => FormRequestOutcome::Expired,
        Ok(Err(_)) => FormRequestOutcome::Canceled,
        Ok(Ok(_)) => FormRequestOutcome::Completed,
    }
}

pub struct PreparedOAuthFlow {
    pub auth_url: String,
    pub request_id: uuid::Uuid,
    pub result_rx: tokio::sync::oneshot::Receiver<OAuthFlowResult>,
}

pub async fn prepare_oauth_flow(
    pool: &PgPool,
    event_bus: &EventBus,
    provider: &str,
    scopes: &str,
    initiator: Option<MessageOrigin>,
    route: CallbackRoute,
) -> Result<PreparedOAuthFlow, BoxError> {
    use crate::core::CredentialStore;

    let provider = client_provider_name(provider);

    // INVARIANT: What this flow will ASK for: everything the account already holds, plus
    // everything it has ever been asked for, plus what this caller wants. Never
    // narrower than any of the three.
    //
    // The `desired` half is what lets *Reconnect* recover a refused scope.
    // Merging only against the GRANTED set computes `granted UNION granted`, so
    // an account a provider had narrowed would stay narrow forever.
    let existing_account = OAuthStore::get_by_provider(pool, &provider).await?;
    let merged_scopes = match existing_account {
        Some(ref acct) => {
            let held = merge_scopes(&acct.scopes, acct.desired_scopes.as_deref().unwrap_or(""));
            merge_scopes(&held, scopes)
        }
        None => scopes.to_string(),
    };

    let client_cred = CredentialStore::get_oauth_client(pool, &provider)
        .await?
        .ok_or_else(|| format!("No OAuth client credentials found for {}", provider))?;

    let client_config: serde_json::Value =
        serde_json::from_str(&client_cred.secret(pool).await?)
            .map_err(|e| format!("Invalid OAuth client credentials JSON: {}", e))?;
    let client_id = client_config["client_id"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("Missing client_id in OAuth credentials")?
        .to_string();
    if client_secret_repeats_client_id(&client_config) {
        return Err(repeated_secret_message(&provider).into());
    }
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
    let authorize_params = AuthorizeParams::parse(client_config["authorize_params"].as_str())?;
    check_client_endpoints_in(&provider, &client_config)?;

    let redirect_uri = redirect_uri_for(&provider, &route, &client_config)?;

    let state = route.state_for(&generate_oauth_state());

    // INVARIANT: Wait BEFORE returning the URL, so the callback cannot arrive before
    // anything is waiting for it. Only the no-gateway path holds the slot.
    let (wait, mut active_flow) = match route {
        CallbackRoute::OwnListener => {
            let (listener, slot) = bind_own_listener(pool, event_bus).await?;
            (CallbackWait::Listener(listener), Some(slot))
        }
        CallbackRoute::Gateway(_) => (
            CallbackWait::Gateway(crate::core::oauth_callback::PendingCallback::open(
                &state, &provider,
            )),
            None,
        ),
    };

    let auth_request_url = build_authorize_url(
        &auth_url,
        &client_id,
        &redirect_uri,
        &merged_scopes,
        &state,
        &auth,
        &authorize_params,
    );

    crate::log_legacy!(
        "[OAuth] Prepared {} authorization URL ({} client, {}), redirect_uri {}",
        provider,
        auth.kind(),
        match wait {
            CallbackWait::Listener(_) => "own listener",
            CallbackWait::Gateway(_) => "gateway delivers",
        },
        redirect_uri
    );

    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let request_id = uuid::Uuid::new_v4();
    let pool = pool.clone();
    let event_bus = event_bus.clone();
    let initiator = initiator.clone();
    let holds_port = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let task_holds_port = holds_port.clone();

    let task = tokio::spawn(async move {
        let mut page_reply: Option<PageReply> = None;
        let result = async {
            let waited = tokio::time::timeout(
                std::time::Duration::from_secs(120),
                wait.received(&provider, &state),
            )
            .await;

            // INVARIANT: The future owns the listener or the delivery slot, so both the
            // completed and the timed-out path have already closed it. Publish
            // that BEFORE the token exchange, which is a network round trip and
            // must not be abortable by a supersede.
            task_holds_port.store(false, std::sync::atomic::Ordering::Release);

            crate::engine::form_requests::resolve_or_log(
                &pool,
                &event_bus,
                request_id,
                authorization_outcome(&waited),
                None,
            )
            .await;

            let received = waited
                .map_err(|_| OAUTH_TIMEOUT_MSG.to_string())?
                .map_err(|e| format!("OAuth callback error: {}", e))?;
            let code = received.grant.code;
            page_reply = Some(received.reply);

            let token_resp = exchange_code(
                &provider,
                &token_url,
                &client_id,
                &auth,
                &code,
                &redirect_uri,
            )
            .await
            .map_err(|e| e.to_string())?;

            let (email, display_name) = if let Some(ref url) = userinfo_url {
                fetch_userinfo(url, &token_resp.access_token, userinfo_method).await
            } else {
                (None, None)
            };

            let token_expiry = token_resp.expiry_from(chrono::Utc::now());

            let granted_scopes = token_resp.scope.as_deref().unwrap_or(&merged_scopes);

            let stored = OAuthStore::connect(
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

            crate::log_legacy!(
                "[OAuth] Connected {} account: {} (scopes: {})",
                provider,
                email.as_deref().unwrap_or("unknown"),
                granted_scopes
            );

            Ok(OAuthFlowOutcome {
                email,
                display_name,
                granted_scopes: granted_scopes.to_string(),
                requested_scopes: merged_scopes.clone(),
                // INVARIANT: From the row the upsert left, not from `token_resp`: a
                // re-authorization returning no refresh token keeps the one
                // the account already had, and the Accounts card reads that
                // same row.
                has_refresh_token: stored.has_refresh_token,
            })
        }
        .await;

        if let Some(reply) = page_reply {
            reply.settle(result.is_ok());
        }
        let _ = result_tx.send(result);
    });

    // INVARIANT: Register the new owner before releasing the lock, so the next flow has
    // something to supersede and the port is never orphaned again.
    if let Some(slot) = active_flow.as_mut() {
        **slot = Some(ActiveCallbackFlow {
            task,
            request_id,
            holds_port,
        });
    }
    drop(active_flow);

    Ok(PreparedOAuthFlow {
        auth_url: auth_request_url,
        request_id,
        result_rx,
    })
}

/// INVARIANT: Bind this engine's own callback listener, superseding the flow that holds it.
///
/// Returns the slot still locked: the caller holds it past the spawn, so two
/// callers cannot both find it empty and race for the socket. See
/// `docs/adr/0068-oauth-callback-port-has-one-owner.md`.
async fn bind_own_listener(
    pool: &PgPool,
    event_bus: &EventBus,
) -> Result<
    (
        CallbackListener,
        tokio::sync::MutexGuard<'static, Option<ActiveCallbackFlow>>,
    ),
    BoxError,
> {
    let mut active_flow = ACTIVE_CALLBACK_FLOW.lock().await;
    if let Some(superseded) = release_callback_port(&mut active_flow).await {
        crate::log_legacy!(
            "[OAuth] Superseded an authorization still waiting on port {}",
            CALLBACK_PORT
        );
        crate::engine::form_requests::resolve_or_log(
            pool,
            event_bus,
            superseded,
            crate::engine::thread_events::FormRequestOutcome::Superseded,
            None,
        )
        .await;
    }
    let listener = CallbackListener::bind(CALLBACK_PORT)
        .await
        .map_err(|e| callback_bind_error(CALLBACK_PORT, e))?;
    Ok((listener, active_flow))
}

pub async fn run_oauth_flow<F>(
    pool: &PgPool,
    event_bus: &EventBus,
    provider: &str,
    scopes: &str,
    initiator: Option<MessageOrigin>,
    route: CallbackRoute,
    open_auth_url: F,
) -> Result<OAuthFlowOutcome, BoxError>
where
    F: AsyncFnOnce(&str, uuid::Uuid) -> Result<(), BoxError>,
{
    let prepared = prepare_oauth_flow(pool, event_bus, provider, scopes, initiator, route).await?;

    crate::log_legacy!(
        "[OAuth] Handing {} authorization URL to the client to open",
        provider
    );
    open_auth_url(&prepared.auth_url, prepared.request_id).await?;

    let result = prepared.result_rx.await;
    use crate::engine::thread_events::FormRequestOutcome;
    let answered = match &result {
        Ok(Ok(_)) => FormRequestOutcome::Completed,
        Ok(Err(_)) => FormRequestOutcome::Expired,
        Err(_) => FormRequestOutcome::Superseded,
    };
    crate::engine::form_requests::resolve_or_log(
        pool,
        event_bus,
        prepared.request_id,
        answered,
        None,
    )
    .await;
    result
        .map_err(|_| FLOW_SUPERSEDED_MSG)?
        .map_err(|e| e.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserinfoMethod {
    Get,
    Post,
}

impl UserinfoMethod {
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim).unwrap_or_default().to_ascii_uppercase() {
            m if m == "POST" => Self::Post,
            _ => Self::Get,
        }
    }
}

fn userinfo_display_name(body: &serde_json::Value) -> Option<String> {
    let name = body.get("name")?;
    if let Some(flat) = name.as_str() {
        return Some(flat.to_string());
    }
    name.get("display_name")
        .and_then(|v| v.as_str())
        .map(String::from)
}

async fn fetch_userinfo(
    userinfo_url: &str,
    access_token: &str,
    method: UserinfoMethod,
) -> (Option<String>, Option<String>) {
    let client = bounded_http_client();
    let request = match method {
        UserinfoMethod::Get => client.get(userinfo_url),
        // INVARIANT: No body and no `Content-Type`, deliberately. A JSON content type with
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
            crate::log_legacy!(@OAuth, "Userinfo fetch failed: {}", e);
            return (None, None);
        }
    };

    if !resp.status().is_success() {
        crate::log_legacy!(@OAuth, "Userinfo returned status {}", resp.status());
        return (None, None);
    }

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            crate::log_legacy!(@OAuth, "Userinfo parse failed: {}", e);
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
