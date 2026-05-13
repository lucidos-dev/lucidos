use sqlx::PgPool;
use std::collections::HashMap;

// Background model preference keys
pub const PREF_MODEL_TITLE: &str = "model_title";
pub const PREF_MODEL_IMAGE_DESCRIPTION: &str = "model_image_description";
pub const PREF_MODEL_MEMORY: &str = "model_memory";

// Chat preference keys (also written by frontend Settings UI)
pub const PREF_CHAT_MODEL: &str = "chat_model";
pub const PREF_CHAT_REASONING_EFFORT: &str = "chat_reasoning_effort";

/// Default chat model when neither user preference nor `LUCIDOS_MODEL` env is set.
/// Mirrored on the frontend in `crates/lucidos-app/src/store/models.ts`.
pub const DEFAULT_CHAT_MODEL: &str = "claude-opus-4-7";

// Vertex AI configuration
pub const PREF_VERTEX_REGION: &str = "vertex_region";

// Image generation model (also written by frontend Settings UI)
pub const PREF_IMAGE_MODEL: &str = "image_model";

// When off, ContextCaptured still fires per LLM call but section
// bodies are dropped — only the name + char_count survives. Defaults
// to "true" to preserve historical behavior.
pub const PREF_CAPTURE_CONTEXT: &str = "capture_context";

/// Store for managing user preferences in the database
pub struct PreferenceStore;

