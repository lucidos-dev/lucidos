use serde::Serialize;

use crate::engine::aux_purpose::{recommended, select_model, ModelSource, Reach};
use crate::engine::ContextPurpose;
use crate::llm::model_prices::{price_of, ModelPrice};
use crate::llm::model_registry::{
    default_effort, resolve_route, ModelRegistry, ProviderKind, Unconfigured,
};
use crate::llm::reasoning::clamp_effort;
use crate::llm::ServiceTier;

pub(crate) struct CompactorModel {
    pub(crate) id: &'static str,
    pub(crate) seeds: &'static [TierSeed],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TierSeed {
    pub(crate) effort: &'static str,
    pub(crate) usage: MeasuredUsage,
    pub(crate) secs_per_call: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MeasuredUsage {
    pub(crate) avg_input: u64,
    pub(crate) avg_cache_read: u64,
    pub(crate) avg_cache_write: u64,
    pub(crate) avg_output: u64,
}

pub(crate) const SEED_ROUNDS_PER_NODE: f64 = 1.08;

const fn cached(
    effort: &'static str,
    input: u64,
    cache_read: u64,
    cache_write: u64,
    output: u64,
    secs: f64,
) -> TierSeed {
    TierSeed {
        effort,
        usage: MeasuredUsage {
            avg_input: input,
            avg_cache_read: cache_read,
            avg_cache_write: cache_write,
            avg_output: output,
        },
        secs_per_call: Some(secs),
    }
}

const fn uncached(effort: &'static str, input: u64, output: u64, secs: Option<f64>) -> TierSeed {
    TierSeed {
        effort,
        usage: MeasuredUsage {
            avg_input: input,
            avg_cache_read: 0,
            avg_cache_write: 0,
            avg_output: output,
        },
        secs_per_call: secs,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Tokens {
    pub(crate) input: f64,
    pub(crate) cache_read: f64,
    pub(crate) cache_write: f64,
    pub(crate) output: f64,
}

impl CompactorModel {
    pub(crate) fn price(&self) -> ModelPrice {
        price_of(self.id).unwrap_or_else(|| panic!("{} has no price card", self.id))
    }

    pub(crate) fn usd(&self, tokens: Tokens, tier: Option<ServiceTier>) -> f64 {
        let price = self.price();
        let plain = (tokens.input - tokens.cache_read - tokens.cache_write).max(0.0);
        let input = plain
            + tokens.cache_read * price.cache.read
            + tokens.cache_write * price.cache.write_5m;
        let share = match tier {
            Some(ServiceTier::Flex) => price.flex.unwrap_or(1.0),
            None => 1.0,
        };
        share * (input * price.input + tokens.output * price.output) / 1_000_000.0
    }
}

pub(crate) fn priced(model: &str) -> Option<&'static CompactorModel> {
    COMPACTOR_MODELS.iter().find(|m| m.id == model)
}

pub(crate) fn serves_flex(provider: ProviderKind, model: &str) -> bool {
    provider == ProviderKind::OpenAi && priced(model).is_some_and(|m| m.price().flex.is_some())
}

pub(crate) const COMPACTOR_MODELS: &[CompactorModel] = &[
    CompactorModel {
        id: "gpt-6.1-sol",
        seeds: &[cached("low", 7_315, 4_905, 2_407, 116, 3.7)],
    },
    CompactorModel {
        id: "gemini-3.8-flash",
        seeds: &[
            uncached("low", 3_229, 93, Some(2.6)),
            uncached("medium", 3_229, 495, Some(8.0)),
        ],
    },
    CompactorModel {
        id: "claude-sonnet-5-5",
        seeds: &[
            uncached("low", 4_383, 181, Some(2.2)),
            uncached("medium", 4_422, 182, None),
        ],
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompactorDefault {
    pub model: &'static str,
    pub effort: &'static str,
}

pub const COMPACTOR_DEFAULTS: &[CompactorDefault] = &[
    CompactorDefault {
        model: "gpt-6.1-sol",
        effort: "low",
    },
    CompactorDefault {
        model: "gemini-3.8-flash",
        effort: "low",
    },
    CompactorDefault {
        model: "claude-sonnet-5-5",
        effort: "low",
    },
];

pub(crate) fn default_effort_for(model: &str) -> Option<&'static str> {
    COMPACTOR_DEFAULTS
        .iter()
        .find(|entry| entry.model == model)
        .map(|entry| entry.effort)
}

fn unset_effort(model: &str, registry: &ModelRegistry) -> Option<&'static str> {
    default_effort_for(model).or_else(|| default_effort(registry, model))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CompactorSelection {
    pub model: String,
    pub effort: Option<String>,
    pub source: ModelSource,
}

fn select(
    stored_model: Option<String>,
    stored_effort: Option<String>,
    chat_model: &str,
    registry: &ModelRegistry,
    reach: &Reach,
) -> CompactorSelection {
    let (model, source) = select_model(
        stored_model,
        &recommended(ContextPurpose::SummaryCompaction, registry),
        chat_model,
        registry,
        reach,
    );
    let effort = stored_effort.or_else(|| unset_effort(&model, registry).map(str::to_string));
    CompactorSelection {
        model,
        effort,
        source,
    }
}

pub(crate) async fn compactor_selection(
    pool: &sqlx::PgPool,
    registry: &ModelRegistry,
    reach: &Reach,
    chat_model: &str,
) -> CompactorSelection {
    let pair = crate::engine::aux_purpose::SUMMARY_COMPACTION_PREFS;
    select(
        pair.model.read(pool).await,
        pair.reasoning.read(pool).await,
        chat_model,
        registry,
        reach,
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CompactorRoute {
    pub(crate) lane: String,
    /// INVARIANT: The tier the route runs, snapped as routing snaps it. The capture
    /// records this, so the estimate finds history under the tier that ran.
    pub(crate) effort: Option<String>,
    pub(crate) flex: bool,
}

pub(crate) fn compactor_route(
    model: &str,
    effort: Option<&str>,
    registry: &ModelRegistry,
    configured: &[ProviderKind],
) -> Result<CompactorRoute, String> {
    let route = resolve_route(registry, model, None, |kind| configured.contains(&kind)).map_err(
        |Unconfigured(kind)| {
            format!(
                "no configured provider serves {model}; configure {} or pick another model",
                kind.as_str()
            )
        },
    )?;
    Ok(CompactorRoute {
        lane: format!("{}/{}", route.provider.as_str(), route.wire_id),
        effort: effort
            .and_then(|e| clamp_effort(e, route.provider, &route.wire_id))
            .map(str::to_string),
        flex: serves_flex(route.provider, model),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::prefs;
    use crate::llm::model_registry::{load_from_db, ModelRouting};
    use crate::test_support::{setup_test_db, teardown_test_db};
    use std::sync::{Arc, RwLock};

    #[test]
    fn every_id_is_unique() {
        let mut ids: Vec<&str> = COMPACTOR_MODELS.iter().map(|m| m.id).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), before, "a duplicate id in COMPACTOR_MODELS");
    }

    #[test]
    fn every_compactor_model_is_priced() {
        let table = crate::llm::model_prices::shipped_table();
        for model in COMPACTOR_MODELS {
            let cards = table.models.get(model.id);
            assert!(
                cards.is_some_and(|c| c.iter().any(|card| card.card.from.is_none())),
                "{} has no undated card",
                model.id
            );
        }
    }

    fn compactor_ids() -> Vec<&'static str> {
        COMPACTOR_MODELS.iter().map(|m| m.id).collect()
    }

    #[test]
    fn the_compactor_catalog_holds_one_model_per_provider_family() {
        assert_eq!(
            compactor_ids(),
            ["gpt-6.1-sol", "gemini-3.8-flash", "claude-sonnet-5-5"]
        );
    }

    #[test]
    fn every_default_is_a_compactor_model() {
        for entry in COMPACTOR_DEFAULTS {
            assert!(
                compactor_ids().contains(&entry.model),
                "{} is a default but not a compactor model",
                entry.model
            );
        }
    }

    #[test]
    fn every_default_tier_is_seeded_and_seeds_climb_the_ladder() {
        for entry in COMPACTOR_DEFAULTS {
            let model = COMPACTOR_MODELS.iter().find(|m| m.id == entry.model);
            assert!(
                model.is_some_and(|m| m.seeds.iter().any(|s| s.effort == entry.effort)),
                "{} has no {} seed",
                entry.model,
                entry.effort
            );
        }
        let effort_step =
            |effort: &str| crate::llm::EFFORT_LADDER.iter().position(|t| *t == effort);
        for model in COMPACTOR_MODELS {
            let effort_steps: Vec<_> = model.seeds.iter().map(|s| effort_step(s.effort)).collect();
            assert!(
                effort_steps.iter().all(Option::is_some),
                "{}: {effort_steps:?}",
                model.id
            );
            assert!(
                effort_steps.windows(2).all(|w| w[0] < w[1]),
                "{}: {effort_steps:?}",
                model.id
            );
        }
    }

    fn selected(
        stored_model: Option<&str>,
        stored_effort: Option<&str>,
        kinds: &[ProviderKind],
    ) -> CompactorSelection {
        select(
            stored_model.map(str::to_string),
            stored_effort.map(str::to_string),
            "my-chat-model",
            &crate::llm::model_registry::empty(),
            &Reach::configured(kinds),
        )
    }

    #[test]
    fn a_stored_model_and_tier_win_over_the_default() {
        let s = selected(Some("gemini-3.8-flash"), Some("high"), &ProviderKind::ALL);
        assert_eq!(s.model, "gemini-3.8-flash");
        assert_eq!(s.effort.as_deref(), Some("high"));
        assert_eq!(s.source, ModelSource::Preference);
    }

    #[test]
    fn a_model_picked_without_a_tier_runs_at_its_measured_or_default_tier() {
        let registry = crate::llm::model_registry::empty();
        let mut opus = ModelRouting::single(ProviderKind::Vertex, "claude-opus-5-5");
        opus.default_effort = Some("medium");
        let mut sol = ModelRouting::single(ProviderKind::OpenAi, "gpt-6.1-sol");
        sol.default_effort = Some("medium");
        registry.write().unwrap().extend([
            ("claude-opus-5-5".to_string(), opus),
            ("gpt-6.1-sol".to_string(), sol),
        ]);
        let effort = |model: &str| {
            select(
                Some(model.to_string()),
                None,
                "my-chat-model",
                &registry,
                &Reach::configured(&[]),
            )
            .effort
        };
        assert_eq!(effort("gpt-6.1-sol").as_deref(), Some("low"), "measured");
        assert_eq!(effort("claude-opus-5-5").as_deref(), Some("medium"));
        assert_eq!(effort("my-local-model"), None);
    }

    #[test]
    fn a_stored_tier_applies_to_the_default_model() {
        let s = selected(None, Some("medium"), &[ProviderKind::OpenAi]);
        assert_eq!(
            (s.model.as_str(), s.effort.as_deref()),
            ("gpt-6.1-sol", Some("medium"))
        );
        assert_eq!(s.source, ModelSource::Default);
    }

    #[test]
    fn with_no_default_reachable_the_chat_model_compacts() {
        let s = selected(None, None, &[ProviderKind::Local]);
        assert_eq!(s.model, "my-chat-model");
        assert_eq!(s.effort, None, "the chat model has no row here");
        assert_eq!(s.source, ModelSource::ChatModel);
    }

    #[test]
    fn an_unreachable_model_names_what_to_configure() {
        let registry = crate::llm::model_registry::empty();
        let reason = compactor_route("gpt-6.1-sol", None, &registry, &[ProviderKind::Vertex])
            .expect_err("OpenAI is not configured");
        assert!(
            reason.contains("gpt-6.1-sol") && reason.contains("openai"),
            "{reason}"
        );
        assert_eq!(
            compactor_route("gpt-6.1-sol", None, &registry, &[ProviderKind::OpenAi])
                .map(|r| r.lane),
            Ok("openai/gpt-6.1-sol".to_string())
        );
    }

    #[test]
    fn the_route_carries_the_tier_it_runs() {
        let registry = crate::llm::model_registry::empty();
        let effort = |model: &str, stored: &str, kind: ProviderKind| {
            compactor_route(model, Some(stored), &registry, &[kind])
                .expect("served")
                .effort
        };
        assert_eq!(
            effort("gpt-6.1-sol", "none", ProviderKind::OpenAi).as_deref(),
            clamp_effort("none", ProviderKind::OpenAi, "gpt-6.1-sol")
        );
        assert_eq!(
            effort("gpt-6.1-sol", "medium", ProviderKind::OpenAi).as_deref(),
            Some("medium")
        );
        assert_ne!(
            effort("gpt-6.1-sol", "none", ProviderKind::OpenAi).as_deref(),
            Some("none"),
            "Sol has no none tier"
        );
    }

    const PROSE_LABELS: &[(&str, &str)] = &[
        ("gpt-6.1-sol", "GPT-6.1 Sol"),
        ("gemini-3.8-flash", "Gemini 3.8 Flash"),
        ("claude-sonnet-5-5", "Sonnet 5.5"),
    ];

    #[tokio::test]
    async fn the_selection_reads_the_stored_keys() {
        let (pool, db_name) = setup_test_db().await;
        let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
        let vertex = Reach::configured(&[ProviderKind::Vertex]);
        let unset = compactor_selection(&pool, &registry, &vertex, "my-chat-model").await;
        assert_eq!(unset.model, "gemini-3.8-flash");
        assert_eq!(unset.source, ModelSource::Default);

        crate::core::PreferenceStore::set_row_for_test(
            &pool,
            prefs::MODEL_MEMORY.key(),
            "gpt-5.4-mini",
        )
        .await
        .unwrap();
        let memory_only = compactor_selection(&pool, &registry, &vertex, "my-chat-model").await;
        assert_eq!(memory_only.model, "gemini-3.8-flash", "no inheritance");

        for (key, value) in [
            (prefs::MODEL_SUMMARY_COMPACTION.key(), "claude-sonnet-5-5"),
            (prefs::REASONING_SUMMARY_COMPACTION.key(), "high"),
        ] {
            crate::core::PreferenceStore::set_row_for_test(&pool, key, value)
                .await
                .unwrap();
        }
        let stored = compactor_selection(&pool, &registry, &vertex, "my-chat-model").await;
        assert_eq!(stored.model, "claude-sonnet-5-5");
        assert_eq!(stored.effort.as_deref(), Some("high"));
        assert_eq!(stored.source, ModelSource::Preference);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    fn only(kinds: &[ProviderKind]) -> impl Fn(ProviderKind) -> bool + '_ {
        move |kind| kinds.contains(&kind)
    }

    fn resolved(
        registry: &ModelRegistry,
        kinds: &[ProviderKind],
    ) -> Option<(&'static str, &'static str, ProviderKind)> {
        let (model, source) = select_model(
            None,
            &recommended(ContextPurpose::SummaryCompaction, registry),
            "",
            registry,
            &Reach::configured(kinds),
        );
        if source != ModelSource::Default {
            return None;
        }
        let entry = COMPACTOR_DEFAULTS.iter().find(|e| e.model == model)?;
        let route = resolve_route(registry, entry.model, None, only(kinds)).ok()?;
        Some((entry.model, entry.effort, route.provider))
    }

    #[tokio::test]
    async fn the_default_follows_the_configured_providers() {
        let (pool, db_name) = setup_test_db().await;
        let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
        use ProviderKind::*;
        let all = ProviderKind::ALL;
        assert_eq!(
            resolved(&registry, &all),
            Some(("gpt-6.1-sol", "low", ChatGptPlan)),
            "every provider configured: a signed-in plan leads the GPT row"
        );
        assert_eq!(
            resolved(&registry, &[Anthropic]),
            Some(("claude-sonnet-5-5", "low", Anthropic)),
            "only Anthropic"
        );
        assert_eq!(
            resolved(&registry, &[OpenAi]),
            Some(("gpt-6.1-sol", "low", OpenAi)),
            "only OpenAI"
        );
        assert_eq!(
            resolved(&registry, &[OpenRouter]),
            Some(("gpt-6.1-sol", "low", OpenRouter)),
            "only OpenRouter"
        );
        assert_eq!(resolved(&registry, &[]), None, "none");
        assert_eq!(
            resolved(&registry, &[Vertex]),
            Some(("gemini-3.8-flash", "low", Vertex)),
            "only Vertex"
        );
        assert_eq!(
            resolved(&registry, &[Vertex, Anthropic]),
            Some(("gemini-3.8-flash", "low", Vertex)),
            "Vertex and Anthropic"
        );
        assert_eq!(resolved(&registry, &[Local]), None, "only local");
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn every_default_tier_survives_every_route() {
        let (pool, db_name) = setup_test_db().await;
        let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
        for entry in COMPACTOR_DEFAULTS {
            for kind in ProviderKind::ALL {
                let Ok(route) = resolve_route(&registry, entry.model, Some(kind), |k| k == kind)
                else {
                    continue;
                };
                if route.provider != kind {
                    continue;
                }
                assert_eq!(
                    clamp_effort(entry.effort, kind, &route.wire_id),
                    Some(entry.effort),
                    "{} at {} snaps on {:?}",
                    entry.model,
                    entry.effort,
                    kind
                );
            }
        }
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    mod guard_tests {
        use super::*;

        #[test]
        fn every_prose_copy_names_the_defaults() {
            let unset = |spec: &prefs::PrefSpec| match spec.default {
                prefs::PrefDefault::Unset(text) => text,
                _ => panic!("{} must be unset by default", spec.key),
            };
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let doc = |path: &str| crate::core::knowhow::doc_family_text(&repo, path);
            let model_copies = [
                (
                    "the model description",
                    prefs::MODEL_SUMMARY_COMPACTION.spec.description.to_string(),
                ),
                (
                    "the model's unset text",
                    unset(&prefs::MODEL_SUMMARY_COMPACTION.spec).to_string(),
                ),
                ("preferences.md", doc("system-knowhow/preferences.md")),
                ("the glossary", doc("docs/glossary.md")),
            ];
            for entry in COMPACTOR_DEFAULTS {
                let label = PROSE_LABELS
                    .iter()
                    .find(|(id, _)| *id == entry.model)
                    .map_or(entry.model, |(_, label)| *label);
                for (name, text) in &model_copies {
                    assert!(
                        text.contains(entry.model) || text.contains(label),
                        "{name} omits {}",
                        entry.model
                    );
                }
                assert!(
                    unset(&prefs::REASONING_SUMMARY_COMPACTION.spec).contains(entry.effort),
                    "the tier's unset text omits {}",
                    entry.effort
                );
            }
        }
    }
}
