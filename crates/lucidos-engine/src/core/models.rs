//! Database-backed registry of chat models for the Lucidos Agent picker.
//!
//! Plain config table (authoritative), mirroring the `mcp_servers` / `credentials`
//! store pattern: the migration seeds builtins, the HTTP API mutates user rows,
//! and CRUD emits audit `Model*` SystemEvents. The table drives the chat model
//! picker and `RoutingProvider`'s provider selection; the Claude Code `/model`
//! picker stays hand-maintained in `runtime/cc_menu_options.json`.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

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
    /// Backend that serves the model:
    /// "vertex" | "anthropic" | "openai" | "openrouter" | "local".
    pub provider: String,
    pub sort_order: i32,
    /// [`SOURCE_BUILTIN`] or [`SOURCE_USER`].
    pub source: String,
    pub enabled: bool,
    /// Declared context window in tokens. `None` = not declared, so
    /// `engine::context::context_window_from_prefix` decides from the id shape.
    /// Only worth setting for ids the prefix map gets wrong — every OpenRouter /
    /// Gemini / local model, which otherwise takes the 200k fallback.
    pub context_window: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl Model {
    pub fn is_builtin(&self) -> bool {
        self.source == SOURCE_BUILTIN
    }
}

/// Raw DB row: (id, label, provider, sort_order, source, enabled,
/// context_window, created_at).
type ModelRow = (
    String,
    String,
    String,
    i32,
    String,
    bool,
    Option<i32>,
    chrono::DateTime<chrono::Utc>,
);

const SELECT_COLS: &str =
    "id, label, provider, sort_order, source, enabled, context_window, created_at";

fn row_to_model(row: ModelRow) -> Model {
    let (id, label, provider, sort_order, source, enabled, context_window, created_at) = row;
    Model {
        id,
        label,
        provider,
        sort_order,
        source,
        enabled,
        context_window,
        created_at,
    }
}

