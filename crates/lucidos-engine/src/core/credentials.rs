use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::fmt;
use uuid::Uuid;

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
    pub base_url: String,
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
    pub base_url: String,
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
            .field("base_url", &self.base_url)
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
    String,
    String,
    String,
    String,
    DateTime<Utc>,
    DateTime<Utc>,
    Option<String>,
);

/// Columns selected for a full credential (with secret), in `CredentialRow` order.
const CREDENTIAL_COLUMNS: &str =
    "id, service_name, base_url, auth_type, auth_value, auth_header, created_at, updated_at, env_var_name";

/// Parse a credential row tuple from the database (with secret).
fn parse_credential(row: CredentialRow) -> Credential {
    let (
        id,
        service_name,
        base_url,
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
        base_url,
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
    String,
    String,
    String,
    DateTime<Utc>,
    DateTime<Utc>,
    Option<String>,
);

/// Parse a credential info row tuple from the database (without secret).
fn parse_credential_info(row: CredentialInfoRow) -> CredentialInfo {
    let (id, service_name, base_url, auth_type, auth_header, created_at, updated_at, env_var_name) =
        row;
    CredentialInfo {
        id,
        service_name,
        base_url,
        auth_type: AuthType::parse(&auth_type),
        auth_header,
        env_var_name,
        created_at,
        updated_at,
    }
}

/// Store for managing API credentials in the database
pub struct CredentialStore;

impl CredentialStore {
    /// Defensive double-write — the migration owns this CREATE TABLE
    /// (see `20260517160627_consolidate_init_schema_tables.sql`). Slated
    /// for removal in `harden-init-schema-tables-vs-migrations-pattern-finish`.
    pub async fn init_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS credentials (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                service_name TEXT UNIQUE NOT NULL,
                base_url TEXT NOT NULL,
                auth_type TEXT NOT NULL,
                auth_value TEXT NOT NULL,
                auth_header TEXT DEFAULT 'Authorization',
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Insert or update a credential (upsert by service_name)
    pub async fn upsert(
        pool: &PgPool,
        service_name: &str,
        base_url: &str,
        auth_type: AuthType,
        auth_value: &str,
        auth_header: Option<&str>,
        env_var_name: Option<&str>,
    ) -> Result<Uuid, sqlx::Error> {
        let auth_header = auth_header.unwrap_or("Authorization");

        let result = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO credentials (service_name, base_url, auth_type, auth_value, auth_header, env_var_name)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (service_name) DO UPDATE SET
                base_url = EXCLUDED.base_url,
                auth_type = EXCLUDED.auth_type,
                auth_value = EXCLUDED.auth_value,
                auth_header = EXCLUDED.auth_header,
                env_var_name = EXCLUDED.env_var_name,
                updated_at = NOW()
            RETURNING id
            "#,
        )
        .bind(service_name)
        .bind(base_url)
        .bind(auth_type.to_string())
        .bind(auth_value)
        .bind(auth_header)
        .bind(env_var_name)
        .fetch_one(pool)
        .await?;

        Ok(result)
    }

    /// Get a credential by service name (includes the secret)
    pub async fn get(pool: &PgPool, service_name: &str) -> Result<Option<Credential>, sqlx::Error> {
        let result = sqlx::query_as::<_, CredentialRow>(&format!(
            "SELECT {CREDENTIAL_COLUMNS} FROM credentials WHERE service_name = $1"
        ))
        .bind(service_name)
        .fetch_optional(pool)
        .await?;

        Ok(result.map(parse_credential))
    }

    /// List all credentials (without secrets)
    pub async fn list(pool: &PgPool) -> Result<Vec<CredentialInfo>, sqlx::Error> {
        let results = sqlx::query_as::<_, CredentialInfoRow>(
            r#"
            SELECT id, service_name, base_url, auth_type, auth_header, created_at, updated_at, env_var_name
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

    /// Delete a credential by service name
    pub async fn delete(pool: &PgPool, service_name: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM credentials WHERE service_name = $1")
            .bind(service_name)
            .execute(pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Update an existing credential's editable fields (everything except the
    /// immutable `service_name`). `auth_value: None` keeps the stored secret
    /// untouched — used when the user edits non-secret fields without
    /// re-entering the secret. Returns whether a row existed.
    pub async fn update(
        pool: &PgPool,
        service_name: &str,
        base_url: &str,
        auth_type: AuthType,
        auth_header: Option<&str>,
        auth_value: Option<&str>,
        env_var_name: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let auth_header = auth_header.unwrap_or("Authorization");

        // Two static queries instead of a dynamic one: the only difference is
        // whether `auth_value` is touched, and sqlx wants compile-time SQL.
        // `env_var_name` is always set (NULL clears it back to the `CRED_` default).
        let result = match auth_value {
            Some(value) => {
                sqlx::query(
                    r#"
                    UPDATE credentials
                    SET base_url = $2, auth_type = $3, auth_header = $4, auth_value = $5, env_var_name = $6, updated_at = NOW()
                    WHERE service_name = $1
                    "#,
                )
                .bind(service_name)
                .bind(base_url)
                .bind(auth_type.to_string())
                .bind(auth_header)
                .bind(value)
                .bind(env_var_name)
                .execute(pool)
                .await?
            }
            None => {
                sqlx::query(
                    r#"
                    UPDATE credentials
                    SET base_url = $2, auth_type = $3, auth_header = $4, env_var_name = $5, updated_at = NOW()
                    WHERE service_name = $1
                    "#,
                )
                .bind(service_name)
                .bind(base_url)
                .bind(auth_type.to_string())
                .bind(auth_header)
                .bind(env_var_name)
                .execute(pool)
                .await?
            }
        };

        Ok(result.rows_affected() > 0)
    }

    /// Find a credential whose base_url scopes the given URL.
    ///
    /// Matching is URL-aware: same scheme, host, effective port, and a path
    /// prefix on a segment boundary. A raw string prefix check would inject a
    /// credential scoped to `https://api.example.com` into
    /// `https://api.example.com.evil.test/`.
    pub async fn find_by_url(pool: &PgPool, url: &str) -> Result<Option<Credential>, sqlx::Error> {
        let results = sqlx::query_as::<_, CredentialRow>(&format!(
            "SELECT {CREDENTIAL_COLUMNS} FROM credentials ORDER BY length(base_url) DESC"
        ))
        .fetch_all(pool)
        .await?;

        for row in results {
            if credential_base_url_matches(&row.2, url) {
                return Ok(Some(parse_credential(row)));
            }
        }

        Ok(None)
    }
}

fn credential_base_url_matches(base_url: &str, request_url: &str) -> bool {
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
/// `{NAME}` is the credential's `service_name` uppercased with `-`/`.`/space
/// replaced by `_`.
pub fn credential_env_vars(credentials: Vec<Credential>) -> Vec<(String, String)> {
    let mut env_vars = Vec::new();
    for cred in credentials {
        let env_name = cred
            .service_name
            .to_uppercase()
            .replace(['-', ' ', '.'], "_");
        let prefix = format!("CRED_{}", env_name);

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

    fn make_cred(service_name: &str, auth_type: AuthType, auth_value: &str) -> Credential {
        Credential {
            id: Uuid::nil(),
            service_name: service_name.to_string(),
            base_url: String::new(),
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
        assert_eq!(pairs.len(), 1, "custom name equal to the default must not double-emit");
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

    #[test]
    fn debug_redacts_credential_secret() {
        let cred = make_cred(
            "oauth:google",
            AuthType::OauthClient,
            r#"{"client_id":"abc","client_secret":"super-secret-value"}"#,
        );
        let dbg = format!("{:?}", cred);
        // The secret `auth_value` must never appear in a `{:?}` rendering.
        assert!(
            !dbg.contains("super-secret-value"),
            "secret leaked: {dbg}"
        );
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
}