impl PreferenceStore {
    /// Initialize the preferences table schema
    pub async fn init_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS preferences (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Set a global preference (insert or update, device_id IS NULL)
    pub async fn set(pool: &PgPool, key: &str, value: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO preferences (key, value, device_id, updated_at)
            VALUES ($1, $2, NULL, NOW())
            ON CONFLICT (key, COALESCE(device_id, '')) DO UPDATE SET
                value = EXCLUDED.value,
                updated_at = NOW()
            "#,
        )
        .bind(key)
        .bind(value)
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Set a per-device preference (insert or update)
    pub async fn set_for_device(
        pool: &PgPool,
        key: &str,
        value: &str,
        device_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO preferences (key, value, device_id, updated_at)
            VALUES ($1, $2, $3, NOW())
            ON CONFLICT (key, COALESCE(device_id, '')) DO UPDATE SET
                value = EXCLUDED.value,
                updated_at = NOW()
            "#,
        )
        .bind(key)
        .bind(value)
        .bind(device_id)
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Get a global preference by key (device_id IS NULL)
    pub async fn get(pool: &PgPool, key: &str) -> Result<Option<String>, sqlx::Error> {
        let result = sqlx::query_scalar::<_, String>(
            "SELECT value FROM preferences WHERE key = $1 AND device_id IS NULL",
        )
        .bind(key)
        .fetch_optional(pool)
        .await?;

        Ok(result)
    }

    /// Get a preference for a specific device, falling back to the global value
    pub async fn get_for_device(
        pool: &PgPool,
        key: &str,
        device_id: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        // Try device-specific first
        let result = sqlx::query_scalar::<_, String>(
            "SELECT value FROM preferences WHERE key = $1 AND device_id = $2",
        )
        .bind(key)
        .bind(device_id)
        .fetch_optional(pool)
        .await?;

        if result.is_some() {
            return Ok(result);
        }

        // Fall back to global
        Self::get(pool, key).await
    }

    /// Get all global preferences as a HashMap
    pub async fn get_all(pool: &PgPool) -> Result<HashMap<String, String>, sqlx::Error> {
        let results = sqlx::query_as::<_, (String, String)>(
            "SELECT key, value FROM preferences WHERE device_id IS NULL ORDER BY key ASC",
        )
        .fetch_all(pool)
        .await?;

        Ok(results.into_iter().collect())
    }

    /// Get merged preferences for a device: global values overridden by device-specific ones
    pub async fn get_all_for_device(
        pool: &PgPool,
        device_id: &str,
    ) -> Result<HashMap<String, String>, sqlx::Error> {
        // Start with global preferences
        let mut map = Self::get_all(pool).await?;

        // Override with device-specific preferences
        let device_results = sqlx::query_as::<_, (String, String)>(
            "SELECT key, value FROM preferences WHERE device_id = $1 ORDER BY key ASC",
        )
        .bind(device_id)
        .fetch_all(pool)
        .await?;

        for (key, value) in device_results {
            map.insert(key, value);
        }

        Ok(map)
    }

    /// Delete a global preference by key
    pub async fn delete(pool: &PgPool, key: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM preferences WHERE key = $1 AND device_id IS NULL")
            .bind(key)
            .execute(pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Check if a global preference exists
    pub async fn exists(pool: &PgPool, key: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM preferences WHERE key = $1 AND device_id IS NULL)",
        )
        .bind(key)
        .fetch_one(pool)
        .await?;

        Ok(result)
    }

    /// Read the per-step context-capture toggle. Defaults to `true` when
    /// unset or on DB error so existing workspaces keep capturing.
    pub async fn capture_context(pool: &PgPool) -> bool {
        match Self::get(pool, PREF_CAPTURE_CONTEXT).await {
            Ok(Some(v)) => v != "false",
            Ok(None) => true,
            Err(e) => {
                log!(
                    "[Preferences] Failed to read {}: {}",
                    PREF_CAPTURE_CONTEXT,
                    e
                );
                true
            }
        }
    }

    /// Read the user's chat model + reasoning effort preferences for code
    /// paths that originate a chat without an explicit user request
    /// (spawn_thread, process_trigger). DB errors are logged and treated as
    /// "unset" — callers fall back to the engine default.
    pub async fn user_chat_settings(pool: &PgPool) -> (Option<String>, Option<String>) {
        let model = Self::get(pool, PREF_CHAT_MODEL).await.unwrap_or_else(|e| {
            log!("[Preferences] Failed to read {}: {}", PREF_CHAT_MODEL, e);
            None
        });
        let effort = Self::get(pool, PREF_CHAT_REASONING_EFFORT)
            .await
            .unwrap_or_else(|e| {
                log!(
                    "[Preferences] Failed to read {}: {}",
                    PREF_CHAT_REASONING_EFFORT,
                    e
                );
                None
            });
        (model, effort)
    }

    /// Resolve the (model, effort) pair stamped on a chat exchange when the
    /// caller didn't fully specify them. Caller-provided values take
    /// precedence; missing values fall back to the user's chat preferences.
    /// Skips the DB read entirely when both are already set.
    pub async fn resolve_chat_overrides(
        pool: &PgPool,
        model_override: Option<String>,
        effort_override: Option<String>,
    ) -> (Option<String>, Option<String>) {
        if model_override.is_some() && effort_override.is_some() {
            return (model_override, effort_override);
        }
        let (pref_model, pref_effort) = Self::user_chat_settings(pool).await;
        (
            model_override.or(pref_model),
            effort_override.or(pref_effort),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    #[tokio::test]
    async fn user_chat_settings_returns_none_when_unset() {
        let (pool, db_name) = setup_test_db().await;
        let (model, effort) = PreferenceStore::user_chat_settings(&pool).await;
        assert_eq!(model, None);
        assert_eq!(effort, None);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn capture_context_defaults_to_true_when_unset() {
        let (pool, db_name) = setup_test_db().await;
        assert!(PreferenceStore::capture_context(&pool).await);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn capture_context_returns_false_when_disabled() {
        let (pool, db_name) = setup_test_db().await;
        PreferenceStore::set(&pool, PREF_CAPTURE_CONTEXT, "false")
            .await
            .unwrap();
        assert!(!PreferenceStore::capture_context(&pool).await);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn capture_context_returns_true_when_explicitly_true() {
        let (pool, db_name) = setup_test_db().await;
        PreferenceStore::set(&pool, PREF_CAPTURE_CONTEXT, "true")
            .await
            .unwrap();
        assert!(PreferenceStore::capture_context(&pool).await);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn user_chat_settings_returns_stored_values() {
        let (pool, db_name) = setup_test_db().await;
        PreferenceStore::set(&pool, PREF_CHAT_MODEL, "claude-opus-4-7[1m]")
            .await
            .unwrap();
        PreferenceStore::set(&pool, PREF_CHAT_REASONING_EFFORT, "max")
            .await
            .unwrap();
        let (model, effort) = PreferenceStore::user_chat_settings(&pool).await;
        assert_eq!(model.as_deref(), Some("claude-opus-4-7[1m]"));
        assert_eq!(effort.as_deref(), Some("max"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    async fn seed_chat_prefs(pool: &PgPool, model: &str, effort: &str) {
        PreferenceStore::set(pool, PREF_CHAT_MODEL, model)
            .await
            .unwrap();
        PreferenceStore::set(pool, PREF_CHAT_REASONING_EFFORT, effort)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn resolve_chat_overrides_falls_back_to_prefs_when_none() {
        let (pool, db_name) = setup_test_db().await;
        seed_chat_prefs(&pool, "claude-opus-4-7[1m]", "xhigh").await;
        let (model, effort) =
            PreferenceStore::resolve_chat_overrides(&pool, None, None).await;
        assert_eq!(model.as_deref(), Some("claude-opus-4-7[1m]"));
        assert_eq!(effort.as_deref(), Some("xhigh"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_chat_overrides_keeps_explicit_caller_values() {
        let (pool, db_name) = setup_test_db().await;
        seed_chat_prefs(&pool, "claude-opus-4-7[1m]", "xhigh").await;
        let (model, effort) = PreferenceStore::resolve_chat_overrides(
            &pool,
            Some("claude-sonnet-4-6".to_string()),
            Some("medium".to_string()),
        )
        .await;
        assert_eq!(model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(effort.as_deref(), Some("medium"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_chat_overrides_mixes_caller_and_prefs() {
        let (pool, db_name) = setup_test_db().await;
        seed_chat_prefs(&pool, "claude-opus-4-7[1m]", "xhigh").await;
        let (model, effort) = PreferenceStore::resolve_chat_overrides(
            &pool,
            Some("claude-sonnet-4-6".to_string()),
            None,
        )
        .await;
        assert_eq!(model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(effort.as_deref(), Some("xhigh"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_chat_overrides_returns_none_when_no_caller_no_prefs() {
        let (pool, db_name) = setup_test_db().await;
        let (model, effort) =
            PreferenceStore::resolve_chat_overrides(&pool, None, None).await;
        assert_eq!(model, None);
        assert_eq!(effort, None);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
