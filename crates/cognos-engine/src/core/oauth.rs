use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Full OAuth account row including tokens (internal use only)
#[derive(Debug, Clone)]
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

/// OAuth account info without tokens (safe for API responses)
#[derive(Debug, Clone, Serialize, Deserialize)]
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
// Well-known provider configuration
// ---------------------------------------------------------------------------

pub struct OAuthProviderConfig {
    pub auth_url: &'static str,
    pub token_url: &'static str,
    pub userinfo_url: Option<&'static str>,
}

/// Return well-known OAuth endpoints for a provider name.
pub fn well_known_provider(provider: &str) -> Option<OAuthProviderConfig> {
    match provider {
        "google" => Some(OAuthProviderConfig {
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            userinfo_url: Some("https://www.googleapis.com/oauth2/v2/userinfo"),
        }),
        "microsoft" => Some(OAuthProviderConfig {
            auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            userinfo_url: Some("https://graph.microsoft.com/v1.0/me"),
        }),
        "github" => Some(OAuthProviderConfig {
            auth_url: "https://github.com/login/oauth/authorize",
            token_url: "https://github.com/login/oauth/access_token",
            userinfo_url: Some("https://api.github.com/user"),
        }),
        "dropbox" => Some(OAuthProviderConfig {
            auth_url: "https://www.dropbox.com/oauth2/authorize",
            token_url: "https://api.dropboxapi.com/oauth2/token",
            userinfo_url: None,
        }),
        _ => None,
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
        let result = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                Option<String>,
                Option<String>,
                String,
                Option<String>,
                Option<DateTime<Utc>>,
                String,
                DateTime<Utc>,
                DateTime<Utc>,
            ),
        >(
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
        .await?;

        Ok(result.map(
            |(
                id,
                provider,
                email,
                display_name,
                access_token,
                refresh_token,
                token_expiry,
                scopes,
                created_at,
                updated_at,
            )| {
                OAuthAccount {
                    id,
                    provider,
                    email,
                    display_name,
                    access_token,
                    refresh_token,
                    token_expiry,
                    scopes,
                    created_at,
                    updated_at,
                }
            },
        ))
    }

    /// Get the first OAuth account for a provider (ordered by created_at ASC)
    pub async fn get_by_provider(
        pool: &PgPool,
        provider: &str,
    ) -> Result<Option<OAuthAccount>, sqlx::Error> {
        let result = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                Option<String>,
                Option<String>,
                String,
                Option<String>,
                Option<DateTime<Utc>>,
                String,
                DateTime<Utc>,
                DateTime<Utc>,
            ),
        >(
            r#"
            SELECT id, provider, email, display_name, access_token,
                   refresh_token, token_expiry, scopes,
                   created_at, updated_at
            FROM oauth_accounts
            WHERE provider = $1
            ORDER BY created_at ASC
            LIMIT 1
            "#,
        )
        .bind(provider)
        .fetch_optional(pool)
        .await?;

        Ok(result.map(
            |(
                id,
                provider,
                email,
                display_name,
                access_token,
                refresh_token,
                token_expiry,
                scopes,
                created_at,
                updated_at,
            )| {
                OAuthAccount {
                    id,
                    provider,
                    email,
                    display_name,
                    access_token,
                    refresh_token,
                    token_expiry,
                    scopes,
                    created_at,
                    updated_at,
                }
            },
        ))
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
        let results = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                Option<String>,
                Option<String>,
                String,
                DateTime<Utc>,
                DateTime<Utc>,
            ),
        >(
            r#"
            SELECT id, provider, email, display_name, scopes,
                   created_at, updated_at
            FROM oauth_accounts
            ORDER BY provider ASC, created_at ASC
            "#,
        )
        .fetch_all(pool)
        .await?;

        Ok(results
            .into_iter()
            .map(
                |(id, provider, email, display_name, scopes, created_at, updated_at)| {
                    OAuthAccountInfo {
                        id,
                        provider,
                        email,
                        display_name,
                        scopes,
                        created_at,
                        updated_at,
                    }
                },
            )
            .collect())
    }

    /// List all OAuth accounts including tokens (for env injection into scripts)
    pub async fn list_all_with_tokens(pool: &PgPool) -> Result<Vec<OAuthAccount>, sqlx::Error> {
        let results = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                Option<String>,
                Option<String>,
                String,
                Option<String>,
                Option<DateTime<Utc>>,
                String,
                DateTime<Utc>,
                DateTime<Utc>,
            ),
        >(
            r#"
            SELECT id, provider, email, display_name, access_token,
                   refresh_token, token_expiry, scopes,
                   created_at, updated_at
            FROM oauth_accounts
            ORDER BY provider ASC, created_at ASC
            "#,
        )
        .fetch_all(pool)
        .await?;

        Ok(results
            .into_iter()
            .map(
                |(
                    id,
                    provider,
                    email,
                    display_name,
                    access_token,
                    refresh_token,
                    token_expiry,
                    scopes,
                    created_at,
                    updated_at,
                )| {
                    OAuthAccount {
                        id,
                        provider,
                        email,
                        display_name,
                        access_token,
                        refresh_token,
                        token_expiry,
                        scopes,
                        created_at,
                        updated_at,
                    }
                },
            )
            .collect())
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