/// The chat model registry.
///
/// **No caller can skip the event.** [`Self::create`], [`Self::update`],
/// [`Self::set_enabled`] and [`Self::delete`] are the only reachable mutators;
/// the raw row writes are private to this module. `Model{Created,Updated,
/// Deleted}` is what makes the in-memory `ModelRegistry` reload
/// (`spawn_models_registry_subscriber`) and the picker update without a
/// restart, so a silent write would leave the registry serving a stale model
/// list until the next boot.
///
/// Same shape and the same reachability-not-atomicity guarantee as
/// `RepositoryStore`; see `core::announced_surfaces`.
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
                .fetch_optional(pool)
                .await?;
        Ok(row.map(row_to_model))
    }

    /// Insert a user-added model row. **Private on purpose**: [`Self::create`]
    /// is the reachable mutator, and it emits.
    async fn insert_row(
        pool: &PgPool,
        id: &str,
        label: &str,
        provider: &str,
        sort_order: i32,
        context_window: Option<i32>,
    ) -> Result<Model, sqlx::Error> {
        let row: ModelRow = sqlx::query_as(&format!(
            "INSERT INTO models (id, label, provider, sort_order, source, enabled, context_window) \
             VALUES ($1, $2, $3, $4, '{SOURCE_USER}', TRUE, $5) \
             RETURNING {SELECT_COLS}"
        ))
        .bind(id)
        .bind(label)
        .bind(provider)
        .bind(sort_order)
        .bind(context_window)
        .fetch_one(pool)
        .await?;
        Ok(row_to_model(row))
    }

    /// Update the editable fields of a user model (never `source` or `id`).
    /// `context_window` is written as given — `None` clears the declaration and
    /// hands the model back to the prefix-map fallback, so the caller must
    /// resolve "field absent from the request" to the existing value before
    /// calling. Returns whether a row existed.
    async fn update_row(
        pool: &PgPool,
        id: &str,
        label: &str,
        provider: &str,
        sort_order: i32,
        enabled: bool,
        context_window: Option<i32>,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE models SET label = $2, provider = $3, sort_order = $4, enabled = $5, \
             context_window = $6, updated_at = NOW() WHERE id = $1",
        )
        .bind(id)
        .bind(label)
        .bind(provider)
        .bind(sort_order)
        .bind(enabled)
        .bind(context_window)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Toggle a model's enabled flag without touching its other fields. Works on
    /// builtin rows too (the disable-only path).
    /// Returns `None` when no such model exists, `Some(changed)` otherwise.
    /// `rows_affected` cannot answer "changed": Postgres writes a new tuple
    /// version even when the value is identical. The self-join reads the
    /// pre-update value in the same statement.
    async fn set_enabled_row(
        pool: &PgPool,
        id: &str,
        enabled: bool,
    ) -> Result<Option<bool>, sqlx::Error> {
        sqlx::query_scalar(
            "UPDATE models AS m SET enabled = $2, updated_at = NOW() \
             FROM (SELECT id, enabled FROM models WHERE id = $1) AS prior \
             WHERE m.id = prior.id \
             RETURNING (prior.enabled IS DISTINCT FROM $2)",
        )
        .bind(id)
        .bind(enabled)
        .fetch_optional(pool)
        .await
    }

    /// Delete a model row. **Private on purpose**: [`Self::delete`] is the
    /// reachable mutator, and it emits.
    async fn delete_row(pool: &PgPool, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM models WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Add a user model and announce it. The only way to create one.
    ///
    /// Errors (unique violation) if `id` already exists; the caller maps that to
    /// a 4xx so the user can pick another id. Nothing is announced on that
    /// error, because nothing was written.
    #[allow(clippy::too_many_arguments)] // one arg per model column, plus the bus and actor
    pub async fn create(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        label: &str,
        provider: &str,
        sort_order: i32,
        context_window: Option<i32>,
        actor: Option<MessageOrigin>,
    ) -> Result<Model, sqlx::Error> {
        let model = Self::insert_row(pool, id, label, provider, sort_order, context_window).await?;
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ModelCreated {
                    id: model.id.clone(),
                    label: model.label.clone(),
                    provider: model.provider.clone(),
                    actor,
                }),
                "[Models] ModelCreated",
            )
            .await;
        Ok(model)
    }

    /// Edit a user model and announce it. Announces only when a row existed, so
    /// an edit aimed at a missing id stays silent.
    #[allow(clippy::too_many_arguments)] // one arg per editable column, plus the bus and actor
    pub async fn update(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        label: &str,
        provider: &str,
        sort_order: i32,
        enabled: bool,
        context_window: Option<i32>,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let updated = Self::update_row(
            pool,
            id,
            label,
            provider,
            sort_order,
            enabled,
            context_window,
        )
        .await?;
        if updated {
            Self::announce_update(event_bus, id, actor).await;
        }
        Ok(updated)
    }

    /// Toggle a model's enabled flag and announce it, without touching its other
    /// fields. Works on builtin rows too (the disable-only path).
    ///
    /// Returns whether the model exists (callers report "no model '<id>' in the
    /// registry" on `false`), but announces only when the flag actually MOVED:
    /// `ModelUpdated` makes the in-memory ModelRegistry rebuild, and a retrying
    /// agent re-asserting the current value would rebuild it once per call for
    /// no state change.
    pub async fn set_enabled(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        enabled: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let outcome = Self::set_enabled_row(pool, id, enabled).await?;
        if outcome == Some(true) {
            Self::announce_update(event_bus, id, actor).await;
        }
        Ok(outcome.is_some())
    }

    /// Remove a model and announce it. The caller guards against deleting
    /// builtins. `ModelDeleted` fires only when a row was actually removed.
    pub async fn delete(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let removed = Self::delete_row(pool, id).await?;
        if removed {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::ModelDeleted {
                        id: id.to_string(),
                        actor,
                    }),
                    "[Models] ModelDeleted",
                )
                .await;
        }
        Ok(removed)
    }

    /// Shared by the two edit paths, which differ only in which columns they
    /// touch: the registry reloads wholesale on `ModelUpdated`, so both say the
    /// same thing.
    async fn announce_update(event_bus: &EventBus, id: &str, actor: Option<MessageOrigin>) {
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ModelUpdated {
                    id: id.to_string(),
                    actor,
                }),
                "[Models] ModelUpdated",
            )
            .await;
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
            models
                .iter()
                .any(|m| m.id == "claude-fable-5" && m.provider == "anthropic" && m.is_builtin()),
            "Fable 5 builtin must be seeded on the anthropic provider"
        );
        assert!(
            models
                .iter()
                .any(|m| m.id == "claude-opus-4-8@default" && m.provider == "vertex"),
            "existing Vertex builtins must be seeded"
        );
        assert!(
            models.iter().any(|m| m.id == "claude-opus-5@default"
                && m.provider == "vertex"
                && m.is_builtin()
                && m.enabled),
            "Opus 5 builtin must be seeded on the vertex provider, enabled"
        );
        // Ordered by sort_order — Fable 5 (0) sorts before Opus 5 (5) before
        // Opus 4.8 (10).
        let fable = models
            .iter()
            .position(|m| m.id == "claude-fable-5")
            .unwrap();
        let opus5 = models
            .iter()
            .position(|m| m.id == "claude-opus-5@default")
            .unwrap();
        let opus = models
            .iter()
            .position(|m| m.id == "claude-opus-4-8@default")
            .unwrap();
        assert!(
            fable < opus5 && opus5 < opus,
            "sort_order must drive display order"
        );
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

        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let created = ModelStore::create(
            &pool,
            &bus,
            "my-model",
            "My Model",
            "anthropic",
            99,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(created.source, SOURCE_USER);
        assert!(created.enabled);
        assert!(!created.is_builtin());

        assert!(ModelStore::update(
            &pool, &bus, "my-model", "Renamed", "vertex", 5, false, None, None
        )
        .await
        .unwrap());
        let fetched = ModelStore::get(&pool, "my-model").await.unwrap().unwrap();
        assert_eq!(fetched.label, "Renamed");
        assert_eq!(fetched.provider, "vertex");
        assert!(!fetched.enabled);
        // source is immutable through update
        assert_eq!(fetched.source, SOURCE_USER);

        assert!(ModelStore::delete(&pool, &bus, "my-model", None)
            .await
            .unwrap());
        assert!(ModelStore::get(&pool, "my-model").await.unwrap().is_none());

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The load-bearing guarantee: a registry write and its announcement are one
    /// operation, so the in-memory ModelRegistry reloads no matter which entry
    /// path made the write. An edit or delete aimed at a missing id changes
    /// nothing and therefore announces nothing.
    #[tokio::test]
    async fn every_mutation_announces_and_a_miss_does_not() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        async fn emitted(pool: &PgPool, event_type: &str) -> i64 {
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
                .bind(event_type)
                .fetch_one(pool)
                .await
                .unwrap()
        }

        ModelStore::create(&pool, &bus, "m", "M", "anthropic", 10, None, None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "ModelCreated").await, 1);

        ModelStore::update(&pool, &bus, "m", "M2", "anthropic", 10, true, None, None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "ModelUpdated").await, 1);

        ModelStore::set_enabled(&pool, &bus, "m", false, None)
            .await
            .unwrap();
        assert_eq!(
            emitted(&pool, "ModelUpdated").await,
            2,
            "a toggle is an update the registry must reload on"
        );

        // Re-asserting the current value still reports the model exists, but
        // must not make the registry rebuild for no state change.
        assert!(ModelStore::set_enabled(&pool, &bus, "m", false, None)
            .await
            .unwrap());
        assert_eq!(
            emitted(&pool, "ModelUpdated").await,
            2,
            "a no-op toggle must not announce"
        );

        assert!(
            !ModelStore::set_enabled(&pool, &bus, "missing", false, None)
                .await
                .unwrap()
        );
        assert_eq!(
            emitted(&pool, "ModelUpdated").await,
            2,
            "a toggle that matched no row must not announce"
        );

        assert!(ModelStore::delete(&pool, &bus, "m", None).await.unwrap());
        assert_eq!(emitted(&pool, "ModelDeleted").await, 1);
        assert!(!ModelStore::delete(&pool, &bus, "m", None).await.unwrap());
        assert_eq!(
            emitted(&pool, "ModelDeleted").await,
            1,
            "second delete removes nothing and therefore announces nothing"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A user model can declare its real context window, change it, and clear it
    /// back to the prefix-map fallback. This is the storage half of the kimi-k3
    /// fix: without a declared window the trim budget assumed 200k on a
    /// 1,048,576-token model.
    #[tokio::test]
    async fn context_window_round_trips_and_clears() {
        let (pool, db_name) = setup_test_db().await;

        // Absent on create → NULL (fall back to the prefix map).
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let created = ModelStore::create(
            &pool,
            &bus,
            "ctx-model",
            "Ctx",
            "openrouter",
            99,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(created.context_window, None);

        // Declared on create.
        let declared = ModelStore::create(
            &pool,
            &bus,
            "moonshotai/kimi-k3",
            "Kimi K3",
            "openrouter",
            100,
            Some(1_048_576),
            None,
        )
        .await
        .unwrap();
        assert_eq!(declared.context_window, Some(1_048_576));
        // …and survives a re-read, not just the RETURNING row.
        let reread = ModelStore::get(&pool, "moonshotai/kimi-k3")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reread.context_window, Some(1_048_576));

        // Update sets it.
        assert!(ModelStore::update(
            &pool,
            &bus,
            "ctx-model",
            "Ctx",
            "openrouter",
            99,
            true,
            Some(262_144),
            None
        )
        .await
        .unwrap());
        let fetched = ModelStore::get(&pool, "ctx-model").await.unwrap().unwrap();
        assert_eq!(fetched.context_window, Some(262_144));

        // …and `None` clears it back to the fallback.
        assert!(ModelStore::update(
            &pool,
            &bus,
            "ctx-model",
            "Ctx",
            "openrouter",
            99,
            true,
            None,
            None
        )
        .await
        .unwrap());
        let cleared = ModelStore::get(&pool, "ctx-model").await.unwrap().unwrap();
        assert_eq!(cleared.context_window, None);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Builtins declare the window of the request Lucidos actually makes — which
    /// is not always the model's theoretical maximum.
    ///
    /// The distinction is load-bearing for Claude. Current Claude models
    /// advertise 1M, but Lucidos only requests 1M mode for its own `[1m]` id
    /// suffix (`parse_context_suffix` → `is_1m` → the `context-1m-2025-08-07`
    /// beta in `build_claude_request`). A bare id sends no such beta, so its
    /// real budget is the 200k the prefix map already infers — declaring 1M
    /// there would let the packer build a prompt the API rejects.
    #[tokio::test]
    async fn migration_declares_context_window_on_verified_builtins() {
        let (pool, db_name) = setup_test_db().await;

        let expected: &[(&str, i32)] = &[
            // OpenRouter / Vertex-Gemini — no context opt-in, full window applies.
            ("z-ai/glm-5.2", 1_048_576),
            ("gemini-3.1-pro-preview", 1_048_576),
            ("gemini-3.5-flash", 1_048_576),
            ("gemini-3-flash-preview", 1_048_576),
            // Claude `[1m]` rows — these DO request 1M mode.
            ("claude-fable-5[1m]", 1_000_000),
            ("claude-opus-5@default[1m]", 1_000_000),
            ("claude-opus-4-8@default[1m]", 1_000_000),
            ("claude-opus-4-7[1m]", 1_000_000),
            ("claude-opus-4-6[1m]", 1_000_000),
            ("claude-sonnet-4-6[1m]", 1_000_000),
            // OpenAI — no context opt-in either; the 400k guess understates these.
            ("gpt-5.5-pro", 1_050_000),
            ("gpt-5.5", 1_050_000),
            ("gpt-5.6-sol", 1_050_000),
            ("gpt-5.6-terra", 1_050_000),
            ("gpt-5.6-luna", 1_050_000),
        ];

        for (id, window) in expected {
            let m = ModelStore::get(&pool, id).await.unwrap().unwrap();
            assert_eq!(
                m.context_window,
                Some(*window),
                "{id} must declare its real {window}-token window"
            );
        }

        // Bare Claude ids stay undeclared so they keep tracking the prefix map's
        // 200k — which is correct, because the request carries no 1M beta.
        // Declaring 1M here is the dangerous direction: the packer would exceed
        // the API mode the request actually selected.
        for id in [
            "claude-fable-5",
            "claude-opus-5@default",
            "claude-opus-4-8@default",
            "claude-opus-4-7",
            "claude-sonnet-4-6",
        ] {
            let m = ModelStore::get(&pool, id).await.unwrap().unwrap();
            assert_eq!(
                m.context_window, None,
                "{id} sends no 1M beta — it must stay on the prefix map's 200k"
            );
        }

        // Unverified windows — an over-declared window is worse than the
        // fallback (rejected request vs. trimming early).
        for id in [
            "claude-opus-4-5@20251101",
            "gpt-5.4",
            "gpt-5.3-codex",
            "gpt-5.3-codex-spark",
            "gpt-5.2-codex",
        ] {
            let m = ModelStore::get(&pool, id).await.unwrap().unwrap();
            assert_eq!(
                m.context_window, None,
                "{id} has no verified window — it must fall back to the prefix map"
            );
        }

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn set_enabled_toggles_builtin_without_other_changes() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        assert!(
            ModelStore::set_enabled(&pool, &bus, "claude-fable-5", false, None)
                .await
                .unwrap()
        );
        let m = ModelStore::get(&pool, "claude-fable-5")
            .await
            .unwrap()
            .unwrap();
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
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let result = ModelStore::create(
            &pool,
            &bus,
            "claude-fable-5",
            "Dupe",
            "anthropic",
            1,
            None,
            None,
        )
        .await;
        assert!(result.is_err(), "duplicate id must error");
        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
