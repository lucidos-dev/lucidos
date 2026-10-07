//! The compactor's model catalog: one source for its price table, its
//! per-model token seeds and its default. A second copy of any of them falls
//! silently out of sync; see
//! `docs/plans/2026-10-06-tree-compactor-provider-aware-default.md`.
//!
//! [`super::estimate`] reads [`COMPACTOR_MODELS`] for its price table, and
//! Settings recommends [`COMPACTOR_DEFAULTS`] first. Every default is a priced
//! row, so the default always has an estimate.

use serde::Serialize;

use crate::engine::aux_purpose::{recommended, select_model, ModelSource};
use crate::engine::ContextPurpose;
use crate::llm::model_registry::{resolve_route, ModelRegistry, ProviderKind, Unconfigured};

/// One measured compactor model.
pub(crate) struct CompactorModel {
    pub(crate) id: &'static str,
    /// List price in USD per million (input, output) tokens.
    pub(crate) price: (f64, f64),
    /// Tokens one compactor call spends, measured in the comparison the plan
    /// above records. The estimate prices a model with too little history of
    /// its own on these.
    pub(crate) seed: MeasuredUsage,
}

/// Average tokens of one compactor call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MeasuredUsage {
    pub(crate) avg_input: u64,
    /// The average at the `low` tier: the baseline the estimate scales other
    /// tiers from.
    pub(crate) avg_output_low: u64,
}

pub(crate) const COMPACTOR_MODELS: &[CompactorModel] = &[
    CompactorModel {
        id: "gpt-6.1-sol",
        // https://developers.openai.com/api/docs/models/gpt-6.1-sol
        price: (2.0, 10.0),
        seed: MeasuredUsage {
            avg_input: 2_909,
            avg_output_low: 109,
        },
    },
    CompactorModel {
        id: "gemini-3.8-flash",
        // Introductory: it doubles to (1.5, 7.5) on 1 January 2027
        // (docs/temporary-measures.md, "Gemini 3.8 Flash's introductory
        // compactor price").
        price: (0.75, 3.75),
        seed: MeasuredUsage {
            avg_input: 3_229,
            avg_output_low: 93,
        },
    },
    CompactorModel {
        id: "claude-sonnet-5-5",
        // https://platform.claude.com/docs/en/about-claude/pricing, "Model
        // pricing" table, "Claude Sonnet 5.5" row.
        price: (2.0, 10.0),
        seed: MeasuredUsage {
            avg_input: 4_383,
            avg_output_low: 181,
        },
    },
];

/// One entry of the compactor's default: a model and the tier it runs at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CompactorDefault {
    pub(crate) model: &'static str,
    pub(crate) effort: &'static str,
}

/// The compactor's default, best first. The first model a configured provider
/// serves wins. The order and the tiers come from the comparison in the plan
/// above: best summaries first, within reason on cost.
///
/// The conversation summary leads its auxiliary default with these models too
/// (ADR 0377), so an edit here moves both summary writers.
pub(crate) const COMPACTOR_DEFAULTS: &[CompactorDefault] = &[
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

/// The tier a default entry runs `model` at, for a model picked with no tier.
fn default_effort_for(model: &str) -> Option<&'static str> {
    COMPACTOR_DEFAULTS
        .iter()
        .find(|entry| entry.model == model)
        .map(|entry| entry.effort)
}

/// The tier for a model no default entry names, picked with no tier.
pub(crate) const FALLBACK_EFFORT: &str = "low";

/// The model and tier the compactor runs on, and where the model came from.
/// `GET /api/v1/models/background` serves it, through `AuxCall`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CompactorSelection {
    pub model: String,
    pub effort: String,
    pub source: ModelSource,
}

