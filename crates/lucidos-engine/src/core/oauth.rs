use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

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

/// Store for managing OAuth accounts in the database
pub struct OAuthStore;

impl OAuthStore {
    /// Insert or update an OAuth account (upsert on provider+email).
    /// Uses a separate conflict clause when email is NULL because PostgreSQL
    /// treats NULL != NULL, so `UNIQUE(provider, email)` never fires for NULLs.
    /// A partial unique index `oauth_accounts_provider_no_email` covers that case.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert(
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

    /// Delete an OAuth account by UUID
    pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM oauth_accounts WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;

        Ok(result.rows_affected() > 0)
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

/// Exchange an authorization code for tokens.
async fn exchange_code(
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<TokenResponse, BoxError> {
    let client = reqwest::Client::new();
    let resp = client
        .post(token_url)
        .header("Accept", "application/json")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Token exchange failed ({}): {}", status, body).into());
    }

    let token: TokenResponse = resp.json().await?;
    Ok(token)
}

/// Refresh an access token using a refresh token.
async fn refresh_access_token(
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<TokenResponse, BoxError> {
    let client = reqwest::Client::new();
    let resp = client
        .post(token_url)
        .header("Accept", "application/json")
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
            ("client_secret", client_secret),
        ])
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Token refresh failed ({}): {}", status, body).into());
    }

    let token: TokenResponse = resp.json().await?;
    Ok(token)
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
        .ok_or_else(|| format!("Missing client_id in {} credentials", cred_service))?;
    let csec = config["client_secret"]
        .as_str()
        .ok_or_else(|| format!("Missing client_secret in {} credentials", cred_service))?;
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

/// Wait for an OAuth callback on the given listener, extract the authorization code.
async fn wait_for_oauth_callback(
    listener: tokio::net::TcpListener,
) -> Result<String, BoxError> {
    let (stream, _) = listener.accept().await?;
    let mut buf = vec![0u8; 4096];
    stream.readable().await?;
    let n = stream.try_read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // Parse the GET request line to extract the code parameter
    let first_line = request.lines().next().unwrap_or("");
    let path = first_line.split_whitespace().nth(1).unwrap_or("");
    let code = path
        .split('?')
        .nth(1)
        .and_then(|q| {
            q.split('&')
                .find(|p| p.starts_with("code="))
                .map(|p| p.trim_start_matches("code=").to_string())
        })
        .ok_or("No authorization code in callback")?;

    // Best-effort browser response — token already exchanged downstream, so a
    // disconnected browser must not fail the caller. Log so regressions surface.
    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html><body><h2>Authorization successful!</h2><p>You can close this tab and return to Lucidos.</p></body></html>";
    stream.writable().await?;
    if let Err(e) = stream.try_write(response.as_bytes()) {
        crate::log!("[OAuth] Failed to write success response to browser: {}", e);
    }

    // URL-decode the code
    let code = urlencoding::decode(&code)?.into_owned();

    Ok(code)
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
        .ok_or("Missing client_id in OAuth credentials")?
        .to_string();
    let client_secret = client_config["client_secret"]
        .as_str()
        .ok_or("Missing client_secret in OAuth credentials")?
        .to_string();

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

    // Start temporary localhost listener for callback BEFORE returning the URL
    let listener = tokio::net::TcpListener::bind("127.0.0.1:14981").await?;
    let redirect_uri = "http://127.0.0.1:14981/oauth/callback".to_string();

    // Build authorization URL with merged scopes
    let auth_request_url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent",
        auth_url,
        urlencoding::encode(&client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&merged_scopes),
    );

    crate::log!(
        "[OAuth] Prepared {} authorization URL, listener on port 14981",
        provider
    );

    // Spawn background task to wait for callback and complete the exchange
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let pool = pool.clone();
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

            // Exchange code for tokens
            let token_resp =
                exchange_code(&token_url, &client_id, &client_secret, &code, &redirect_uri)
                    .await
                    .map_err(|e| format!("Token exchange failed: {}", e))?;

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

            // Store account with granted scopes
            OAuthStore::insert(
                &pool,
                &provider,
                email.as_deref(),
                display_name.as_deref(),
                &token_resp.access_token,
                token_resp.refresh_token.as_deref(),
                token_expiry,
                granted_scopes,
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
    provider: &str,
    scopes: &str,
) -> Result<(Option<String>, Option<String>, String), BoxError> {
    let prepared = prepare_oauth_flow(pool, provider, scopes).await?;

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
    let client = reqwest::Client::new();
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
