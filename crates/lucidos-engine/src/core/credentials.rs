use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::fmt;
use uuid::Uuid;

/// Authentication type for a stored credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    ApiKey,
    Bearer,
    Basic,
    Password,
    OauthClient,
    EmailPassword,
}

impl AuthType {
    /// Parse from a database string, defaulting to `ApiKey` for unknown values.
    pub fn parse(s: &str) -> Self {
        match s {
            "api_key" => Self::ApiKey,
            "bearer" => Self::Bearer,
            "basic" => Self::Basic,
            "password" => Self::Password,
            "oauth_client" => Self::OauthClient,
            "email_password" => Self::EmailPassword,
            _ => Self::ApiKey,
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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Full credential including the secret value
#[derive(Debug, Clone)]
pub struct Credential {
    pub id: Uuid,
    pub service_name: String,
    pub base_url: String,
    pub auth_type: AuthType,
    pub auth_value: String,
    pub auth_header: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Parse a credential row tuple from the database (with secret).
fn parse_credential(
    row: (
        Uuid,
        String,
        String,
        String,
        String,
        String,
        DateTime<Utc>,
        DateTime<Utc>,
    ),
) -> Credential {
    let (id, service_name, base_url, auth_type, auth_value, auth_header, created_at, updated_at) =
        row;
    Credential {
        id,
        service_name,
        base_url,
        auth_type: AuthType::parse(&auth_type),
        auth_value,
        auth_header,
        created_at,
        updated_at,
    }
}

/// Parse a credential info row tuple from the database (without secret).
fn parse_credential_info(
    row: (
        Uuid,
        String,
        String,
        String,
        String,
        DateTime<Utc>,
        DateTime<Utc>,
    ),
) -> CredentialInfo {
    let (id, service_name, base_url, auth_type, auth_header, created_at, updated_at) = row;
    CredentialInfo {
        id,
        service_name,
        base_url,
        auth_type: AuthType::parse(&auth_type),
        auth_header,
        created_at,
        updated_at,
    }
}

/// Store for managing API credentials in the database
pub struct CredentialStore;

impl CredentialStore {
    /// Initialize the credentials table schema
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
    ) -> Result<Uuid, sqlx::Error> {
        let auth_header = auth_header.unwrap_or("Authorization");

        let result = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO credentials (service_name, base_url, auth_type, auth_value, auth_header)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (service_name) DO UPDATE SET
                base_url = EXCLUDED.base_url,
                auth_type = EXCLUDED.auth_type,
                auth_value = EXCLUDED.auth_value,
                auth_header = EXCLUDED.auth_header,
                updated_at = NOW()
            RETURNING id
            "#,
        )
        .bind(service_name)
        .bind(base_url)
        .bind(auth_type.to_string())
        .bind(auth_value)
        .bind(auth_header)
        .fetch_one(pool)
        .await?;

        Ok(result)
    }

    /// Get a credential by service name (includes the secret)
    pub async fn get(pool: &PgPool, service_name: &str) -> Result<Option<Credential>, sqlx::Error> {
        let result = sqlx::query_as::<_, (Uuid, String, String, String, String, String, DateTime<Utc>, DateTime<Utc>)>(
            r#"
            SELECT id, service_name, base_url, auth_type, auth_value, auth_header, created_at, updated_at
            FROM credentials
            WHERE service_name = $1
            "#,
        )
        .bind(service_name)
        .fetch_optional(pool)
        .await?;

        Ok(result.map(parse_credential))
    }

    /// List all credentials (without secrets)
    pub async fn list(pool: &PgPool) -> Result<Vec<CredentialInfo>, sqlx::Error> {
        let results = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                String,
                String,
                DateTime<Utc>,
                DateTime<Utc>,
            ),
        >(
            r#"
            SELECT id, service_name, base_url, auth_type, auth_header, created_at, updated_at
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
        let results = sqlx::query_as::<_, (Uuid, String, String, String, String, String, DateTime<Utc>, DateTime<Utc>)>(
            r#"
            SELECT id, service_name, base_url, auth_type, auth_value, auth_header, created_at, updated_at
            FROM credentials
            ORDER BY service_name ASC
            "#,
        )
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

    /// Update just the auth_value for an existing credential
    pub async fn update_value(
        pool: &PgPool,
        service_name: &str,
        auth_value: &str,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE credentials
            SET auth_value = $2, updated_at = NOW()
            WHERE service_name = $1
            "#,
        )
        .bind(service_name)
        .bind(auth_value)
        .execute(pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Find a credential whose base_url matches the given URL prefix
    pub async fn find_by_url(pool: &PgPool, url: &str) -> Result<Option<Credential>, sqlx::Error> {
        let results = sqlx::query_as::<_, (Uuid, String, String, String, String, String, DateTime<Utc>, DateTime<Utc>)>(
            r#"
            SELECT id, service_name, base_url, auth_type, auth_value, auth_header, created_at, updated_at
            FROM credentials
            ORDER BY length(base_url) DESC
            "#,
        )
        .fetch_all(pool)
        .await?;

        for row in results {
            if url.starts_with(&row.2) {
                return Ok(Some(parse_credential(row)));
            }
        }

        Ok(None)
    }
}

/// Build CRED_* environment variables from a list of credentials.
/// - `password` type: emits `CRED_{NAME}_USERNAME` and `CRED_{NAME}_PASSWORD`
///   from the JSON-encoded auth_value.
/// - other types (api_key, bearer, basic): emits `CRED_{NAME}` with the raw
///   auth_value.
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
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&cred.auth_value) {
                let username = parsed["username"].as_str().unwrap_or("");
                let password = parsed["password"].as_str().unwrap_or("");
                env_vars.push((format!("{}_USERNAME", prefix), username.to_string()));
                env_vars.push((format!("{}_PASSWORD", prefix), password.to_string()));
            }
        } else {
            env_vars.push((prefix, cred.auth_value));
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
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
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
        assert_eq!(env.get("CRED_COMFORT_CLOUD_USERNAME").map(String::as_str), Some("alice"));
        assert_eq!(env.get("CRED_COMFORT_CLOUD_PASSWORD").map(String::as_str), Some("s3cret"));
        assert!(env.get("CRED_COMFORT_CLOUD").is_none());
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
        assert!(env.get("CRED_FIREBASE_WEB_API_KEY_USERNAME").is_none());
        assert!(env.get("CRED_FIREBASE_WEB_API_KEY_PASSWORD").is_none());
    }

    #[test]
    fn bearer_emits_single_cred_var() {
        let cred = make_cred("openai-key", AuthType::Bearer, "sk-test-123");
        let env: std::collections::HashMap<_, _> =
            credential_env_vars(vec![cred]).into_iter().collect();
        assert_eq!(env.get("CRED_OPENAI_KEY").map(String::as_str), Some("sk-test-123"));
        assert!(env.get("CRED_OPENAI_KEY_USERNAME").is_none());
    }

    #[test]
    fn basic_emits_single_cred_var_with_user_colon_password() {
        // `basic` stores `user:password` literally; we hand the whole string
        // to the script as `CRED_<NAME>` and let the script decide how to
        // split it. Mirrors what `run_python` / `run_bash` already do.
        let cred = make_cred("svc-basic", AuthType::Basic, "alice:s3cret");
        let env: std::collections::HashMap<_, _> =
            credential_env_vars(vec![cred]).into_iter().collect();
        assert_eq!(env.get("CRED_SVC_BASIC").map(String::as_str), Some("alice:s3cret"));
        assert!(env.get("CRED_SVC_BASIC_USERNAME").is_none());
        assert!(env.get("CRED_SVC_BASIC_PASSWORD").is_none());
    }

    #[test]
    fn name_transform_uppercases_and_replaces_separators() {
        let cred = make_cred("snake.storage familien-prod", AuthType::ApiKey, "v");
        let env: std::collections::HashMap<_, _> =
            credential_env_vars(vec![cred]).into_iter().collect();
        assert_eq!(
            env.get("CRED_SNAKE_STORAGE_FAMILIEN_PROD").map(String::as_str),
            Some("v")
        );
    }
}