/// The selection, from what is stored. Pure, so each source is tested alone.
fn select(
    stored_model: Option<String>,
    stored_effort: Option<String>,
    chat_model: &str,
    registry: &ModelRegistry,
    is_configured: impl Fn(ProviderKind) -> bool,
) -> CompactorSelection {
    let (model, source) = select_model(
        stored_model,
        &recommended(ContextPurpose::SummaryCompaction, registry),
        chat_model,
        registry,
        is_configured,
    );
    let effort = stored_effort.unwrap_or_else(|| {
        default_effort_for(&model)
            .unwrap_or(FALLBACK_EFFORT)
            .to_string()
    });
    CompactorSelection {
        model,
        effort,
        source,
    }
}

/// The compactor's selection on this workspace, against the providers the
/// router has configured. `chat_model` is the model turns run on, the last
/// resort.
pub(crate) async fn compactor_selection(
    pool: &sqlx::PgPool,
    registry: &ModelRegistry,
    configured: &[ProviderKind],
    chat_model: &str,
) -> CompactorSelection {
    let pair = crate::engine::aux_purpose::SUMMARY_COMPACTION_PREFS;
    select(
        pair.model.read(pool).await,
        pair.reasoning.read(pool).await,
        chat_model,
        registry,
        |kind| configured.contains(&kind),
    )
}

