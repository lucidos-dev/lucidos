use serde::Serialize;

use super::reach::Reach;
use super::{model_source, needs_vision, AuxModelSource, ContextPurpose};
use crate::engine::summary_tree::{default_effort_for, COMPACTOR_DEFAULTS};
use crate::llm::model_registry::{reads_images, GenerationRule, ModelRegistry};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelSource {
    Preference,
    Default,
    ChatModel,
}

impl ModelSource {
    pub(crate) fn generation_rule(self) -> GenerationRule {
        match self {
            Self::Default => GenerationRule::NamedOrNewer,
            Self::Preference | Self::ChatModel => GenerationRule::AnyGeneration,
        }
    }
}

pub(crate) const AUX_FALLBACKS: &[&str] = &["gpt-6-luna", "gemini-3.1-flash-lite"];

const EXTRACTION_MODEL_ENV: &str = "LUCIDOS_EXTRACTION_MODEL";

fn first_reachable<'a>(
    candidates: impl IntoIterator<Item = &'a str>,
    registry: &ModelRegistry,
    reach: &Reach,
) -> Option<&'a str> {
    let rule = ModelSource::Default.generation_rule();
    candidates
        .into_iter()
        .find(|model| reach.serves(registry, model, rule))
}

fn purpose_lead(purpose: ContextPurpose) -> Vec<&'static str> {
    match purpose {
        ContextPurpose::Memory => vec!["gpt-6-luna"],
        ContextPurpose::ReadDecision => vec!["gemini-3.8-flash", "gpt-6-luna"],
        ContextPurpose::WakeDecision => vec!["gemini-3.8-flash", "gpt-6.1-sol"],
        ContextPurpose::ConversationSummary => {
            COMPACTOR_DEFAULTS.iter().map(|entry| entry.model).collect()
        }
        _ => Vec::new(),
    }
}

/// INVARIANT: What a [`measured_only`] purpose's eval cleared after its catalog default,
/// in place of [`AUX_FALLBACKS`].
fn measured_tail(purpose: ContextPurpose) -> &'static [&'static str] {
    match purpose {
        ContextPurpose::CommandJudge => &["gpt-6-luna", "claude-haiku-5-5"],
        _ => &[],
    }
}

fn measured_only(purpose: ContextPurpose) -> bool {
    matches!(
        purpose,
        ContextPurpose::WakeDecision | ContextPurpose::CommandJudge
    )
}

pub fn asks_no_model(purpose: ContextPurpose, source: ModelSource) -> bool {
    measured_only(purpose) && source == ModelSource::ChatModel
}

pub fn recommended(purpose: ContextPurpose, registry: &ModelRegistry) -> Vec<String> {
    let env = std::env::var(EXTRACTION_MODEL_ENV)
        .ok()
        .filter(|model| !model.trim().is_empty());
    keep_capable(purpose, recommended_with(purpose, env.as_deref()), registry)
}

pub fn recommended_effort(purpose: ContextPurpose, model: &str) -> Option<&'static str> {
    match model_source(purpose) {
        AuxModelSource::ProviderResolved => default_effort_for(model),
        _ => None,
    }
}

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
    let measured = measured_only(purpose);
    let env_override = extraction_override.filter(|_| !measured);
    let fallbacks = if measured {
        measured_tail(purpose)
    } else {
        AUX_FALLBACKS
    };
    let mut list: Vec<String> = Vec::new();
    for model in env_override
        .into_iter()
        .chain(purpose_lead(purpose))
        .chain([pair.model.default_text()])
        .chain(fallbacks.iter().copied())
    {
        if !list.iter().any(|seen| seen == model) {
            list.push(model.to_string());
        }
    }
    list
}

/// INVARIANT: The model one call runs on, and where it came from.
///
/// A stored pick wins whether or not anything serves it: it is honoured or
/// refused, never moved to another vendor. Unset, the first reachable
/// recommended model wins, else the chat model. A [`measured_only`] purpose
/// never asks that chat model ([`asks_no_model`]).
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

/// INVARIANT: The reachable recommended model after `model`: where a default call moves
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
