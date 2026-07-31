//! In-memory model routing map, projected from the `models` table.
//!
//! `RoutingProvider` consults this to pick which backend (Vertex / direct
//! Anthropic / OpenAI) serves a given model id, and the context trimmer
//! consults it for the model's declared context window. The map is
//! hot-swappable: the engine's `spawn_models_registry_subscriber` reloads it
//! whenever a `Model*` event fires, so adding a model, re-providering it, or
//! correcting its context window in Settings takes effect without a restart.

use crate::core::ModelStore;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Which provider backend serves a model. `OpenAi`, `OpenRouter`, and `Local`
/// all speak the OpenAI Chat Completions wire format but are distinct backends
/// (different base URL / key / headers), so they map to separate provider
/// instances in [`crate::llm::routing::RoutingProvider`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Vertex,
    Anthropic,
    OpenAi,
    /// OpenRouter (`https://openrouter.ai/api/v1`) — e.g. GLM 5.2.
    OpenRouter,
    /// A generic OpenAI-compatible local server (Ollama / LM Studio / vLLM /
    /// llama.cpp), base URL configurable.
    Local,
}

impl ProviderKind {
    /// Parse a `models.provider` column value. Unknown strings fall back to
    /// Vertex — the historical default for every non-`gpt-` model — so a row
    /// written by a newer engine with an unrecognized provider still routes
    /// somewhere sane rather than erroring.
    pub fn parse(s: &str) -> Self {
        match s {
            "anthropic" => Self::Anthropic,
            "openai" => Self::OpenAi,
            "openrouter" => Self::OpenRouter,
            "local" => Self::Local,
            _ => Self::Vertex,
        }
    }

    /// The `models.provider` column string — inverse of [`Self::parse`]. Used to
    /// report configured providers over `/health` in the same vocabulary the
    /// model rows (and the frontend filter) use.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Vertex => "vertex",
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::OpenRouter => "openrouter",
            Self::Local => "local",
        }
    }
}

/// What the registry knows about one model id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRouting {
    pub provider: ProviderKind,
    /// Context window in tokens as declared on the `models` row. `None` = not
    /// declared, so [`context_window_for`] falls back to the id-shape guess in
    /// `engine::context::context_window_from_prefix`.
    pub context_window: Option<usize>,
}

/// Shared, hot-swappable model routing map. Cloned into `RoutingProvider` and
/// held by the engine; the reload subscriber swaps the inner map on `Model*`
/// events.
pub type ModelRegistry = Arc<RwLock<HashMap<String, ModelRouting>>>;

