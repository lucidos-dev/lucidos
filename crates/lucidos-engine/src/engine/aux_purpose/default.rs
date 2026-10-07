//! The *auxiliary default*: the model an auxiliary purpose runs on while its
//! model preference is unset, resolved against the configured providers.
//!
//! The list a purpose resolves from is also what Settings recommends for it,
//! best first. Decisions and evidence:
//! `docs/plans/2026-10-06-auxiliary-calls-through-the-router.md`.

use serde::Serialize;

use super::{model_source, needs_vision, AuxModelSource, ContextPurpose};
use crate::engine::summary_tree::COMPACTOR_DEFAULTS;
use crate::llm::model_registry::{reads_images, resolve_route, ModelRegistry, ProviderKind};

/// Where a selection's model came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelSource {
    /// The purpose's model preference, as the user set it.
    Preference,
    /// The first recommended model a configured provider serves.
    Default,
    /// No recommended model is reachable, so the call runs on the chat model.
    ChatModel,
}

/// What every auxiliary purpose falls back to after its own catalog default,
/// cheapest first. Between them they reach Vertex, OpenAI, Anthropic and
/// OpenRouter; each has a registry row naming its routes.
pub(crate) const AUX_FALLBACKS: &[&str] =
    &["gemini-3-flash-preview", "gpt-5.4-mini", "claude-haiku-4-5"];

/// The operator's override of every purpose's first choice but the judge's.
const EXTRACTION_MODEL_ENV: &str = "LUCIDOS_EXTRACTION_MODEL";

/// Whether a configured provider serves `model`.
pub(crate) fn is_reachable(
    model: &str,
    registry: &ModelRegistry,
    is_configured: impl Fn(ProviderKind) -> bool,
) -> bool {
    resolve_route(registry, model, None, is_configured).is_ok()
}

/// The first of `candidates` a configured provider serves.
fn first_reachable<'a>(
    candidates: impl IntoIterator<Item = &'a str>,
    registry: &ModelRegistry,
    is_configured: impl Fn(ProviderKind) -> bool,
) -> Option<&'a str> {
    candidates
        .into_iter()
        .find(|model| is_reachable(model, registry, &is_configured))
}

/// What a purpose's measured comparison put ahead of its catalog default.
/// Evidence: `docs/plans/2026-10-06-memory-tasks-model-comparison.md`.
fn purpose_lead(purpose: ContextPurpose) -> Vec<&'static str> {
    match purpose {
        ContextPurpose::Memory => vec!["gpt-5.6-luna"],
        // The two summary writers share one measured list.
        ContextPurpose::ConversationSummary => {
            COMPACTOR_DEFAULTS.iter().map(|entry| entry.model).collect()
        }
        _ => Vec::new(),
    }
}

/// The models `purpose` recommends, best first, each once. Settings shows
/// them first, and the first a configured provider serves is the default.
///
/// `LUCIDOS_EXTRACTION_MODEL` leads when set, except for the command judge,
/// which it never reached. Then the purpose's measured lead, its catalog
/// default, and [`AUX_FALLBACKS`]. The compactor recommends its own measured
/// list (ADR 0373). A purpose with no preference pair recommends nothing.
///
/// A purpose that needs vision recommends only models that read images, so
/// its default never lands on one it would then refuse.
pub(crate) fn recommended(purpose: ContextPurpose, registry: &ModelRegistry) -> Vec<String> {
    let env = std::env::var(EXTRACTION_MODEL_ENV)
        .ok()
        .filter(|model| !model.trim().is_empty());
    keep_capable(purpose, recommended_with(purpose, env.as_deref()), registry)
}

/// `list` without the models `purpose` cannot run on.
fn keep_capable(
    purpose: ContextPurpose,
    list: Vec<String>,
    registry: &ModelRegistry,
) -> Vec<String> {
    match needs_vision(purpose) {
        true => list
            .into_iter()
            .filter(|model| reads_images(registry, model))
            .collect(),
        false => list,
    }
}

fn recommended_with(purpose: ContextPurpose, extraction_override: Option<&str>) -> Vec<String> {
    let pair = match model_source(purpose) {
        AuxModelSource::Preferences(pair) => pair,
        AuxModelSource::ProviderResolved => {
            return COMPACTOR_DEFAULTS
                .iter()
                .map(|entry| entry.model.to_string())
                .collect();
        }
        AuxModelSource::Turn
        | AuxModelSource::BackendPinned
        | AuxModelSource::AgentModel
        | AuxModelSource::CallerChosen => {
            return Vec::new();
        }
    };
    let env_override = extraction_override.filter(|_| purpose != ContextPurpose::CommandJudge);
    let mut list: Vec<String> = Vec::new();
    for model in env_override
        .into_iter()
        .chain(purpose_lead(purpose))
        .chain([pair.model.default_text()])
        .chain(AUX_FALLBACKS.iter().copied())
    {
        if !list.iter().any(|seen| seen == model) {
            list.push(model.to_string());
        }
    }
    list
}

/// The model one call runs on, and where it came from.
///
/// A stored pick wins whether or not anything serves it: it is honoured or
/// refused, never moved to another vendor. Unset, the first reachable
/// recommended model wins, else the chat model.
pub(crate) fn select(
    stored: Option<String>,
    recommended: &[String],
    chat_model: &str,
    registry: &ModelRegistry,
    is_configured: impl Fn(ProviderKind) -> bool,
) -> (String, ModelSource) {
    if let Some(model) = stored {
        return (model, ModelSource::Preference);
    }
    match first_reachable(
        recommended.iter().map(String::as_str),
        registry,
        is_configured,
    ) {
        Some(model) => (model.to_string(), ModelSource::Default),
        None => (chat_model.to_string(), ModelSource::ChatModel),
    }
}

#[cfg(test)]
#[path = "default_tests.rs"]
mod tests;
