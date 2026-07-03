//! In-memory model → provider routing map, projected from the `models` table.
//!
//! `RoutingProvider` consults this to pick which backend (Vertex / direct
//! Anthropic / OpenAI) serves a given model id. The map is hot-swappable: the
//! engine's `spawn_models_registry_subscriber` reloads it whenever a `Model*`
//! event fires, so adding or re-providering a model in Settings takes effect
//! without a restart.

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

/// Shared, hot-swappable model→provider map. Cloned into `RoutingProvider` and
/// held by the engine; the reload subscriber swaps the inner map on `Model*`
/// events.
pub type ModelRegistry = Arc<RwLock<HashMap<String, ProviderKind>>>;

/// Build an empty registry handle (before the first DB load, and in tests).
pub fn empty() -> ModelRegistry {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Load the id→provider map from the `models` table — all rows, enabled or not,
/// since routing must still resolve a model that was disabled after a user saved
/// it as their `chat_model`. On a DB error, return an empty map and log; routing
/// then degrades to the prefix heuristic rather than failing every call.
pub async fn load_from_db(pool: &PgPool) -> HashMap<String, ProviderKind> {
    match ModelStore::list(pool).await {
        Ok(models) => models
            .into_iter()
            .map(|m| (m.id, ProviderKind::parse(&m.provider)))
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

/// Resolve the provider for a model id. Exact registry hit first; on a miss
/// (unknown id, a legacy saved preference no longer in the table, or a poisoned
/// lock) fall back to the prefix heuristic so routing never dead-ends.
pub fn provider_kind_for(registry: &ModelRegistry, model: &str) -> ProviderKind {
    if let Ok(map) = registry.read() {
        if let Some(kind) = map.get(model) {
            return *kind;
        }
    }
    prefix_heuristic(model)
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
            pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
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
        assert_eq!(provider_kind_for(&empty(), "z-ai/glm-5.2"), ProviderKind::Vertex);
    }

    #[test]
    fn exact_registry_hit_wins() {
        let reg = registry(&[
            ("claude-fable-5", ProviderKind::Anthropic),
            ("claude-opus-4-8@default", ProviderKind::Vertex),
            ("gpt-5.5", ProviderKind::OpenAi),
        ]);
        assert_eq!(provider_kind_for(&reg, "claude-fable-5"), ProviderKind::Anthropic);
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
        assert_eq!(
            provider_kind_for(&reg, "gemini-3-flash-preview"),
            ProviderKind::Vertex
        );
    }
}