/// Build an empty registry handle (before the first DB load, and in tests).
pub fn empty() -> ModelRegistry {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Last-resort context window guessed from the model-id shape, for ids with no
/// registry row at all.
///
/// **Prefer the declared window** — call [`context_window_for`], which consults
/// the `models` registry first and only lands here on a miss.
///
/// What the rules mean, and where they fall short:
///
/// - `[1m]` → 1M is correct: the suffix is Lucidos's own marker for "request 1M
///   mode", and `build_claude_request` attaches the `context-1m-2025-08-07` beta
///   for exactly those ids.
/// - `claude-` → 200k is **also correct**, and deliberately so. A bare id sends
///   no 1M beta, so 200k is the window of the request the engine actually makes
///   — not a stale guess about the model's maximum. Declaring 1M on a bare row
///   would let the context packer exceed the API mode the request selected.
/// - `gpt-5` → 400k **understates** the GPT-5.5 / GPT-5.6 families (1,050,000).
///   The OpenAI path has no context opt-in, so the full window always applies;
///   those rows declare it instead.
/// - There is **no rule at all** for OpenRouter / Gemini / local ids, so they
///   take the bare 200k default — kimi-k3 (1,048,576 real) was budgeted at 200k
///   and the trim loop evicted context at ~8% of the true window. That is the
///   gap the `context_window` column was added to close.
///
/// Every guess here errs low on purpose. Under-reporting only trims context
/// earlier than necessary; over-reporting makes the engine pack a prompt the
/// provider then rejects.
///
/// What still legitimately lands here: Claude Code model ids, which live in
/// `runtime/cc_menu_options.json` and never get a `models` row, plus a legacy
/// `chat_model` preference naming a model the user has since deleted.
///
/// Lives here rather than in `engine::context` because it is model-id
/// knowledge, and `llm/` must not depend on `crate::engine` (enforced by
/// `llm::validate::tests::llm_does_not_depend_on_engine`).
pub fn context_window_from_prefix(model: &str) -> usize {
    if model.contains("[1m]") {
        return 1_000_000;
    }
    if model.starts_with("claude-") {
        return 200_000;
    }
    if model.starts_with("gpt-5") {
        return 400_000;
    }
    200_000
}

/// Normalize a stored `models.context_window` into a usable window.
///
/// A non-positive value is treated as undeclared. The API rejects those, but the
/// column is plain nullable SQL — a hand-edited or migrated row must not be able
/// to produce a zero (or, once cast, an enormous wrapped) budget.
fn declared_window(raw: Option<i32>) -> Option<usize> {
    raw.filter(|w| *w > 0).map(|w| w as usize)
}

/// Load the routing map from the `models` table — all rows, enabled or not,
/// since routing must still resolve a model that was disabled after a user saved
/// it as their `chat_model`. On a DB error, return an empty map and log; routing
/// then degrades to the prefix heuristic rather than failing every call.
pub async fn load_from_db(pool: &PgPool) -> HashMap<String, ModelRouting> {
    match ModelStore::list(pool).await {
        Ok(models) => models
            .into_iter()
            .map(|m| {
                (
                    m.id,
                    ModelRouting {
                        provider: ProviderKind::parse(&m.provider),
                        context_window: declared_window(m.context_window),
                    },
                )
            })
            .collect(),
        Err(e) => {
            crate::log!(
                "[ModelRegistry] Failed to load models table; routing falls back to prefix heuristic: {}",
                e
            );
            HashMap::new()
        }
    }
}

/// Look up a model id in the registry, if the lock is readable.
fn routing_for(registry: &ModelRegistry, model: &str) -> Option<ModelRouting> {
    registry.read().ok().and_then(|map| map.get(model).copied())
}

/// Resolve the provider for a model id. Exact registry hit first; on a miss
/// (unknown id, a legacy saved preference no longer in the table, or a poisoned
/// lock) fall back to the prefix heuristic so routing never dead-ends.
pub fn provider_kind_for(registry: &ModelRegistry, model: &str) -> ProviderKind {
    routing_for(registry, model)
        .map(|r| r.provider)
        .unwrap_or_else(|| prefix_heuristic(model))
}

/// Resolve the context window for a model id: the declared window if the row
/// has one, else the id-shape guess.
///
/// This is the fix for the trim budget being computed from a hardcoded prefix
/// map that had no rule for OpenRouter / Gemini / local ids and handed all of
/// them 200k — kimi-k3 (1,048,576 real) was trimmed as if it held 200k, so the
/// agentic loop evicted context at roughly 8% of the true window.
pub fn context_window_for(registry: &ModelRegistry, model: &str) -> usize {
    routing_for(registry, model)
        .and_then(|r| r.context_window)
        .unwrap_or_else(|| context_window_from_prefix(model))
}

/// Last-resort provider guess from the model-string shape. Mirrors the
/// pre-registry routing rule (`gpt-` → OpenAI, else Vertex) extended so direct
/// Anthropic models (Fable 5) still route when absent from the table.
fn prefix_heuristic(model: &str) -> ProviderKind {
    if model.starts_with("gpt-") {
        ProviderKind::OpenAi
    } else if model.contains("claude-fable") {
        ProviderKind::Anthropic
    } else {
        ProviderKind::Vertex
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(pairs: &[(&str, ProviderKind)]) -> ModelRegistry {
        Arc::new(RwLock::new(
            pairs
                .iter()
                .map(|(k, v)| {
                    (
                        k.to_string(),
                        ModelRouting {
                            provider: *v,
                            context_window: None,
                        },
                    )
                })
                .collect(),
        ))
    }

    fn registry_with_windows(pairs: &[(&str, Option<usize>)]) -> ModelRegistry {
        Arc::new(RwLock::new(
            pairs
                .iter()
                .map(|(k, w)| {
                    (
                        k.to_string(),
                        ModelRouting {
                            provider: ProviderKind::OpenRouter,
                            context_window: *w,
                        },
                    )
                })
                .collect(),
        ))
    }

    #[test]
    fn parse_maps_known_providers_and_defaults_to_vertex() {
        assert_eq!(ProviderKind::parse("anthropic"), ProviderKind::Anthropic);
        assert_eq!(ProviderKind::parse("openai"), ProviderKind::OpenAi);
        assert_eq!(ProviderKind::parse("openrouter"), ProviderKind::OpenRouter);
        assert_eq!(ProviderKind::parse("local"), ProviderKind::Local);
        assert_eq!(ProviderKind::parse("vertex"), ProviderKind::Vertex);
        assert_eq!(ProviderKind::parse("something-new"), ProviderKind::Vertex);
    }

    #[test]
    fn registry_routes_openrouter_and_local_by_exact_hit() {
        // OpenRouter / local ids have no prefix heuristic — the registry exact
        // hit is authoritative (the seeded GLM 5.2 builtin is never deletable,
        // so its mapping is always present).
        let reg = registry(&[
            ("z-ai/glm-5.2", ProviderKind::OpenRouter),
            ("llama3.1", ProviderKind::Local),
        ]);
        assert_eq!(
            provider_kind_for(&reg, "z-ai/glm-5.2"),
            ProviderKind::OpenRouter
        );
        assert_eq!(provider_kind_for(&reg, "llama3.1"), ProviderKind::Local);
        // Absent from the table → prefix heuristic, which has no rule for these
        // shapes, so it falls back to Vertex (documented limitation).
        assert_eq!(
            provider_kind_for(&empty(), "z-ai/glm-5.2"),
            ProviderKind::Vertex
        );
    }

    #[test]
    fn exact_registry_hit_wins() {
        let reg = registry(&[
            ("claude-fable-5", ProviderKind::Anthropic),
            ("claude-opus-4-8@default", ProviderKind::Vertex),
            ("gpt-5.5", ProviderKind::OpenAi),
        ]);
        assert_eq!(
            provider_kind_for(&reg, "claude-fable-5"),
            ProviderKind::Anthropic
        );
        assert_eq!(
            provider_kind_for(&reg, "claude-opus-4-8@default"),
            ProviderKind::Vertex
        );
        assert_eq!(provider_kind_for(&reg, "gpt-5.5"), ProviderKind::OpenAi);
    }

    #[test]
    fn registry_can_override_prefix_heuristic() {
        // A user could put a normally-Vertex Claude model onto the direct
        // Anthropic provider; the table wins over the heuristic.
        let reg = registry(&[("claude-opus-4-6", ProviderKind::Anthropic)]);
        assert_eq!(
            provider_kind_for(&reg, "claude-opus-4-6"),
            ProviderKind::Anthropic
        );
    }

    #[test]
    fn miss_falls_back_to_prefix_heuristic() {
        let reg = empty();
        // Legacy saved prefs / ids not in the table still route.
        assert_eq!(provider_kind_for(&reg, "gpt-5.4"), ProviderKind::OpenAi);
        assert_eq!(
            provider_kind_for(&reg, "claude-fable-5[1m]"),
            ProviderKind::Anthropic
        );
        assert_eq!(
            provider_kind_for(&reg, "claude-opus-4-7[1m]"),
            ProviderKind::Vertex
        );
        // Opus 5 is not in the table here; the prefix heuristic routes any
        // non-fable `claude-*` to Vertex, matching the seeded provider.
        assert_eq!(
            provider_kind_for(&reg, "claude-opus-5@default"),
            ProviderKind::Vertex
        );
        assert_eq!(
            provider_kind_for(&reg, "gemini-3-flash-preview"),
            ProviderKind::Vertex
        );
    }

    #[test]
    fn context_window_for_known_models() {
        assert_eq!(context_window_from_prefix("claude-opus-4-7[1m]"), 1_000_000);
        assert_eq!(context_window_from_prefix("claude-opus-4-7"), 200_000);
        assert_eq!(context_window_from_prefix("claude-sonnet-4-6"), 200_000);
        assert_eq!(context_window_from_prefix("gpt-5"), 400_000);
        assert_eq!(context_window_from_prefix("unknown-model"), 200_000);
    }

    /// Where the prefix map falls short, pinned so the limitation stays visible
    /// and nobody "fixes" it by bolting more prefixes on — the fix is to declare
    /// the window on the row.
    ///
    /// Every guess errs low on purpose: under-reporting only trims early, while
    /// over-reporting makes the engine pack a prompt the provider rejects.
    #[test]
    fn prefix_map_under_reports_where_it_has_no_rule() {
        // No rule at all for OpenRouter / Gemini / local ids → bare 200k default.
        for id in [
            "moonshotai/kimi-k3",
            "z-ai/glm-5.2",
            "gemini-3.1-pro-preview",
            "gemini-3.5-flash",
        ] {
            assert_eq!(
                context_window_from_prefix(id),
                200_000,
                "{id} falls back to 200k — its real window must come from the registry"
            );
        }

        // `gpt-5` → 400k, but the 5.5 / 5.6 families are really 1,050,000 and
        // the OpenAI path has no context opt-in to gate it behind.
        for id in ["gpt-5.5", "gpt-5.5-pro", "gpt-5.6-sol"] {
            assert_eq!(
                context_window_from_prefix(id),
                400_000,
                "{id} guesses 400k — its real window must come from the registry"
            );
        }
    }

    /// The `[1m]`-vs-bare split is NOT an oversight — it mirrors what the engine
    /// actually requests. `build_claude_request` attaches the
    /// `context-1m-2025-08-07` beta only when `parse_context_suffix` reports
    /// `is_1m`, so a bare id genuinely runs at 200k however large the model is.
    /// Pinned so nobody "corrects" the bare rows to 1M and starts building
    /// prompts the API rejects.
    #[test]
    fn bare_claude_ids_are_correctly_200k_because_they_send_no_1m_beta() {
        for base in [
            "claude-fable-5",
            "claude-opus-5@default",
            "claude-opus-4-8@default",
            "claude-sonnet-4-6",
        ] {
            assert_eq!(
                context_window_from_prefix(base),
                200_000,
                "{base} sends no 1M beta, so 200k is the real request window"
            );
            assert_eq!(
                context_window_from_prefix(&format!("{base}[1m]")),
                1_000_000,
                "{base}[1m] requests 1M mode, so it gets the 1M window"
            );
        }
    }

    /// The whole point of the `context_window` column: a declared window wins,
    /// so kimi-k3 stops being budgeted as a 200k model.
    #[test]
    fn declared_context_window_wins_over_the_prefix_fallback() {
        let reg = registry_with_windows(&[("moonshotai/kimi-k3", Some(1_048_576))]);
        assert_eq!(context_window_for(&reg, "moonshotai/kimi-k3"), 1_048_576);
    }

    /// An undeclared window, an unknown id, and an empty registry all fall
    /// through to the id-shape guess with exactly today's results — the
    /// back-compat half of the change.
    #[test]
    fn undeclared_window_falls_back_to_the_prefix_map() {
        let reg = registry_with_windows(&[("moonshotai/kimi-k3", None)]);
        // Declared-as-None is the same as absent.
        assert_eq!(context_window_for(&reg, "moonshotai/kimi-k3"), 200_000);
        // Not in the table at all.
        assert_eq!(context_window_for(&reg, "claude-opus-4-7[1m]"), 1_000_000);
        assert_eq!(context_window_for(&reg, "gpt-5.5"), 400_000);
        // Empty registry — every id takes the prefix map.
        let none = empty();
        assert_eq!(context_window_for(&none, "claude-opus-4-7"), 200_000);
        assert_eq!(context_window_for(&none, "claude-opus-4-7[1m]"), 1_000_000);
        assert_eq!(context_window_for(&none, "unknown-model"), 200_000);
    }

    /// A declared window can also be SMALLER than the id-shape guess — a
    /// `claude-`-prefixed proxy or fine-tune served with a 32k window must be
    /// able to say so, or the budget over-promises and the request 400s.
    #[test]
    fn declared_window_can_shrink_as_well_as_grow() {
        let reg = registry_with_windows(&[("claude-opus-4-7", Some(32_000))]);
        assert_eq!(context_window_for(&reg, "claude-opus-4-7"), 32_000);
    }

    /// A hand-edited zero / negative row must not reach the map — otherwise it
    /// would produce a zero budget (trimming everything) or, cast from a
    /// negative i32, an enormous one. `declared_window` drops them so the
    /// prefix map takes over.
    #[test]
    fn non_positive_declared_window_is_treated_as_undeclared() {
        assert_eq!(declared_window(Some(0)), None);
        assert_eq!(declared_window(Some(-1)), None);
        assert_eq!(declared_window(None), None);
        assert_eq!(declared_window(Some(1_048_576)), Some(1_048_576));
    }
}
