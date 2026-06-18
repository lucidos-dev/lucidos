//! Database-backed registry of chat models for the Lucidos Agent picker.
//!
//! Plain config table (authoritative), mirroring the `mcp_servers` / `credentials`
//! store pattern: the migration seeds builtins, the HTTP API mutates user rows,
//! and CRUD emits audit `Model*` SystemEvents. The table drives the chat model
//! picker and `RoutingProvider`'s provider selection; the Claude Code `/model`
//! picker stays hand-maintained in `runtime/cc_menu_options.json`.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// `source = 'builtin'` rows are seeded by migration: disable-only, never
/// deletable (deleting one could orphan a user's saved `chat_model` pref).
pub const SOURCE_BUILTIN: &str = "builtin";
/// `source = 'user'` rows are added in Settings: fully editable and deletable.
pub const SOURCE_USER: &str = "user";

/// A chat model in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    /// Value sent in the API request (e.g. "claude-fable-5",
    /// "claude-opus-4-8@default[1m]"). Primary key.
    pub id: String,
    pub label: String,
    /// Backend that serves the model: "vertex" | "anthropic" | "openai".
    pub provider: String,
    pub sort_order: i32,
    /// [`SOURCE_BUILTIN`] or [`SOURCE_USER`].
    pub source: String,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl Model {
    pub fn is_builtin(&self) -> bool {
        self.source == SOURCE_BUILTIN
    }
}

/// Raw DB row: (id, label, provider, sort_order, source, enabled, created_at).
type ModelRow = (
    String,
    String,
    String,
    i32,
    String,
    bool,
    chrono::DateTime<chrono::Utc>,
);

const SELECT_COLS: &str = "id, label, provider, sort_order, source, enabled, created_at";

fn row_to_model(row: ModelRow) -> Model {
    let (id, label, provider, sort_order, source, enabled, created_at) = row;
    Model {
        id,
        label,
        provider,
        sort_order,
        source,
        enabled,
        created_at,
    }
}

pub struct ModelStore;

