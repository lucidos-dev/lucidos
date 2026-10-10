//! The *auxiliary default*: the model an auxiliary purpose runs on while its
//! model preference is unset, resolved against the configured providers.
//!
//! The list a purpose resolves from is also what Settings recommends for it,
//! best first. Decisions and evidence:
//! `docs/plans/2026-10-06-auxiliary-calls-through-the-router.md`.

use serde::Serialize;

use super::reach::Reach;
use super::{model_source, needs_vision, AuxModelSource, ContextPurpose};
use crate::engine::summary_tree::{default_effort_for, COMPACTOR_DEFAULTS};
use crate::llm::model_registry::{reads_images, ModelRegistry};

/// Where a selection's model came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelSource {
    /// The purpose's model preference, as the user set it.
    Preference,
    /// The first recommended model a configured provider serves, skipping
    /// any it answered not-found for (ADR 0403).
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

/// The first of `candidates` that `reach` serves.
fn first_reachable<'a>(
    candidates: impl IntoIterator<Item = &'a str>,
    registry: &ModelRegistry,
    reach: &Reach,
) -> Option<&'a str> {
    candidates
        .into_iter()
        .find(|model| reach.serves(registry, model))
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

/// The tier the badge in `purpose`'s picker marks as recommended for `model`.
/// Only the compactor has one: its tiers are the measured *compactor
/// default*'s. Every other purpose's default tier was never benchmarked.
pub(crate) fn recommended_effort(purpose: ContextPurpose, model: &str) -> Option<&'static str> {
    match model_source(purpose) {
        AuxModelSource::ProviderResolved => default_effort_for(model),
        _ => None,
    }
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
    reach: &Reach,
) -> (String, ModelSource) {
    if let Some(model) = stored {
        return (model, ModelSource::Preference);
    }
    match first_reachable(recommended.iter().map(String::as_str), registry, reach) {
        Some(model) => (model.to_string(), ModelSource::Default),
        None => (chat_model.to_string(), ModelSource::ChatModel),
    }
}

/// The reachable recommended model after `model`: where a default call moves
/// when `model` answers not-found. A stored pick and the chat model move
/// nowhere, so only a [`ModelSource::Default`] selection asks.
pub(crate) fn next_reachable(
    model: &str,
    recommended: &[String],
    registry: &ModelRegistry,
    reach: &Reach,
) -> Option<String> {
    let after = recommended
        .iter()
        .skip_while(|candidate| *candidate != model)
        .skip(1)
        .map(String::as_str);
    first_reachable(after, registry, reach).map(str::to_string)
}

/// The models `purpose` would run on ahead of the one in force, plus that one,
/// whose provider answered not-found. Settings names them.
pub(crate) fn passed_over(
    model: &str,
    source: ModelSource,
    recommended: &[String],
    registry: &ModelRegistry,
    reach: &Reach,
) -> Vec<String> {
    let ahead: Vec<&str> = match source {
        ModelSource::Preference => Vec::new(),
        ModelSource::Default | ModelSource::ChatModel => recommended
            .iter()
            .map(String::as_str)
            .take_while(|candidate| *candidate != model)
            .collect(),
    };
    ahead
        .into_iter()
        .chain(std::iter::once(model))
        .filter(|candidate| reach.answered_not_found(registry, candidate))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
#[path = "default_tests.rs"]
mod tests;
