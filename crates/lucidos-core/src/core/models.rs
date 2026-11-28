use sqlx::PgPool;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

pub use super::model_types::{
    validate_routes, Model, ModelFields, Route, SOURCE_BUILTIN, SOURCE_USER,
};

type ModelRow = (
    String,
    String,
    sqlx::types::Json<Vec<Route>>,
    Option<String>,
    bool,
    Option<String>,
    Option<String>,
    i32,
    String,
    chrono::DateTime<chrono::Utc>,
);

const SELECT_COLS: &str = "id, label, routes, preferred_provider, vision, default_effort, \
     successor, sort_order, source, created_at";

fn row_to_model(row: ModelRow) -> Model {
    let (
        id,
        label,
        routes,
        preferred_provider,
        vision,
        default_effort,
        successor,
        sort_order,
        source,
        created_at,
    ) = row;
    Model {
        id,
        label,
        routes: routes.0,
        preferred_provider,
        vision,
        default_effort,
        successor,
        sort_order,
        source,
        created_at,
    }
}

/// INVARIANT: The chat model registry.
///
/// **No caller can skip the event.** [`Self::create`], [`Self::update`] and
/// [`Self::delete`] are the only reachable mutators;
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
    /// INVARIANT: All models, ordered for display (sort_order, then label). The registry
    /// needs every row, so a saved pick off the chat ladder still routes.
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

    async fn insert_row(
        pool: &PgPool,
        id: &str,
        fields: &ModelFields,
    ) -> Result<Model, sqlx::Error> {
        let row: ModelRow = sqlx::query_as(&format!(
            "INSERT INTO models \
               (id, label, routes, preferred_provider, vision, sort_order, source) \
             VALUES ($1, $2, $3, $4, $5, $6, '{SOURCE_USER}') \
             RETURNING {SELECT_COLS}"
        ))
        .bind(id)
        .bind(&fields.label)
        .bind(sqlx::types::Json(&fields.routes))
        .bind(&fields.preferred_provider)
        .bind(fields.vision)
        .bind(fields.sort_order)
        .fetch_one(pool)
        .await?;
        Ok(row_to_model(row))
    }

    /// INVARIANT: Update the editable fields of a user model (never `source` or `id`).
    /// Every field is written as given, so the caller must resolve "absent from
    /// the request" to the existing value first. A `None` `preferred_provider`
    /// clears the pick and hands the model back to its first configured route.
    /// Returns whether a row existed.
    async fn update_row(
        pool: &PgPool,
        id: &str,
        fields: &ModelFields,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE models SET label = $2, routes = $3, preferred_provider = $4, \
             vision = $5, sort_order = $6, updated_at = NOW() WHERE id = $1",
        )
        .bind(id)
        .bind(&fields.label)
        .bind(sqlx::types::Json(&fields.routes))
        .bind(&fields.preferred_provider)
        .bind(fields.vision)
        .bind(fields.sort_order)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_row(pool: &PgPool, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM models WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

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

    pub async fn update(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        fields: &ModelFields,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let updated = Self::update_row(pool, id, fields).await?;
        if updated {
            Self::announce_update(event_bus, id, actor).await;
        }
        Ok(updated)
    }

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
    use crate::core::prefs;
    use crate::test_support::{setup_test_db, teardown_test_db};

    fn routes_to(m: &Model, provider: &str) -> bool {
        m.routes.iter().any(|r| r.provider == provider)
    }

    fn window_on(m: &Model, provider: &str) -> Option<i32> {
        m.routes
            .iter()
            .find(|r| r.provider == provider)
            .and_then(|r| r.context_window)
    }

    fn declared_window(m: &Model) -> Option<i32> {
        m.routes.first().and_then(|r| r.context_window)
    }

    fn fields(label: &str, provider: &str, sort_order: i32, window: Option<i32>) -> ModelFields {
        ModelFields {
            label: label.to_string(),
            routes: vec![Route {
                provider: provider.to_string(),
                id: None,
                context_window: window,
            }],
            preferred_provider: None,
            vision: false,
            sort_order,
        }
    }

    #[tokio::test]
    async fn november_models_are_additive_native_window_builtins() {
        let (pool, db_name) = setup_test_db().await;
        for (id, window, effort, providers) in [
            (
                "claude-fable-5-2",
                1_000_000,
                "high",
                vec!["anthropic", "claude-gateway"],
            ),
            ("gemini-4-argon", 1_048_576, "medium", vec!["vertex"]),
            ("gemini-4-carbon", 1_048_576, "medium", vec!["vertex"]),
            ("gemini-4-neon", 1_048_576, "high", vec!["vertex"]),
        ] {
            let m = ModelStore::get(&pool, id).await.unwrap().expect(id);
            assert!(m.is_builtin() && m.vision && m.successor.is_none());
            assert_eq!(m.default_effort.as_deref(), Some(effort));
            assert_eq!(
                m.routes
                    .iter()
                    .map(|r| r.provider.as_str())
                    .collect::<Vec<_>>(),
                providers
            );
            assert!(m
                .routes
                .iter()
                .all(|r| r.context_window == Some(window) && r.id.is_none()));
            assert!(ModelStore::get(&pool, &format!("{id}[1m]"))
                .await
                .unwrap()
                .is_none());
        }
        for (id, migration) in [
            (
                "gemini-4-argon",
                include_str!(
                    "../../migrations/20261110213512_add_fable_5_2_and_gemini_4_models.sql"
                ),
            ),
            (
                "gemini-4-neon",
                include_str!("../../migrations/20261119060505_add_gemini_4_neon.sql"),
            ),
        ] {
            sqlx::query("DELETE FROM models WHERE id = $1")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
            let custom = fields("My endpoint", "local", 77, Some(64_000));
            let before = ModelStore::insert_row(&pool, id, &custom).await.unwrap();
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
            let after = ModelStore::get(&pool, id).await.unwrap().unwrap();
            assert_eq!(
                serde_json::to_value(before).unwrap(),
                serde_json::to_value(after).unwrap()
            );
        }
        let mut alias = fields("Custom Gemini", "vertex", 78, Some(1_048_576));
        alias.routes[0].id = Some("gemini-4-carbon".to_string());
        ModelStore::insert_row(&pool, "custom-gemini", &alias)
            .await
            .unwrap();
        let registry = crate::llm::model_registry::empty();
        crate::llm::model_registry::reload(&registry, &pool).await;
        let route =
            crate::llm::model_registry::resolve_route(&registry, "custom-gemini", None, |_| true)
                .unwrap();
        let defaults = crate::llm::vertex::sdk_request_defaults(&route.wire_id, Some("medium"));
        let cap = defaults["body"]["generationConfig"]["maxOutputTokens"]
            .as_u64()
            .unwrap() as usize;
        let window = route.context_window.unwrap();
        assert_eq!(
            crate::llm::context_tokens::agent_context_char_budget_for(&route.wire_id, window),
            (window - cap) * 3 / 2
        );
        pool.close().await;
        teardown_test_db(&db_name).await;
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
            models
                .iter()
                .any(|m| m.id == "claude-fable-5-1" && routes_to(m, "anthropic") && m.is_builtin()),
            "Fable 5.1 builtin must be seeded on the anthropic provider"
        );
        assert!(
            models
                .iter()
                .any(|m| m.id == "claude-opus-4-8" && routes_to(m, "vertex")),
            "existing Vertex builtins must be seeded"
        );
        assert!(
            models
                .iter()
                .any(|m| m.id == "claude-opus-5-5" && routes_to(m, "vertex") && m.is_builtin()),
            "Opus 5.5 builtin must be seeded on the vertex provider"
        );
        for id in ["claude-sonnet-5-5", "claude-sonnet-5-5[1m]"] {
            assert!(
                models.iter().any(|m| m.id == id
                    && routes_to(m, "vertex")
                    && routes_to(m, "anthropic")
                    && m.is_builtin()),
                "{id} builtin must be seeded on vertex and anthropic"
            );
        }
        let haiku55 = models
            .iter()
            .find(|m| m.id == "claude-haiku-5-5")
            .expect("Haiku 5.5 builtin must be seeded");
        assert!(haiku55.is_builtin() && haiku55.vision);
        assert_eq!(
            haiku55
                .routes
                .iter()
                .map(|r| (r.provider.as_str(), r.id.as_deref(), r.context_window))
                .collect::<Vec<_>>(),
            [
                ("vertex", None, Some(1_000_000)),
                ("anthropic", None, Some(1_000_000)),
                ("claude-gateway", None, Some(1_000_000)),
                (
                    "openrouter",
                    Some("anthropic/claude-haiku-5.5"),
                    Some(1_000_000)
                ),
            ],
            "first party first, so a configured one never leaves for OpenRouter"
        );
        for (id, successor) in [
            ("claude-opus-5", "claude-opus-5-5"),
            ("claude-sonnet-5", "claude-sonnet-5-5"),
        ] {
            assert!(
                models.iter().any(|m| m.id == id
                    && routes_to(m, "vertex")
                    && m.is_builtin()
                    && m.successor.as_deref() == Some(successor)),
                "{id} keeps its vertex row, retired to {successor}"
            );
        }
        assert!(
            models
                .iter()
                .any(|m| m.id == "claude-sonnet-4-6" && m.is_builtin()),
            "Sonnet 4.6 is still SEEDED"
        );
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
        let sonnet55 = models
            .iter()
            .position(|m| m.id == "claude-sonnet-5-5")
            .unwrap();
        let sonnet55_1m = models
            .iter()
            .position(|m| m.id == "claude-sonnet-5-5[1m]")
            .unwrap();
        let opus5 = models.iter().position(|m| m.id == "claude-opus-5").unwrap();
        let sonnet5 = models
            .iter()
            .position(|m| m.id == "claude-sonnet-5")
            .unwrap();
        let haiku55 = models
            .iter()
            .position(|m| m.id == "claude-haiku-5-5")
            .unwrap();
        let opus = models
            .iter()
            .position(|m| m.id == "claude-opus-4-8")
            .unwrap();
        assert!(
            fable51 < fable
                && fable < opus55
                && opus55 < sonnet55
                && sonnet55 < sonnet55_1m
                && sonnet55_1m < opus5
                && opus5 < sonnet5
                && sonnet5 < haiku55
                && haiku55 < opus,
            "sort_order must drive display order"
        );
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn migration_routes_every_anthropic_builtin_to_the_claude_gateway() {
        let (pool, db_name) = setup_test_db().await;
        let models = ModelStore::list(&pool).await.unwrap();
        let mut routed = 0;
        for m in models.iter().filter(|m| m.is_builtin()) {
            let at = |provider: &str| m.routes.iter().position(|r| r.provider == provider);
            match (at("anthropic"), at("claude-gateway")) {
                (Some(anthropic), Some(gateway)) => {
                    assert_eq!(gateway, anthropic + 1, "{}: right after anthropic", m.id);
                    let (a, g) = (&m.routes[anthropic], &m.routes[gateway]);
                    assert_eq!(
                        (&a.id, a.context_window),
                        (&g.id, g.context_window),
                        "{}",
                        m.id
                    );
                    routed += 1;
                }
                (None, None) => {}
                (anthropic, gateway) => {
                    panic!("{}: anthropic {anthropic:?}, gateway {gateway:?}", m.id)
                }
            }
        }
        assert!(
            routed > 0,
            "the current Claude builtins route to the gateway"
        );
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn migration_routes_every_gpt_builtin_to_the_chatgpt_plan_first() {
        let (pool, db_name) = setup_test_db().await;
        let models = ModelStore::list(&pool).await.unwrap();
        let mut routed = 0;
        for m in models.iter().filter(|m| m.is_builtin()) {
            let at = |provider: &str| m.routes.iter().position(|r| r.provider == provider);
            match (at("openai"), at("chatgpt-plan")) {
                (Some(openai), Some(plan)) => {
                    assert!(m.id.starts_with("gpt-"), "{}", m.id);
                    assert_eq!(plan + 1, openai, "{}: right before openai", m.id);
                    let (o, p) = (&m.routes[openai], &m.routes[plan]);
                    assert_eq!(
                        (&o.id, o.context_window),
                        (&p.id, p.context_window),
                        "{}",
                        m.id
                    );
                    routed += 1;
                }
                (None, None) => {}
                (Some(_), None) if !m.id.starts_with("gpt-") => {}
                (openai, plan) => panic!("{}: openai {openai:?}, plan {plan:?}", m.id),
            }
        }
        assert!(routed > 0, "the current GPT builtins route to the plan");
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn migration_seeds_the_newest_small_models_with_an_openrouter_route() {
        let (pool, db_name) = setup_test_db().await;
        for (id, first_party, openrouter_id, window, effort) in [
            (
                "gpt-6-luna",
                "openai",
                "openai/gpt-6-luna",
                1_050_000,
                "medium",
            ),
            (
                "gemini-3.1-flash-lite",
                "vertex",
                "google/gemini-3.1-flash-lite",
                1_048_576,
                "low",
            ),
        ] {
            let m = ModelStore::get(&pool, id)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{id} must be seeded"));
            assert!(m.is_builtin() && m.vision, "{id}");
            assert_eq!(m.successor, None, "{id}");
            assert_eq!(m.default_effort.as_deref(), Some(effort), "{id}");
            let plan = (first_party == "openai").then_some(("chatgpt-plan", None, Some(window)));
            let expected: Vec<_> = plan
                .into_iter()
                .chain([
                    (first_party, None, Some(window)),
                    ("openrouter", Some(openrouter_id), Some(window)),
                ])
                .collect();
            assert_eq!(
                m.routes
                    .iter()
                    .map(|r| (r.provider.as_str(), r.id.as_deref(), r.context_window))
                    .collect::<Vec<_>>(),
                expected,
                "{id}"
            );
        }
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
        assert!(m.is_builtin(), "GLM 5.2 must be a builtin");
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn migration_seeds_the_grok_family_on_xai_with_declared_windows() {
        let (pool, db_name) = setup_test_db().await;
        for (id, label, window) in [
            ("grok-4.6", "Grok 4.6", 500_000),
            ("grok-4.5", "Grok 4.5", 500_000),
            ("grok-4.20", "Grok 4.20", 2_000_000),
            ("grok-4.3", "Grok 4.3", 1_000_000),
        ] {
            let m = ModelStore::get(&pool, id)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{id} must be seeded"));
            assert!(routes_to(&m, "xai"), "{id}");
            assert_eq!(m.label, label, "{id}");
            assert!(m.is_builtin(), "{id} must be a builtin");
            assert_eq!(declared_window(&m), Some(window), "{id}");
        }

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
            assert!(m.is_builtin(), "{id} must be a builtin");
            assert_eq!(declared_window(&m), Some(window), "{id}");
        }

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

    #[tokio::test]
    async fn migration_keeps_the_prior_generation_of_every_family() {
        let (pool, db_name) = setup_test_db().await;
        for id in [
            "claude-opus-4-8",
            "claude-opus-4-8[1m]",
            "claude-opus-4-7",
            "claude-opus-4-7[1m]",
            "claude-sonnet-4-6",
            "claude-sonnet-4-6[1m]",
            "claude-opus-5",
            "claude-sonnet-5",
            "claude-fable-5",
            "claude-haiku-4-5",
            "gemini-3-flash-preview",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5-pro",
            "gpt-5.5",
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5.3-codex",
            "grok-4.5",
            "grok-4.3",
        ] {
            let m = ModelStore::get(&pool, id)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{id} must keep its row"));
            assert!(m.is_builtin(), "{id}");
        }

        for id in [
            "claude-fable-5-1",
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
            "gemini-3.8-flash",
            "gemini-3.1-flash-lite",
            "gpt-6-astra",
            "gpt-6.2-astra",
            "gpt-6.2-sol",
            "gpt-6.1-sol",
            "gpt-6-luna",
            "grok-4.6",
            "grok-4.20",
            "z-ai/glm-5.2",
            "gemini-3.1-pro-preview",
        ] {
            let m = ModelStore::get(&pool, id)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{id} must be seeded"));
            assert_eq!(m.successor, None, "{id} must stay current");
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
        assert!(!created.is_builtin());

        assert!(ModelStore::update(
            &pool,
            &bus,
            "my-model",
            &fields("Renamed", "vertex", 5, None),
            None
        )
        .await
        .unwrap());
        let fetched = ModelStore::get(&pool, "my-model").await.unwrap().unwrap();
        assert_eq!(fetched.label, "Renamed");
        assert!(routes_to(&fetched, "vertex"));
        assert_eq!(fetched.source, SOURCE_USER);

        assert!(ModelStore::delete(&pool, &bus, "my-model", None)
            .await
            .unwrap());
        assert!(ModelStore::get(&pool, "my-model").await.unwrap().is_none());

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

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

        ModelStore::update(&pool, &bus, "m", &fields("M2", "anthropic", 10, None), None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "ModelUpdated").await, 1);

        assert!(!ModelStore::update(
            &pool,
            &bus,
            "missing",
            &fields("M2", "anthropic", 10, None),
            None,
        )
        .await
        .unwrap());
        assert_eq!(
            emitted(&pool, "ModelUpdated").await,
            1,
            "an edit that matched no row must not announce"
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

    #[tokio::test]
    async fn context_window_round_trips_and_clears() {
        let (pool, db_name) = setup_test_db().await;

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
        let reread = ModelStore::get(&pool, "moonshotai/kimi-k3")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(declared_window(&reread), Some(1_048_576));

        assert!(ModelStore::update(
            &pool,
            &bus,
            "ctx-model",
            &fields("Ctx", "openrouter", 99, Some(262_144)),
            None
        )
        .await
        .unwrap());
        let fetched = ModelStore::get(&pool, "ctx-model").await.unwrap().unwrap();
        assert_eq!(declared_window(&fetched), Some(262_144));

        assert!(ModelStore::update(
            &pool,
            &bus,
            "ctx-model",
            &fields("Ctx", "openrouter", 99, None),
            None
        )
        .await
        .unwrap());
        let cleared = ModelStore::get(&pool, "ctx-model").await.unwrap().unwrap();
        assert_eq!(declared_window(&cleared), None);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    const BUILTINS_NOT_KNOWN_TO_READ_IMAGES: &[&str] = &[
        "gpt-5.3-codex-spark",
        "z-ai/glm-5.2",
        "grok-4.6",
        "grok-4.5",
        "grok-4.20",
        "grok-4.3",
        "laguna-s-2.1-free",
        "nemotron-3.5-lightning-free",
        "x-preview-f-free",
        "nemotron-3-ultra-free",
        "muse-spark-1.2-contributor-free",
        "hy3-free",
    ];

    fn in_an_image_reading_family(id: &str) -> bool {
        (id.starts_with("claude-") || id.starts_with("gemini-") || id.starts_with("gpt-"))
            && !BUILTINS_NOT_KNOWN_TO_READ_IMAGES.contains(&id)
    }

    #[tokio::test]
    async fn every_builtin_declares_whether_it_reads_images() {
        let (pool, db_name) = setup_test_db().await;
        let builtins: Vec<Model> = ModelStore::list(&pool)
            .await
            .unwrap()
            .into_iter()
            .filter(Model::is_builtin)
            .collect();
        assert!(!builtins.is_empty());
        for m in &builtins {
            assert_eq!(
                m.vision,
                in_an_image_reading_family(&m.id),
                "builtin '{}' has vision = {}; seed it, or list it in \
                 BUILTINS_NOT_KNOWN_TO_READ_IMAGES",
                m.id,
                m.vision
            );
        }
        for id in BUILTINS_NOT_KNOWN_TO_READ_IMAGES {
            assert!(
                builtins.iter().any(|m| m.id == *id),
                "'{id}' is listed but is no builtin row"
            );
        }
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn the_vision_seed_reruns_cleanly_and_leaves_user_rows_alone() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        ModelStore::create(
            &pool,
            &bus,
            "claude-my-own",
            &fields("Mine", "anthropic", 1000, None),
            None,
        )
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/20261006204014_add_vision_flag_to_models.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let mine = ModelStore::get(&pool, "claude-my-own")
            .await
            .unwrap()
            .unwrap();
        assert!(!mine.vision, "a user row is never seeded");
        let haiku = ModelStore::get(&pool, "claude-haiku-4-5")
            .await
            .unwrap()
            .unwrap();
        assert!(haiku.vision);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    const BUILTINS_WITH_NO_DOCUMENTED_EFFORT: &[&str] = &[
        "gpt-6-astra",
        "gpt-6.2-astra",
        "gpt-6.2-sol",
        "gpt-5.2-codex",
        "gpt-5.3-codex-spark",
        "grok-4.20",
        "z-ai/glm-5.2",
        "laguna-s-2.1-free",
        "nemotron-3.5-lightning-free",
        "x-preview-f-free",
        "nemotron-3-ultra-free",
        "muse-spark-1.2-contributor-free",
        "hy3-free",
    ];

    #[tokio::test]
    async fn every_builtin_declares_its_default_effort() {
        let (pool, db_name) = setup_test_db().await;
        let builtins: Vec<Model> = ModelStore::list(&pool)
            .await
            .unwrap()
            .into_iter()
            .filter(Model::is_builtin)
            .collect();
        for m in &builtins {
            let undocumented = BUILTINS_WITH_NO_DOCUMENTED_EFFORT.contains(&m.id.as_str());
            let Some(effort) = m.default_effort.as_deref() else {
                assert!(
                    undocumented,
                    "builtin '{}' has no default effort; seed one, or list it in \
                     BUILTINS_WITH_NO_DOCUMENTED_EFFORT",
                    m.id
                );
                continue;
            };
            assert!(!undocumented, "'{}' is listed but seeds {effort}", m.id);
            let route = &m.routes[0];
            let provider = crate::llm::ProviderKind::parse(&route.provider);
            assert_eq!(
                crate::llm::reasoning::clamp_effort(effort, provider, route.wire_id(&m.id)),
                Some(effort),
                "'{}' at {effort} snaps on its first route",
                m.id
            );
        }
        for id in BUILTINS_WITH_NO_DOCUMENTED_EFFORT {
            assert!(
                builtins.iter().any(|m| m.id == *id),
                "'{id}' is listed but is no builtin row"
            );
        }
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn the_default_effort_seed_reruns_cleanly_and_leaves_user_rows_alone() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        ModelStore::create(
            &pool,
            &bus,
            "claude-my-own",
            &fields("Mine", "anthropic", 1000, None),
            None,
        )
        .await
        .unwrap();
        let before = ModelStore::list(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/20261008115918_add_default_effort_to_models.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let after = ModelStore::list(&pool).await.unwrap();
        let efforts = |rows: &[Model]| -> Vec<(String, Option<String>)> {
            rows.iter()
                .map(|m| (m.id.clone(), m.default_effort.clone()))
                .collect()
        };
        assert_eq!(efforts(&before), efforts(&after));
        let mine = after.iter().find(|m| m.id == "claude-my-own").unwrap();
        assert_eq!(mine.default_effort, None);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn the_default_effort_check_allows_exactly_the_ladder() {
        let (pool, db_name) = setup_test_db().await;
        let check: String = sqlx::query_scalar(
            "SELECT pg_get_constraintdef(oid) FROM pg_constraint \
             WHERE conrelid = 'models'::regclass AND contype = 'c' \
               AND pg_get_constraintdef(oid) LIKE '%default_effort%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let allowed: Vec<&str> = check
            .split('\'')
            .skip(1)
            .step_by(2)
            .filter(|word| !word.is_empty())
            .collect();
        assert_eq!(allowed, crate::llm::EFFORT_LADDER, "{check}");
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn the_default_effort_reaches_the_registry() {
        let (pool, db_name) = setup_test_db().await;
        let registry: crate::llm::ModelRegistry = std::sync::Arc::new(std::sync::RwLock::new(
            crate::llm::model_registry::load_from_db(&pool).await,
        ));
        use crate::llm::model_registry::default_effort;
        assert_eq!(default_effort(&registry, "claude-opus-5-5"), Some("medium"));
        assert_eq!(default_effort(&registry, "gpt-6-astra"), None);
        assert_eq!(default_effort(&registry, "no-such-model"), None);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn the_vision_flag_round_trips_into_the_registry() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let looking = ModelFields {
            vision: true,
            ..fields("Looker", "local", 1000, None)
        };
        let created = ModelStore::create(&pool, &bus, "local/looker", &looking, None)
            .await
            .unwrap();
        assert!(created.vision);
        let registry = crate::llm::model_registry::load_from_db(&pool).await;
        assert!(registry["local/looker"].vision);

        let blind = ModelFields {
            vision: false,
            ..looking
        };
        assert!(
            ModelStore::update(&pool, &bus, "local/looker", &blind, None)
                .await
                .unwrap()
        );
        let reread = ModelStore::get(&pool, "local/looker")
            .await
            .unwrap()
            .unwrap();
        assert!(!reread.vision);
        let registry = crate::llm::model_registry::load_from_db(&pool).await;
        assert!(!registry["local/looker"].vision);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn migration_declares_context_window_on_verified_builtins() {
        let (pool, db_name) = setup_test_db().await;

        let expected: &[(&str, i32)] = &[
            ("z-ai/glm-5.2", 1_048_576),
            ("gemini-3.1-pro-preview", 1_048_576),
            ("gemini-3.5-flash", 1_048_576),
            ("gemini-3-flash-preview", 1_048_576),
            ("claude-fable-5-1[1m]", 1_000_000),
            ("claude-fable-5[1m]", 1_000_000),
            ("claude-opus-5-5[1m]", 1_000_000),
            ("claude-sonnet-5-5[1m]", 1_000_000),
            ("claude-opus-5[1m]", 1_000_000),
            ("claude-opus-4-8[1m]", 1_000_000),
            ("claude-opus-4-7[1m]", 1_000_000),
            ("claude-opus-4-6[1m]", 1_000_000),
            ("claude-sonnet-5[1m]", 1_000_000),
            ("claude-sonnet-4-6[1m]", 1_000_000),
            ("claude-fable-5-1", 1_000_000),
            ("claude-fable-5", 1_000_000),
            ("claude-opus-5-5", 1_000_000),
            ("claude-sonnet-5-5", 1_000_000),
            ("claude-opus-5", 1_000_000),
            ("claude-haiku-5-5", 1_000_000),
            ("gpt-5.5-pro", 1_050_000),
            ("gpt-5.5", 1_050_000),
            ("gpt-5.6-sol", 1_050_000),
            ("gpt-5.6-terra", 1_050_000),
            ("gpt-5.6-luna", 1_050_000),
            ("gpt-6-astra", 1_050_000),
            ("gpt-6.2-astra", 1_050_000),
            ("gpt-6.2-sol", 1_050_000),
            ("gpt-6-luna", 1_050_000),
            ("gemini-3.1-flash-lite", 1_048_576),
        ];

        for (id, window) in expected {
            let m = ModelStore::get(&pool, id).await.unwrap().unwrap();
            assert_eq!(
                declared_window(&m),
                Some(*window),
                "{id} must declare its real {window}-token window"
            );
        }

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

    #[tokio::test]
    async fn the_1m_default_window_migration_declares_routes_and_leaves_user_choices() {
        const MIGRATION: &str = include_str!(
            "../../migrations/20260923075003_declare_1m_window_on_1m_default_claude_rows.sql"
        );
        let (pool, db_name) = setup_test_db().await;

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
        let folded = Route {
            provider: "openai".to_string(),
            id: None,
            context_window: Some(1_050_000),
        };
        let plan = Route {
            provider: "chatgpt-plan".to_string(),
            ..folded.clone()
        };
        assert_eq!(
            m.routes,
            vec![plan, folded],
            "the retired columns must fold into exactly one route"
        );
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn gpt_6_2_seed_preserves_routes_and_existing_rows() {
        let (pool, db_name) = setup_test_db().await;
        let seed = include_str!("../../migrations/20261117204507_seed_gpt_6_2_models.sql");
        for id in ["gpt-6.2-astra", "gpt-6.2-sol"] {
            let model = ModelStore::get(&pool, id).await.unwrap().unwrap();
            assert!(model.is_builtin());
            assert!(model.vision);
            assert_eq!(model.default_effort, None);
            let providers: Vec<_> = model.routes.iter().map(|r| r.provider.as_str()).collect();
            let expected = if id.ends_with("sol") {
                vec!["chatgpt-plan", "openai", "openrouter"]
            } else {
                vec!["chatgpt-plan", "openai"]
            };
            assert_eq!(providers, expected);
            for route in &model.routes {
                assert_eq!(route.context_window, Some(1_050_000));
            }
            if id.ends_with("sol") {
                assert_eq!(model.routes[2].wire_id(id), "openai/gpt-6.2-sol");
            }
        }
        sqlx::raw_sql(seed).execute(&pool).await.unwrap();
        sqlx::query("UPDATE models SET source = 'user', label = 'Custom' WHERE id = 'gpt-6.2-sol'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(seed).execute(&pool).await.unwrap();
        let custom = ModelStore::get(&pool, "gpt-6.2-sol")
            .await
            .unwrap()
            .unwrap();
        assert!(!custom.is_builtin());
        assert_eq!(custom.label, "Custom");
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn create_duplicate_id_errors() {
        let (pool, db_name) = setup_test_db().await;
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

    #[tokio::test]
    async fn the_current_claude_generation_routes_to_vertex_anthropic_and_the_gateway() {
        let (pool, db_name) = setup_test_db().await;
        for id in [
            "claude-opus-5-5",
            "claude-opus-5-5[1m]",
            "claude-opus-5",
            "claude-opus-5[1m]",
            "claude-sonnet-5",
            "claude-sonnet-5[1m]",
            "claude-sonnet-5-5",
            "claude-sonnet-5-5[1m]",
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
                vec!["vertex", "anthropic", "claude-gateway"],
                "{id} must reach every backend, Vertex first"
            );
            assert!(
                m.routes.iter().all(|r| r.id.is_none()),
                "{id} is spelled the same on every backend"
            );
            assert_eq!(
                m.preferred_provider, None,
                "{id} must ship unpicked, so the first configured route serves"
            );
            let expected = match id {
                "claude-opus-5-5"
                | "claude-opus-5"
                | "claude-sonnet-5-5"
                | "claude-sonnet-5-5[1m]" => Some(1_000_000),
                _ => None,
            };
            assert_eq!(window_on(&m, "anthropic"), expected, "{id}");
            assert_eq!(window_on(&m, "claude-gateway"), expected, "{id}");
        }

        for id in ["claude-fable-5-1", "claude-fable-5"] {
            let m = ModelStore::get(&pool, id).await.unwrap().unwrap();
            assert_eq!(
                m.routes
                    .iter()
                    .map(|r| r.provider.as_str())
                    .collect::<Vec<_>>(),
                vec!["anthropic", "claude-gateway"],
                "{id} is not on Vertex"
            );
        }

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

    #[tokio::test]
    async fn the_route_list_cannot_be_empty_or_name_a_provider_twice() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        let empty = ModelFields {
            label: "Empty".to_string(),
            routes: Vec::new(),
            preferred_provider: None,
            vision: false,
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

    #[tokio::test]
    async fn the_respell_leaves_no_saved_model_reference_orphaned() {
        const RESPELL: &str =
            include_str!("../../migrations/20260922222019_respell_default_alias_model_ids.sql");
        let (pool, db_name) = setup_test_db().await;
        let thread = uuid::Uuid::new_v4();
        let (chat, memory, title) = (
            prefs::CHAT_MODEL.key(),
            prefs::MODEL_MEMORY.key(),
            prefs::MODEL_TITLE.key(),
        );
        let seed = [
            "DELETE FROM models WHERE id = 'claude-opus-4-8'".to_string(),
            "INSERT INTO models (id, label, routes, sort_order, source) VALUES \
             ('claude-opus-4-8@default', 'Opus 4.8', '[{\"provider\": \"vertex\"}]', 1, \
              'builtin'), \
             ('claude-opus-5@default', 'Opus 5 (legacy)', '[{\"provider\": \"vertex\"}]', 1, \
              'user')"
                .to_string(),
            format!("DELETE FROM preferences WHERE key IN ('{chat}', '{memory}', '{title}')"),
            format!(
                "INSERT INTO preferences (key, value) VALUES \
                 ('{chat}', 'claude-opus-4-8@default'), \
                 ('{memory}', 'claude-opus-4-8@default'), \
                 ('{title}', 'claude-opus-5@default')"
            ),
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
        for key in [chat, memory] {
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

        let legacy = ModelStore::get(&pool, "claude-opus-5@default")
            .await
            .unwrap()
            .expect("the legacy row stays");
        assert_eq!(legacy.routes, vec![Route::bare("vertex")]);
        assert_eq!(
            text(format!(
                "SELECT value FROM preferences WHERE key = '{title}'"
            ))
            .await,
            "claude-opus-5@default"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

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
                "INSERT INTO models (id, label, routes, sort_order, source) \
                 VALUES ('hand-edited', 'Hand edited', $1::jsonb, 1, 'user')",
            )
            .bind(bad)
            .execute(&pool)
            .await;
            assert!(inserted.is_err(), "the CHECK must refuse {bad}");
        }
        sqlx::query(
            "INSERT INTO models (id, label, routes, sort_order, source) \
             VALUES ('hand-edited', 'Hand edited', \
                     '[{\"provider\": \"vertex\", \"id\": \"x\", \"context_window\": 1000, \
                       \"extra\": null}]'::jsonb, 1, 'user')",
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
            vision: existing.vision,
            sort_order: existing.sort_order,
        };
        assert!(ModelStore::update(&pool, &bus, &existing.id, &picked, None)
            .await
            .unwrap());
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

#[cfg(test)]
#[path = "retired_models_tests.rs"]
mod retired_models_tests;