// ---------------------------------------------------------------------------
// Token exchange & refresh (HTTP)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
}

/// Exchange an authorization code for tokens.
pub async fn exchange_code(
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
pub async fn refresh_access_token(
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
    let turl = if let Some(wk) = well_known_provider(&account.provider) {
        wk.token_url.to_string()
    } else {
        config["token_url"]
            .as_str()
            .ok_or_else(|| format!("Missing token_url in {} credentials", cred_service))?
            .to_string()
    };

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

    // Determine endpoints
    let (auth_url, token_url, userinfo_url) = if let Some(config) = well_known_provider(provider) {
        (
            config.auth_url.to_string(),
            config.token_url.to_string(),
            config.userinfo_url.map(|s| s.to_string()),
        )
    } else {
        let auth = client_config["auth_url"]
            .as_str()
            .ok_or("Missing auth_url for custom provider")?;
        let token = client_config["token_url"]
            .as_str()
            .ok_or("Missing token_url for custom provider")?;
        let userinfo = client_config["userinfo_url"]
            .as_str()
            .map(|s| s.to_string());
        (auth.to_string(), token.to_string(), userinfo)
    };

    // Start temporary localhost listener for callback BEFORE returning the URL
    let listener = tokio::net::TcpListener::bind("127.0.0.1:14981").await?;
    let redirect_uri = "http://localhost:14981/oauth/callback".to_string();

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
                crate::engine::tools::credentials::wait_for_oauth_callback(listener),
            )
            .await
            .map_err(|_| "OAuth authorization timed out after 120 seconds".to_string())?
            .map_err(|e| format!("OAuth callback error: {}", e))?;

            // Exchange code for tokens
            let token_resp =
                exchange_code(&token_url, &client_id, &client_secret, &code, &redirect_uri)
                    .await
                    .map_err(|e| format!("Token exchange failed: {}", e))?;

            // Fetch userinfo
            let (email, display_name) = if let Some(ref url) = userinfo_url {
                fetch_userinfo(url, &token_resp.access_token)
                    .await
                    .unwrap_or((None, None))
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
/// Returns (email, display_name). On error, returns (None, None) — never fails.
pub async fn fetch_userinfo(
    userinfo_url: &str,
    access_token: &str,
) -> Result<(Option<String>, Option<String>), BoxError> {
    let client = reqwest::Client::new();
    let resp = client
        .get(userinfo_url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Accept", "application/json")
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            crate::log!(@OAuth, "Userinfo fetch failed: {}", e);
            return Ok((None, None));
        }
    };

    if !resp.status().is_success() {
        crate::log!(@OAuth, "Userinfo returned status {}", resp.status());
        return Ok((None, None));
    }

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            crate::log!(@OAuth, "Userinfo parse failed: {}", e);
            return Ok((None, None));
        }
    };

    let email = body.get("email").and_then(|v| v.as_str()).map(String::from);
    let name = body.get("name").and_then(|v| v.as_str()).map(String::from);

    Ok((email, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_scopes_adds_new_without_duplicates() {
        let existing = "openid email https://www.googleapis.com/auth/gmail.readonly";
        let requested = "https://www.googleapis.com/auth/calendar.readonly";
        let merged = merge_scopes(existing, requested);
        assert_eq!(
            merged,
            "openid email https://www.googleapis.com/auth/gmail.readonly https://www.googleapis.com/auth/calendar.readonly"
        );
    }

    #[test]
    fn merge_scopes_deduplicates() {
        let existing = "openid email";
        let requested = "email https://www.googleapis.com/auth/calendar.readonly";
        let merged = merge_scopes(existing, requested);
        assert_eq!(
            merged,
            "openid email https://www.googleapis.com/auth/calendar.readonly"
        );
    }

    #[test]
    fn merge_scopes_empty_existing() {
        let merged = merge_scopes("", "openid email");
        assert_eq!(merged, "openid email");
    }

    #[test]
    fn merge_scopes_empty_requested() {
        let merged = merge_scopes("openid email", "");
        assert_eq!(merged, "openid email");
    }

    #[test]
    fn merge_scopes_all_duplicates() {
        let merged = merge_scopes("openid email", "openid email");
        assert_eq!(merged, "openid email");
    }

    fn make_account(
        provider: &str,
        access_token: &str,
        refresh_token: Option<&str>,
        token_expiry: Option<DateTime<Utc>>,
    ) -> OAuthAccount {
        OAuthAccount {
            id: Uuid::new_v4(),
            provider: provider.to_string(),
            email: Some("test@example.com".to_string()),
            display_name: None,
            access_token: access_token.to_string(),
            refresh_token: refresh_token.map(|s| s.to_string()),
            token_expiry,
            scopes: "openid email".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn token_needs_refresh_when_expired() {
        let expired = Utc::now() - chrono::Duration::seconds(300);
        let account = make_account("google", "old-token", Some("refresh-tok"), Some(expired));
        assert!(
            token_needs_refresh(&account),
            "expired token should need refresh"
        );
    }

    #[test]
    fn token_needs_refresh_when_expiring_within_60s() {
        let soon = Utc::now() + chrono::Duration::seconds(30);
        let account = make_account("google", "token", Some("refresh-tok"), Some(soon));
        assert!(
            token_needs_refresh(&account),
            "token expiring in 30s should need refresh"
        );
    }

    #[test]
    fn token_does_not_need_refresh_when_valid() {
        let future = Utc::now() + chrono::Duration::seconds(3600);
        let account = make_account("google", "token", Some("refresh-tok"), Some(future));
        assert!(
            !token_needs_refresh(&account),
            "token valid for 1h should not need refresh"
        );
    }

    #[test]
    fn token_needs_refresh_when_expiry_null_with_refresh_token() {
        let account = make_account("google", "token", Some("refresh-tok"), None);
        assert!(
            token_needs_refresh(&account),
            "null expiry with refresh token should need refresh"
        );
    }

    #[test]
    fn token_does_not_need_refresh_when_expiry_null_without_refresh_token() {
        // GitHub-style: no expiry, no refresh token — token is long-lived
        let account = make_account("github", "ghp_token", None, None);
        assert!(
            !token_needs_refresh(&account),
            "null expiry without refresh token should not refresh"
        );
    }

    #[test]
    fn token_does_not_need_refresh_well_beyond_boundary() {
        // Token expiring in 61s — comfortably beyond the 60s buffer, no refresh needed
        let expiry = Utc::now() + chrono::Duration::seconds(61);
        let account = make_account("google", "token", Some("refresh-tok"), Some(expiry));
        assert!(
            !token_needs_refresh(&account),
            "token expiring in 61s should not need refresh"
        );
    }

    #[test]
    fn token_needs_refresh_at_59s() {
        let expiry = Utc::now() + chrono::Duration::seconds(59);
        let account = make_account("google", "token", Some("refresh-tok"), Some(expiry));
        assert!(
            token_needs_refresh(&account),
            "token expiring in 59s should need refresh"
        );
    }
}