impl ModelStore {
    /// All models, ordered for display (sort_order, then label). Includes
    /// disabled rows — the picker filters to `enabled`, the registry needs all
    /// so routing resolves a model even if it was disabled after being saved.
    pub async fn list(pool: &PgPool) -> Result<Vec<Model>, sqlx::Error> {
        let rows: Vec<ModelRow> = sqlx::query_as(&format!(
            "SELECT {SELECT_COLS} FROM models ORDER BY sort_order ASC, label ASC"
        ))
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(row_to_model).collect())
    }

    pub async fn get(pool: &PgPool, id: &str) -> Result<Option<Model>, sqlx::Error> {
        let row: Option<ModelRow> =
            sqlx::query_as(&format!("SELECT {SELECT_COLS} FROM models WHERE id = $1"))
                .bind(id)
                .fetch_one(pool)
                .await
                .map(Some)
                .or_else(|e| match e {
                    sqlx::Error::RowNotFound => Ok(None),
                    other => Err(other),
                })?;
        Ok(row.map(row_to_model))
    }

    /// Insert a user-added model. Errors (unique violation) if `id` already
    /// exists — the caller maps that to a 4xx so the user can pick another id.
    pub async fn create(
        pool: &PgPool,
        id: &str,
        label: &str,
        provider: &str,
        sort_order: i32,
    ) -> Result<Model, sqlx::Error> {
        let row: ModelRow = sqlx::query_as(&format!(
            "INSERT INTO models (id, label, provider, sort_order, source, enabled) \
             VALUES ($1, $2, $3, $4, '{SOURCE_USER}', TRUE) \
             RETURNING {SELECT_COLS}"
        ))
        .bind(id)
        .bind(label)
        .bind(provider)
        .bind(sort_order)
        .fetch_one(pool)
        .await?;
        Ok(row_to_model(row))
    }

    /// Update the editable fields of a user model (never `source` or `id`).
    /// Returns whether a row existed.
    pub async fn update(
        pool: &PgPool,
        id: &str,
        label: &str,
        provider: &str,
        sort_order: i32,
        enabled: bool,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE models SET label = $2, provider = $3, sort_order = $4, enabled = $5, \
             updated_at = NOW() WHERE id = $1",
        )
        .bind(id)
        .bind(label)
        .bind(provider)
        .bind(sort_order)
        .bind(enabled)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Toggle a model's enabled flag without touching its other fields. Works on
    /// builtin rows too (the disable-only path). Returns whether a row existed.
    pub async fn set_enabled(
        pool: &PgPool,
        id: &str,
        enabled: bool,
    ) -> Result<bool, sqlx::Error> {
        let result =
            sqlx::query("UPDATE models SET enabled = $2, updated_at = NOW() WHERE id = $1")
                .bind(id)
                .bind(enabled)
                .execute(pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Delete a model by id. The caller guards against deleting builtins.
    pub async fn delete(pool: &PgPool, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM models WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    #[tokio::test]
    async fn migration_seeds_builtins_including_fable_5() {
        let (pool, db_name) = setup_test_db().await;
        let models = ModelStore::list(&pool).await.unwrap();
        assert!(
            models.iter().any(|m| m.id == "claude-fable-5"
                && m.provider == "anthropic"
                && m.is_builtin()),
            "Fable 5 builtin must be seeded on the anthropic provider"
        );
        assert!(
            models.iter().any(|m| m.id == "claude-opus-4-8@default" && m.provider == "vertex"),
            "existing Vertex builtins must be seeded"
        );
        // Ordered by sort_order — Fable 5 (0) sorts before Opus 4.8 (10).
        let fable = models.iter().position(|m| m.id == "claude-fable-5").unwrap();
        let opus = models
            .iter()
            .position(|m| m.id == "claude-opus-4-8@default")
            .unwrap();
        assert!(fable < opus, "sort_order must drive display order");
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn migration_seeds_glm_5_2_on_openrouter() {
        let (pool, db_name) = setup_test_db().await;
        let m = ModelStore::get(&pool, "z-ai/glm-5.2")
            .await
            .unwrap()
            .expect("GLM 5.2 builtin must be seeded");
        assert_eq!(m.provider, "openrouter");
        assert_eq!(m.label, "GLM 5.2");
        assert!(m.is_builtin(), "GLM 5.2 must be a builtin (disable-only)");
        assert!(m.enabled, "GLM 5.2 builtin is enabled by default");
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn create_update_delete_user_model_round_trips() {
        let (pool, db_name) = setup_test_db().await;

        let created = ModelStore::create(&pool, "my-model", "My Model", "anthropic", 99)
            .await
            .unwrap();
        assert_eq!(created.source, SOURCE_USER);
        assert!(created.enabled);
        assert!(!created.is_builtin());

        assert!(ModelStore::update(&pool, "my-model", "Renamed", "vertex", 5, false)
            .await
            .unwrap());
        let fetched = ModelStore::get(&pool, "my-model").await.unwrap().unwrap();
        assert_eq!(fetched.label, "Renamed");
        assert_eq!(fetched.provider, "vertex");
        assert!(!fetched.enabled);
        // source is immutable through update
        assert_eq!(fetched.source, SOURCE_USER);

        assert!(ModelStore::delete(&pool, "my-model").await.unwrap());
        assert!(ModelStore::get(&pool, "my-model").await.unwrap().is_none());

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn set_enabled_toggles_builtin_without_other_changes() {
        let (pool, db_name) = setup_test_db().await;
        assert!(ModelStore::set_enabled(&pool, "claude-fable-5", false)
            .await
            .unwrap());
        let m = ModelStore::get(&pool, "claude-fable-5").await.unwrap().unwrap();
        assert!(!m.enabled);
        assert_eq!(m.label, "Fable 5", "toggle must not touch the label");
        assert!(m.is_builtin(), "toggle must not change source");
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn create_duplicate_id_errors() {
        let (pool, db_name) = setup_test_db().await;
        // Colliding with a seeded builtin id must fail (unique PK violation) so
        // the API can return a clear "already exists" rather than silently
        // overwriting a builtin.
        let result = ModelStore::create(&pool, "claude-fable-5", "Dupe", "anthropic", 1).await;
        assert!(result.is_err(), "duplicate id must error");
        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
