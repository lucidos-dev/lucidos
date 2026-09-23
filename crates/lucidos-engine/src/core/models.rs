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

/// One way a model can be served: a backend, the id to send it, and that
/// backend's context window.
///
/// A row carries an ordered list of these, which is what lets one model row be
/// served by whichever provider the workspace has credentials for. Before it,
/// `provider` named THE backend and the same model on two backends needed two
/// rows, so it appeared in the picker twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    /// Backend that serves the model on this route:
    /// "vertex" | "anthropic" | "openai" | "openrouter" | "xai" |
    /// "opencode-free" | "local".
    pub provider: String,
    /// The id sent on the wire. `None` means the row's own id, which is the
    /// common case: a first-party Claude id is byte-identical on Vertex and on
    /// the direct Anthropic API. OpenRouter is the case that needs one, since
    /// it prefixes (`anthropic/claude-opus-5-5`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// This backend's context window in tokens. `None` = not declared, so
    /// `llm::model_registry::context_window_from_prefix` reads THIS route's
    /// wire id. That is what makes an OpenRouter route carrying no `[1m]` land
    /// on 200k without declaring anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i32>,
}

impl Route {
    /// A route on `provider` sending the row's own id, with no declared window.
    pub fn bare(provider: &str) -> Self {
        Self {
            provider: provider.to_string(),
            id: None,
            context_window: None,
        }
    }

    /// The id this route puts on the wire: its own if it declares one, else the
    /// row's. Every id-shape rule reads THIS, never the row id, or a route with
    /// a different spelling would be judged by a string it never sends.
    pub fn wire_id<'a>(&'a self, row_id: &'a str) -> &'a str {
        self.id.as_deref().unwrap_or(row_id)
    }
}

/// A chat model in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    /// Identity and primary key (e.g. "claude-fable-5", "claude-opus-5[1m]").
    /// The default wire id too, which a [`Route`] may override.
    pub id: String,
    pub label: String,
    /// Backends that can serve this model, in priority order. Never empty, and
    /// never naming one provider twice (enforced by `model_routes_valid`).
    pub routes: Vec<Route>,
    /// The provider last picked for this model, or `None` for never picked.
    /// Honoured when its route is configured, and refused rather than
    /// substituted when it is not.
    pub preferred_provider: Option<String>,
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

    /// The row's first route's provider, which is what serves the model when
    /// nothing has been picked. Named on the `Model{Created,Updated}` events so
    /// the audit timeline still says which backend a model landed on.
    pub fn primary_provider(&self) -> &str {
        self.routes.first().map_or("", |r| r.provider.as_str())
    }
}

/// The editable shape of a model row, shared by the create and update paths.
///
/// `enabled` is deliberately absent: create always lands enabled, and update
/// takes it as its own argument. A field nobody reads on one of the two paths
/// is a state that looks settable and is not.
#[derive(Debug, Clone)]
pub struct ModelFields {
    pub label: String,
    pub routes: Vec<Route>,
    pub preferred_provider: Option<String>,
    pub sort_order: i32,
}

/// Check a route list against the rules the DB CHECK also enforces, and say
/// what is wrong in a sentence rather than a constraint-violation string.
///
/// An empty list makes the model unreachable with nothing saying so. A
/// provider named twice makes "the first configured route" ambiguous. A blank
/// id or a non-positive window would send nothing or budget nothing. Provider
/// names parse strictly, through the one list `ProviderKind` holds.
pub fn validate_routes(routes: &[Route]) -> Result<(), String> {
    if routes.is_empty() {
        return Err("A model needs at least one route".to_string());
    }
    let mut seen: Vec<&str> = Vec::with_capacity(routes.len());
    for route in routes {
        let p = route.provider.as_str();
        if crate::llm::ProviderKind::from_name(p).is_none() {
            return Err(crate::llm::model_registry::unknown_provider_message(p));
        }
        if seen.contains(&p) {
            return Err(format!("Provider '{p}' appears twice in routes"));
        }
        if route.id.as_deref().is_some_and(|id| id.trim().is_empty()) {
            return Err(format!(
                "The '{p}' route's id is blank (omit it to send the model's own id)"
            ));
        }
        if route.context_window.is_some_and(|w| w <= 0) {
            return Err(format!(
                "The '{p}' route's context_window must be a positive number of tokens \
                 (omit it to infer from the id)"
            ));
        }
        seen.push(p);
    }
    Ok(())
}

/// Raw DB row: (id, label, routes, preferred_provider, sort_order, source,
/// enabled, created_at).
type ModelRow = (
    String,
    String,
    sqlx::types::Json<Vec<Route>>,
    Option<String>,
    i32,
    String,
    bool,
    chrono::DateTime<chrono::Utc>,
);