/// The lane `model`'s calls run in: its route, as `provider/wire id`. When no
/// configured provider serves it, what to configure instead. The compactor
/// reports that as waiting for a model, rather than failing every node
/// against a backend that is not there.
pub(crate) fn compactor_lane(
    model: &str,
    registry: &ModelRegistry,
    configured: &[ProviderKind],
) -> Result<String, String> {
    resolve_route(registry, model, None, |kind| configured.contains(&kind))
        .map(|route| format!("{}/{}", route.provider.as_str(), route.wire_id))
        .map_err(|Unconfigured(kind)| {
            format!(
                "no configured provider serves {model}; configure {} or pick another model",
                kind.as_str()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::prefs;
    use crate::llm::model_registry::load_from_db;
    use crate::llm::reasoning::clamp_effort;
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

    fn compactor_ids() -> Vec<&'static str> {
        COMPACTOR_MODELS.iter().map(|m| m.id).collect()
    }

    /// The comparison measured one model per provider family, and the price
    /// table holds exactly those.
    #[test]
    fn the_price_table_holds_one_model_per_provider_family() {
        assert_eq!(
            compactor_ids(),
            ["gpt-6.1-sol", "gemini-3.8-flash", "claude-sonnet-5-5"]
        );
    }

    /// The estimate prices the default, so every entry must be a priced
    /// compactor row.
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
            only(kinds),
        )
    }

    #[test]
    fn a_stored_model_and_tier_win_over_the_default() {
        let s = selected(Some("gemini-3.5-flash"), Some("high"), &ProviderKind::ALL);
        assert_eq!(s.model, "gemini-3.5-flash");
        assert_eq!(s.effort, "high");
        assert_eq!(s.source, ModelSource::Preference);
    }

    #[test]
    fn a_model_picked_without_a_tier_runs_at_its_default_tier() {
        assert_eq!(selected(Some("gpt-6.1-sol"), None, &[]).effort, "low");
        assert_eq!(
            selected(Some("gemini-3.5-flash"), None, &[]).effort,
            FALLBACK_EFFORT
        );
    }

    #[test]
    fn a_stored_tier_applies_to_the_default_model() {
        let s = selected(None, Some("medium"), &[ProviderKind::OpenAi]);
        assert_eq!(
            (s.model.as_str(), s.effort.as_str()),
            ("gpt-6.1-sol", "medium")
        );
        assert_eq!(s.source, ModelSource::Default);
    }

    /// Nothing on the list is reachable on a local-only install, so the
    /// compactor runs on whatever the workspace chats with.
    #[test]
    fn with_no_default_reachable_the_chat_model_compacts() {
        let s = selected(None, None, &[ProviderKind::Local]);
        assert_eq!(s.model, "my-chat-model");
        assert_eq!(s.effort, FALLBACK_EFFORT);
        assert_eq!(s.source, ModelSource::ChatModel);
    }

    /// A selection nothing configured can serve says what to configure, so
    /// the compactor waits visibly instead of failing every node. One a
    /// provider serves names the lane its calls share.
    #[test]
    fn an_unreachable_model_names_what_to_configure() {
        let registry = crate::llm::model_registry::empty();
        let reason = compactor_lane("gpt-6.1-sol", &registry, &[ProviderKind::Vertex])
            .expect_err("OpenAI is not configured");
        assert!(
            reason.contains("gpt-6.1-sol") && reason.contains("openai"),
            "{reason}"
        );
        assert_eq!(
            compactor_lane("gpt-6.1-sol", &registry, &[ProviderKind::OpenAi]),
            Ok("openai/gpt-6.1-sol".to_string())
        );
    }

    /// How prose names each compactor model, where it differs from the id.
    const PROSE_LABELS: &[(&str, &str)] = &[
        ("gpt-6.1-sol", "GPT-6.1 Sol"),
        ("gemini-3.8-flash", "Gemini 3.8 Flash"),
        ("claude-sonnet-5-5", "Sonnet 5.5"),
    ];

    /// Prose names the default models and tier for the agent, Settings and
    /// readers: the two preferences' texts, `preferences.md` and the glossary.
    /// A pin, since prose cannot import the list.
    #[test]
    fn every_prose_copy_names_the_defaults() {
        let unset = |spec: &prefs::PrefSpec| match spec.default {
            prefs::PrefDefault::Unset(text) => text,
            _ => panic!("{} must be unset by default", spec.key),
        };
        let doc = |path: &str| {
            std::fs::read_to_string(format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR")))
                .unwrap_or_else(|e| panic!("{path}: {e}"))
        };
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

    /// The selection reads the two stored keys, and nothing they inherit.
    #[tokio::test]
    async fn the_selection_reads_the_stored_keys() {
        let (pool, db_name) = setup_test_db().await;
        let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
        let unset =
            compactor_selection(&pool, &registry, &[ProviderKind::Vertex], "my-chat-model").await;
        assert_eq!(unset.model, "gemini-3.8-flash");
        assert_eq!(unset.source, ModelSource::Default);

        crate::core::PreferenceStore::set_row_for_test(
            &pool,
            prefs::MODEL_MEMORY.key(),
            "gpt-5.4-mini",
        )
        .await
        .unwrap();
        let memory_only =
            compactor_selection(&pool, &registry, &[ProviderKind::Vertex], "my-chat-model").await;
        assert_eq!(memory_only.model, "gemini-3.8-flash", "no inheritance");

        for (key, value) in [
            (prefs::MODEL_SUMMARY_COMPACTION.key(), "claude-sonnet-5-5"),
            (prefs::REASONING_SUMMARY_COMPACTION.key(), "high"),
        ] {
            crate::core::PreferenceStore::set_row_for_test(&pool, key, value)
                .await
                .unwrap();
        }
        let stored =
            compactor_selection(&pool, &registry, &[ProviderKind::Vertex], "my-chat-model").await;
        assert_eq!(stored.model, "claude-sonnet-5-5");
        assert_eq!(stored.effort, "high");
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
            only(kinds),
        );
        if source != ModelSource::Default {
            return None;
        }
        let entry = COMPACTOR_DEFAULTS.iter().find(|e| e.model == model)?;
        let route = resolve_route(registry, entry.model, None, only(kinds)).ok()?;
        Some((entry.model, entry.effort, route.provider))
    }

    /// The default for each provider set, against the registry the migrations
    /// leave. A shipped workspace resolves exactly this, so the seeded routes
    /// and the list are pinned together.
    #[tokio::test]
    async fn the_default_follows_the_configured_providers() {
        let (pool, db_name) = setup_test_db().await;
        let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
        use ProviderKind::*;
        let all = ProviderKind::ALL;
        assert_eq!(
            resolved(&registry, &all),
            Some(("gpt-6.1-sol", "low", OpenAi)),
            "every provider configured"
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

    /// A default tier that routing would snap is a default the estimate and
    /// the picker misreport. Every entry keeps its tier on every route it has.
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
}