const SELECT_COLS: &str =
    "id, label, routes, preferred_provider, sort_order, source, enabled, created_at";

fn row_to_model(row: ModelRow) -> Model {
    let (id, label, routes, preferred_provider, sort_order, source, enabled, created_at) = row;
    Model {
        id,
        label,
        routes: routes.0,
        preferred_provider,
        sort_order,
        source,
        enabled,
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
        fields: &ModelFields,
    ) -> Result<Model, sqlx::Error> {
        let row: ModelRow = sqlx::query_as(&format!(
            "INSERT INTO models \
               (id, label, routes, preferred_provider, sort_order, source, enabled) \
             VALUES ($1, $2, $3, $4, $5, '{SOURCE_USER}', TRUE) \
             RETURNING {SELECT_COLS}"
        ))
        .bind(id)
        .bind(&fields.label)
        .bind(sqlx::types::Json(&fields.routes))
        .bind(&fields.preferred_provider)
        .bind(fields.sort_order)
        .fetch_one(pool)
        .await?;
        Ok(row_to_model(row))
    }

    /// Update the editable fields of a user model (never `source` or `id`).
    /// Every field is written as given, so the caller must resolve "absent from
    /// the request" to the existing value first. A `None` `preferred_provider`
    /// clears the pick and hands the model back to its first configured route.
    /// Returns whether a row existed.
    async fn update_row(
        pool: &PgPool,
        id: &str,
        fields: &ModelFields,
        enabled: bool,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE models SET label = $2, routes = $3, preferred_provider = $4, \
             sort_order = $5, enabled = $6, updated_at = NOW() WHERE id = $1",
        )
        .bind(id)
        .bind(&fields.label)
        .bind(sqlx::types::Json(&fields.routes))
        .bind(&fields.preferred_provider)
        .bind(fields.sort_order)
        .bind(enabled)
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
    pub async fn create(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        fields: &ModelFields,
        actor: Option<MessageOrigin>,
    ) -> Result<Model, sqlx::Error> {
        let model = Self::insert_row(pool, id, fields).await?;
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ModelCreated {
                    id: model.id.clone(),
                    label: model.label.clone(),
                    provider: model.primary_provider().to_string(),
                    actor,
                }),
                "[Models] ModelCreated",
            )
            .await;
        Ok(model)
    }

    /// Edit a user model and announce it. Announces only when a row existed, so
    /// an edit aimed at a missing id stays silent.
    pub async fn update(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        fields: &ModelFields,
        enabled: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let updated = Self::update_row(pool, id, fields, enabled).await?;
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

    /// Whether `provider` can serve this model.
    fn routes_to(m: &Model, provider: &str) -> bool {
        m.routes.iter().any(|r| r.provider == provider)
    }

    /// The window declared on this model's `provider` route, or `None` for an
    /// undeclared one or no such route.
    fn window_on(m: &Model, provider: &str) -> Option<i32> {
        m.routes
            .iter()
            .find(|r| r.provider == provider)
            .and_then(|r| r.context_window)
    }

    /// The window declared on the row's FIRST route, which is where the
    /// migration put the retired `context_window` column.
    fn declared_window(m: &Model) -> Option<i32> {
        m.routes.first().and_then(|r| r.context_window)
    }

    /// A single-route field set, which is what nearly every caller builds.
    fn fields(label: &str, provider: &str, sort_order: i32, window: Option<i32>) -> ModelFields {
        ModelFields {
            label: label.to_string(),
            routes: vec![Route {
                provider: provider.to_string(),
                id: None,
                context_window: window,
            }],
            preferred_provider: None,
            sort_order,
        }
    }

    #[tokio::test]
    async fn migration_seeds_builtins_including_fable_5() {
        let (pool, db_name) = setup_test_db().await;
        let models = ModelStore::list(&pool).await.unwrap();
        assert!(
            models
                .iter()
                .any(|m| m.id == "claude-fable-5" && routes_to(m, "anthropic") && m.is_builtin()),
            "Fable 5 builtin must be seeded on the anthropic provider"
        );
        assert!(
            models.iter().any(|m| m.id == "claude-fable-5-1"
                && routes_to(m, "anthropic")
                && m.is_builtin()
                && m.enabled),
            "Fable 5.1 builtin must be seeded on the anthropic provider, enabled"
        );
        assert!(
            models
                .iter()
                .any(|m| m.id == "claude-opus-4-8" && routes_to(m, "vertex")),
            "existing Vertex builtins must be seeded"
        );
        assert!(
            models.iter().any(|m| m.id == "claude-opus-5-5"
                && routes_to(m, "vertex")
                && m.is_builtin()
                && m.enabled),
            "Opus 5.5 builtin must be seeded on the vertex provider, enabled"
        );
        assert!(
            models.iter().any(|m| m.id == "claude-opus-5"
                && routes_to(m, "vertex")
                && m.is_builtin()
                && m.enabled),
            "Opus 5 builtin must be seeded on the vertex provider, enabled"
        );
        assert!(
            models.iter().any(|m| m.id == "claude-sonnet-5"
                && routes_to(m, "vertex")
                && m.is_builtin()
                && m.enabled),
            "Sonnet 5 builtin must be seeded on the vertex provider, enabled"
        );
        assert!(
            models
                .iter()
                .any(|m| m.id == "claude-sonnet-4-6" && m.is_builtin() && !m.enabled),
            "Sonnet 4.6 is still SEEDED, and switched off by the prior-generation prune"
        );
        // Ordered by sort_order, newest first: Fable 5.1 (-2), Fable 5 (0),
        // Opus 5.5 (2), Opus 5 (5), Sonnet 5 (7), Opus 4.8 (10). That groups
        // the current generation at the top of the picker.
        let fable51 = models
            .iter()
            .position(|m| m.id == "claude-fable-5-1")
            .unwrap();
        let fable = models
            .iter()
            .position(|m| m.id == "claude-fable-5")
            .unwrap();
        let opus55 = models
            .iter()
            .position(|m| m.id == "claude-opus-5-5")
            .unwrap();
        let opus5 = models.iter().position(|m| m.id == "claude-opus-5").unwrap();
        let sonnet5 = models
            .iter()
            .position(|m| m.id == "claude-sonnet-5")
            .unwrap();
        let opus = models
            .iter()
            .position(|m| m.id == "claude-opus-4-8")
            .unwrap();
        assert!(
            fable51 < fable
                && fable < opus55
                && opus55 < opus5
                && opus5 < sonnet5
                && sonnet5 < opus,
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
        assert!(routes_to(&m, "openrouter"));
        assert_eq!(m.label, "GLM 5.2");
        assert!(m.is_builtin(), "GLM 5.2 must be a builtin (disable-only)");
        assert!(m.enabled, "GLM 5.2 builtin is enabled by default");
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The Grok family is seeded on the xAI provider, with every window
    /// DECLARED. The id-shape fallback has no rule for a `grok-` id, so an
    /// undeclared row would budget a 2M model at 200k.
    ///
    /// The window is asserted on all four, including the two the
    /// prior-generation prune switched off. A disabled row still has to carry a
    /// true window: routing loads it, so a saved `chat_model` naming one is
    /// budgeted from this column.
    #[tokio::test]
    async fn migration_seeds_the_grok_family_on_xai_with_declared_windows() {
        let (pool, db_name) = setup_test_db().await;
        for (id, label, window, enabled) in [
            ("grok-4.6", "Grok 4.6", 500_000, true),
            ("grok-4.5", "Grok 4.5", 500_000, false),
            ("grok-4.20", "Grok 4.20", 2_000_000, true),
            ("grok-4.3", "Grok 4.3", 1_000_000, false),
        ] {
            let m = ModelStore::get(&pool, id)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{id} must be seeded"));
            assert!(routes_to(&m, "xai"), "{id}");
            assert_eq!(m.label, label, "{id}");
            assert!(m.is_builtin(), "{id} must be a builtin (disable-only)");
            assert_eq!(m.enabled, enabled, "{id}");
            assert_eq!(declared_window(&m), Some(window), "{id}");
        }

        // The OpenRouter route for Grok is a different id on a different
        // provider. Nothing in this seed may claim it.
        assert!(
            ModelStore::get(&pool, "x-ai/grok-4.6")
                .await
                .unwrap()
                .is_none(),
            "the seed must not create an OpenRouter-prefixed row"
        );
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The keyless free tier is seeded with every window DECLARED, since these
    /// ids share no shape the fallback could read. Seeding does not switch the
    /// tier on: the provider is built from the `opencode_free_enabled`
    /// preference, so the rows stay filtered out until the user opts in.
    #[tokio::test]
    async fn migration_seeds_the_free_tier_with_declared_windows() {
        let (pool, db_name) = setup_test_db().await;
        for (id, label, window) in [
            ("laguna-s-2.1-free", "Laguna S 2.1 (free)", 256_000),
            (
                "nemotron-3.5-lightning-free",
                "Nemotron 3.5 Lightning (free)",
                262_144,
            ),
            ("x-preview-f-free", "Ox Alpha (free)", 1_000_000),
            (
                "nemotron-3-ultra-free",
                "Nemotron 3 Ultra (free)",
                1_000_000,
            ),
            (
                "muse-spark-1.2-contributor-free",
                "Muse Spark 1.2 (free)",
                1_048_576,
            ),
            ("hy3-free", "Hy3 (free)", 190_000),
        ] {
            let m = ModelStore::get(&pool, id)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{id} must be seeded"));
            assert!(routes_to(&m, "opencode-free"), "{id}");
            assert_eq!(m.label, label, "{id}");
            assert!(m.is_builtin(), "{id} must be a builtin (disable-only)");
            assert!(m.enabled, "{id} is enabled by default");
            assert_eq!(declared_window(&m), Some(window), "{id}");
        }

        // The relay serves big-pickle only to the OpenCode CLI's own
        // User-Agent, so a seeded row would never answer us.
        assert!(
            ModelStore::get(&pool, "big-pickle")
                .await
                .unwrap()
                .is_none(),
            "big-pickle is User-Agent gated and must not be seeded"
        );
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The prior generation of every family is switched off, and KEPT.
    ///
    /// Both halves matter. A retired model leaves the picker, and its row has to
    /// survive, because `model_registry::load_from_db` loads disabled rows so a
    /// saved `chat_model` naming one still routes. Delete instead, and a bare
    /// `grok-4.5` falls to the id-shape guess and lands on Vertex.
    #[tokio::test]
    async fn migration_switches_off_the_prior_generation_of_every_family() {
        let (pool, db_name) = setup_test_db().await;
        for id in [
            "claude-opus-4-8",
            "claude-opus-4-8[1m]",
            "claude-opus-4-7",
            "claude-opus-4-7[1m]",
            "claude-sonnet-4-6",
            "claude-sonnet-4-6[1m]",
            "gpt-5.5",
            "gpt-5.4",
            "gpt-5.3-codex",
            "grok-4.5",
            "grok-4.3",
        ] {
            let m = ModelStore::get(&pool, id)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{id} must keep its row, disable-only"));
            assert!(m.is_builtin(), "{id}");
            assert!(!m.enabled, "{id} must be switched off");
        }

        // The current lineup, which the prune must not reach. Opus 5 in
        // particular is `DEFAULT_CHAT_MODEL`: switch it off and a fresh install
        // resolves to a model its own picker will not show.
        for id in [
            "claude-fable-5-1",
            "claude-fable-5",
            "claude-opus-5-5",
            "claude-opus-5",
            "claude-sonnet-5",
            "gpt-6-astra",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5-pro",
            "grok-4.6",
            "grok-4.20",
            "z-ai/glm-5.2",
            "gemini-3.1-pro-preview",
        ] {
            let m = ModelStore::get(&pool, id)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{id} must be seeded"));
            assert!(m.enabled, "{id} must stay on");
        }
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
            &fields("My Model", "anthropic", 99, None),
            None,
        )
        .await
        .unwrap();
        assert_eq!(created.source, SOURCE_USER);
        assert!(created.enabled);
        assert!(!created.is_builtin());

        assert!(ModelStore::update(
            &pool,
            &bus,
            "my-model",
            &fields("Renamed", "vertex", 5, None),
            false,
            None
        )
        .await
        .unwrap());
        let fetched = ModelStore::get(&pool, "my-model").await.unwrap().unwrap();
        assert_eq!(fetched.label, "Renamed");
        assert!(routes_to(&fetched, "vertex"));
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

        ModelStore::create(&pool, &bus, "m", &fields("M", "anthropic", 10, None), None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "ModelCreated").await, 1);

        ModelStore::update(
            &pool,
            &bus,
            "m",
            &fields("M2", "anthropic", 10, None),
            true,
            None,
        )
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
            &fields("Ctx", "openrouter", 99, None),
            None,
        )
        .await
        .unwrap();
        assert_eq!(declared_window(&created), None);

        // Declared on create.
        let declared = ModelStore::create(
            &pool,
            &bus,
            "moonshotai/kimi-k3",
            &fields("Kimi K3", "openrouter", 100, Some(1_048_576)),
            None,
        )
        .await
        .unwrap();
        assert_eq!(declared_window(&declared), Some(1_048_576));
        // …and survives a re-read, not just the RETURNING row.
        let reread = ModelStore::get(&pool, "moonshotai/kimi-k3")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(declared_window(&reread), Some(1_048_576));

        // Update sets it.
        assert!(ModelStore::update(
            &pool,
            &bus,
            "ctx-model",
            &fields("Ctx", "openrouter", 99, Some(262_144)),
            true,
            None
        )
        .await
        .unwrap());
        let fetched = ModelStore::get(&pool, "ctx-model").await.unwrap().unwrap();
        assert_eq!(declared_window(&fetched), Some(262_144));

        // …and `None` clears it back to the fallback.
        assert!(ModelStore::update(
            &pool,
            &bus,
            "ctx-model",
            &fields("Ctx", "openrouter", 99, None),
            true,
            None
        )
        .await
        .unwrap());
        let cleared = ModelStore::get(&pool, "ctx-model").await.unwrap().unwrap();
        assert_eq!(declared_window(&cleared), None);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Builtins declare the window of the request Lucidos actually makes, which
    /// is not always the model's theoretical maximum.
    ///
    /// The distinction is load-bearing for Claude. Lucidos requests 1M mode for
    /// its own `[1m]` id suffix (the 1M-context beta in `build_claude_request`).
    /// A bare id sends no beta, so most bare rows run at the 200k the prefix
    /// map infers. The exception is a family whose DEFAULT window is 1M (Opus
    /// 5, Opus 5.5, Fable 5.x): its bare request is 1M too.
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
            ("claude-fable-5-1[1m]", 1_000_000),
            ("claude-fable-5[1m]", 1_000_000),
            ("claude-opus-5-5[1m]", 1_000_000),
            ("claude-opus-5[1m]", 1_000_000),
            ("claude-opus-4-8[1m]", 1_000_000),
            ("claude-opus-4-7[1m]", 1_000_000),
            ("claude-opus-4-6[1m]", 1_000_000),
            ("claude-sonnet-5[1m]", 1_000_000),
            ("claude-sonnet-4-6[1m]", 1_000_000),
            // Bare rows of the families whose default window is 1M: no beta
            // needed, so the bare request is 1M as well.
            ("claude-fable-5-1", 1_000_000),
            ("claude-fable-5", 1_000_000),
            ("claude-opus-5-5", 1_000_000),
            ("claude-opus-5", 1_000_000),
            // OpenAI — no context opt-in either; the 400k guess understates these.
            ("gpt-5.5-pro", 1_050_000),
            ("gpt-5.5", 1_050_000),
            ("gpt-5.6-sol", 1_050_000),
            ("gpt-5.6-terra", 1_050_000),
            ("gpt-5.6-luna", 1_050_000),
            ("gpt-6-astra", 1_050_000),
        ];

        for (id, window) in expected {
            let m = ModelStore::get(&pool, id).await.unwrap().unwrap();
            assert_eq!(
                declared_window(&m),
                Some(*window),
                "{id} must declare its real {window}-token window"
            );
        }

        // Every other bare Claude id stays undeclared, tracking the prefix
        // map's 200k: the request carries no 1M beta and 1M is not their
        // default. Declaring 1M here is the dangerous direction, because the
        // packer would exceed the API mode the request actually selected.
        for id in [
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-sonnet-5",
            "claude-sonnet-4-6",
        ] {
            let m = ModelStore::get(&pool, id).await.unwrap().unwrap();
            assert_eq!(
                declared_window(&m),
                None,
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
                declared_window(&m),
                None,
                "{id} has no verified window — it must fall back to the prefix map"
            );
        }

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The 1M-default window migration, re-run verbatim from its file. It
    /// declares the window on every route sending the row's own id to Vertex or
    /// Anthropic. It leaves a route with its own id, a window the user set, and
    /// a user-created row sharing a builtin id.
    #[tokio::test]
    async fn the_1m_default_window_migration_declares_routes_and_leaves_user_choices() {
        const MIGRATION: &str = include_str!(
            "../../migrations/20260923075003_declare_1m_window_on_1m_default_claude_rows.sql"
        );
        let (pool, db_name) = setup_test_db().await;

        // The chain already ran this migration, so put each row back to a shape
        // it has to act on (or refuse to).
        sqlx::raw_sql(
            r#"UPDATE models SET routes = '[{"provider": "vertex"}, {"provider": "anthropic"},
                   {"provider": "openrouter", "id": "anthropic/claude-opus-5"}]'
                 WHERE id = 'claude-opus-5';
               UPDATE models SET routes = '[{"provider": "anthropic", "context_window": 300000}]'
                 WHERE id = 'claude-fable-5-1';
               UPDATE models SET routes = '[{"provider": "vertex"}]', source = 'user'
                 WHERE id = 'claude-opus-5-5';"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();

        let routes = |id: &'static str| {
            let pool = pool.clone();
            async move { ModelStore::get(&pool, id).await.unwrap().unwrap().routes }
        };
        let windows: Vec<Option<i32>> = routes("claude-opus-5")
            .await
            .iter()
            .map(|r| r.context_window)
            .collect();
        assert_eq!(windows, vec![Some(1_000_000), Some(1_000_000), None]);
        assert_eq!(
            routes("claude-fable-5-1").await[0].context_window,
            Some(300_000)
        );
        assert_eq!(routes("claude-opus-5-5").await[0].context_window, None);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The GPT-6 Astra seed, as the migration chain leaves it.
    ///
    /// It used to run that migration's SQL verbatim through `include_str!`, to
    /// pin its `ON CONFLICT DO UPDATE`. A workspace could already hold the id as
    /// a hand-added `user` row. DO NOTHING would have left that row deletable,
    /// outside every disable-only builtin protection.
    ///
    /// **That re-run is gone, because the statement can no longer execute.** It
    /// writes `provider` and `context_window`, which the routes migration drops
    /// AFTER it. Nothing can reach that SQL again on any workspace, so a test
    /// re-running it would assert an unreachable path.
    ///
    /// What is still worth pinning is the FOLD. Astra's seed is a later
    /// generation than the original registry, so its row proves a late
    /// `provider` plus `context_window` landed correctly on one route.
    #[tokio::test]
    async fn the_astra_seed_survives_the_fold_into_routes() {
        let (pool, db_name) = setup_test_db().await;
        let m = ModelStore::get(&pool, "gpt-6-astra")
            .await
            .unwrap()
            .expect("the seed must leave a row");
        assert!(m.is_builtin(), "the promotion to builtin must survive");
        assert_eq!(m.label, "GPT-6 Astra");
        assert_eq!(m.sort_order, 36);
        assert!(m.enabled);
        assert_eq!(
            m.routes,
            vec![Route {
                provider: "openai".to_string(),
                id: None,
                context_window: Some(1_050_000),
            }],
            "the retired columns must fold into exactly one route"
        );
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
            &fields("Dupe", "anthropic", 1, None),
            None,
        )
        .await;
        assert!(result.is_err(), "duplicate id must error");
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The whole point of the change: the current-generation Claude rows are
    /// reachable from a workspace holding only an Anthropic key.
    ///
    /// Vertex stays FIRST, so a workspace already on Vertex keeps its backend.
    /// Both routes leave the id to default, because a first-party Claude id is
    /// byte-identical on the two backends.
    #[tokio::test]
    async fn the_current_claude_generation_routes_to_vertex_and_anthropic() {
        let (pool, db_name) = setup_test_db().await;
        for id in [
            "claude-opus-5-5",
            "claude-opus-5-5[1m]",
            "claude-opus-5",
            "claude-opus-5[1m]",
            "claude-sonnet-5",
            "claude-sonnet-5[1m]",
        ] {
            let m = ModelStore::get(&pool, id)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{id} must be seeded"));
            assert_eq!(
                m.routes
                    .iter()
                    .map(|r| r.provider.as_str())
                    .collect::<Vec<_>>(),
                vec!["vertex", "anthropic"],
                "{id} must reach both backends, Vertex first"
            );
            assert!(
                m.routes.iter().all(|r| r.id.is_none()),
                "{id} is spelled the same on both backends"
            );
            assert_eq!(
                m.preferred_provider, None,
                "{id} must ship unpicked, so the first configured route serves"
            );
            // The Anthropic route declares no window on a `[1m]` row or on
            // Sonnet 5: the id-shape guess reads that route's own id, which
            // carries `[1m]` where the row does. The bare Opus 5 and 5.5 rows
            // run 1M by default, so 20260923075003 declares it on both routes.
            let expected = match id {
                "claude-opus-5-5" | "claude-opus-5" => Some(1_000_000),
                _ => None,
            };
            assert_eq!(window_on(&m, "anthropic"), expected, "{id}");
        }

        // Fable is not published on Vertex, so its rows keep one route.
        for id in ["claude-fable-5-1", "claude-fable-5"] {
            let m = ModelStore::get(&pool, id).await.unwrap().unwrap();
            assert_eq!(
                m.routes
                    .iter()
                    .map(|r| r.provider.as_str())
                    .collect::<Vec<_>>(),
                vec!["anthropic"],
                "{id} is Anthropic-only"
            );
        }

        // The retired rows are an explicit non-goal: their direct-API ids were
        // never probed, and seeding one unverified trades a clean "not
        // configured" refusal for a vendor 404.
        for id in ["claude-opus-4-8", "claude-opus-4-7", "claude-sonnet-4-6"] {
            let m = ModelStore::get(&pool, id).await.unwrap().unwrap();
            assert!(
                !routes_to(&m, "anthropic"),
                "{id} is retired and must keep its single Vertex route"
            );
        }
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A row cannot be stored with no route, nor with one provider twice. The
    /// first makes the model unreachable with nothing saying so; the second
    /// makes "the first configured route" ambiguous.
    #[tokio::test]
    async fn the_route_list_cannot_be_empty_or_name_a_provider_twice() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        let empty = ModelFields {
            label: "Empty".to_string(),
            routes: Vec::new(),
            preferred_provider: None,
            sort_order: 1,
        };
        assert!(
            ModelStore::create(&pool, &bus, "no-routes", &empty, None)
                .await
                .is_err(),
            "the CHECK must refuse an empty route list"
        );

        let doubled = ModelFields {
            routes: vec![Route::bare("anthropic"), Route::bare("anthropic")],
            ..empty.clone()
        };
        assert!(
            ModelStore::create(&pool, &bus, "doubled", &doubled, None)
                .await
                .is_err(),
            "the CHECK must refuse a duplicated provider"
        );

        // And the same judgment at the API boundary, which is what turns those
        // into a sentence rather than a constraint-violation string.
        let refused = |routes: &[Route], says: &str| {
            let err = validate_routes(routes).expect_err("refused");
            assert!(err.contains(says), "{err}");
        };
        refused(&[], "at least one route");
        refused(&doubled.routes, "appears twice");
        refused(&[Route::bare("nope")], "Unknown provider 'nope'");
        assert_eq!(validate_routes(&[Route::bare("anthropic")]), Ok(()));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The `@default` re-spell reaches every store that reads a model id back
    /// as a live setting, and moves nothing when its rename was skipped.
    ///
    /// The migration already ran on this database, so the test seeds legacy
    /// spellings and runs the same file again. It is written to be re-runnable.
    #[tokio::test]
    async fn the_respell_leaves_no_saved_model_reference_orphaned() {
        const RESPELL: &str =
            include_str!("../../migrations/20260922222019_respell_default_alias_model_ids.sql");
        let (pool, db_name) = setup_test_db().await;
        let thread = uuid::Uuid::new_v4();
        let seed = [
            // Opus 4.8 renames cleanly. Opus 5 collides with the bare row the
            // first run produced, so its references must stay where they are.
            "DELETE FROM models WHERE id = 'claude-opus-4-8'".to_string(),
            "INSERT INTO models (id, label, routes, sort_order, source, enabled) VALUES \
             ('claude-opus-4-8@default', 'Opus 4.8', '[{\"provider\": \"vertex\"}]', 1, \
              'builtin', false), \
             ('claude-opus-5@default', 'Opus 5 (legacy)', '[{\"provider\": \"vertex\"}]', 1, \
              'user', true)"
                .to_string(),
            "DELETE FROM preferences WHERE key IN ('chat_model', 'model_memory', 'model_title')"
                .to_string(),
            "INSERT INTO preferences (key, value) VALUES \
             ('chat_model', 'claude-opus-4-8@default'), \
             ('model_memory', 'claude-opus-4-8@default'), \
             ('model_title', 'claude-opus-5@default')"
                .to_string(),
            format!(
                "INSERT INTO thread_summaries (thread_id, compose_selection) VALUES \
                 ('{thread}', '{{\"model\": \"claude-opus-4-8@default\"}}')"
            ),
            format!(
                "INSERT INTO thread_queue (id, kind, summary, request) VALUES \
                 ('{thread}', 'cron', 's', \
                  '{{\"type\": \"cron\", \"model\": \"claude-opus-4-8@default\"}}')"
            ),
            format!(
                "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload) VALUES \
                 (gen_random_uuid(), 'thread', '{thread}', 'MessageReceived', \
                  '{{\"text\": \"hi\", \"model\": \"claude-opus-4-8@default\"}}'), \
                 (gen_random_uuid(), 'trigger', 't1', 'TriggerCreated', \
                  '{{\"id\": \"t1\", \"model\": \"claude-opus-4-8@default\"}}')"
            ),
        ];
        for statement in seed {
            sqlx::query(&statement)
                .execute(&pool)
                .await
                .expect(&statement);
        }

        sqlx::raw_sql(RESPELL)
            .execute(&pool)
            .await
            .expect("the re-spell re-runs");

        let text = |sql: String| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, String>(&sql)
                    .fetch_one(&pool)
                    .await
                    .expect(&sql)
            }
        };
        assert!(ModelStore::get(&pool, "claude-opus-4-8")
            .await
            .unwrap()
            .is_some());
        for key in ["chat_model", "model_memory"] {
            assert_eq!(
                text(format!("SELECT value FROM preferences WHERE key = '{key}'")).await,
                "claude-opus-4-8"
            );
        }
        assert_eq!(
            text(format!(
                "SELECT compose_selection->>'model' FROM thread_summaries \
                 WHERE thread_id = '{thread}'"
            ))
            .await,
            "claude-opus-4-8"
        );
        assert_eq!(
            text(format!(
                "SELECT request->>'model' FROM thread_queue WHERE id = '{thread}'"
            ))
            .await,
            "claude-opus-4-8"
        );
        for event in ["MessageReceived", "TriggerCreated"] {
            assert_eq!(
                text(format!(
                    "SELECT payload->>'model' FROM events WHERE event_type = '{event}' \
                     AND payload->>'model' LIKE 'claude-opus-4-8%'"
                ))
                .await,
                "claude-opus-4-8"
            );
        }

        // The collision: the legacy row stays, and so does what names it. It
        // keeps its Vertex-only route too, since the direct API rejects its id.
        let legacy = ModelStore::get(&pool, "claude-opus-5@default")
            .await
            .unwrap()
            .expect("the legacy row stays");
        assert_eq!(legacy.routes, vec![Route::bare("vertex")]);
        assert_eq!(
            text("SELECT value FROM preferences WHERE key = 'model_title'".to_string()).await,
            "claude-opus-5@default"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// One route the engine cannot decode fails the WHOLE registry read, so the
    /// CHECK refuses every malformed shape a hand edit could write. A string
    /// window must be refused too, rather than raise inside the cast.
    #[tokio::test]
    async fn a_malformed_route_cannot_reach_the_table() {
        let (pool, db_name) = setup_test_db().await;
        for bad in [
            r#"[{"provider": "vertex", "id": 5}]"#,
            r#"[{"provider": "vertex", "id": "  "}]"#,
            r#"[{"provider": "vertex", "context_window": "big"}]"#,
            r#"[{"provider": "vertex", "context_window": 0}]"#,
            r#"[{"provider": "vertex", "context_window": 1.5}]"#,
            r#"[{"provider": "vertex", "context_window": 1048576.0}]"#,
            r#"[{"provider": "vertex", "context_window": 99999999999}]"#,
            r#"[{"id": "no-provider"}]"#,
            r#"["vertex"]"#,
        ] {
            let inserted = sqlx::query(
                "INSERT INTO models (id, label, routes, sort_order, source, enabled) \
                 VALUES ('hand-edited', 'Hand edited', $1::jsonb, 1, 'user', true)",
            )
            .bind(bad)
            .execute(&pool)
            .await;
            assert!(inserted.is_err(), "the CHECK must refuse {bad}");
        }
        sqlx::query(
            "INSERT INTO models (id, label, routes, sort_order, source, enabled) \
             VALUES ('hand-edited', 'Hand edited', \
                     '[{\"provider\": \"vertex\", \"id\": \"x\", \"context_window\": 1000, \
                       \"extra\": null}]'::jsonb, 1, 'user', true)",
        )
        .execute(&pool)
        .await
        .expect("a well-formed route is stored");
        assert!(
            ModelStore::list(&pool).await.is_ok(),
            "the registry still reads"
        );

        let blank_id = Route {
            id: Some(" ".to_string()),
            ..Route::bare("vertex")
        };
        assert!(validate_routes(&[blank_id])
            .unwrap_err()
            .contains("id is blank"));
        let no_window = Route {
            context_window: Some(0),
            ..Route::bare("vertex")
        };
        assert!(validate_routes(&[no_window])
            .unwrap_err()
            .contains("positive number"));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The picker's write path. `preferred_provider` round-trips on a BUILTIN,
    /// which is the case that matters: refusing it there would pin every seeded
    /// model to whichever backend the migration listed first.
    #[tokio::test]
    async fn a_builtin_remembers_its_preferred_provider() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let existing = ModelStore::get(&pool, "claude-opus-5-5")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(existing.preferred_provider, None);

        let picked = ModelFields {
            label: existing.label.clone(),
            routes: existing.routes.clone(),
            preferred_provider: Some("anthropic".to_string()),
            sort_order: existing.sort_order,
        };
        assert!(
            ModelStore::update(&pool, &bus, &existing.id, &picked, true, None)
                .await
                .unwrap()
        );
        let after = ModelStore::get(&pool, "claude-opus-5-5")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.preferred_provider.as_deref(), Some("anthropic"));
        assert!(after.is_builtin(), "the pick must not change source");
        assert_eq!(after.label, existing.label);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
