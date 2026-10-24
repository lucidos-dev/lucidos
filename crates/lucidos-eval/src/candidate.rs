//! A model the single-call checks (`command-judge`, `read-decision`,
//! `wake-decision`) score: its id, the tier it runs at, and the provider that
//! serves it.

use std::sync::Arc;

use lucidos_engine::llm::model_registry::{self, ProviderKind};
use lucidos_engine::llm::provider_build::{build_active_provider, ProviderBuildOutcome};
use lucidos_engine::llm::LlmProvider;

use crate::judge::{judge_context, provider_override, resolve_provider, route_to};
use crate::Fallible;

/// A candidate: a model id and the tier it runs at, written `model@tier`.
pub(crate) struct Candidate<'a> {
    pub(crate) spec: &'a str,
    pub(crate) model: &'a str,
    pub(crate) effort: Option<&'a str>,
}

pub(crate) fn candidate(spec: &str) -> Candidate<'_> {
    match spec.split_once('@') {
        Some((model, effort)) => Candidate {
            spec,
            model,
            effort: Some(effort),
        },
        None => Candidate {
            spec,
            model: spec,
            effort: None,
        },
    }
}

/// The provider pinned by `JUDGE_PROVIDER_VAR`, if any.
pub(crate) fn pinned_provider() -> Fallible<Option<ProviderKind>> {
    provider_override(
        std::env::var(crate::judge::JUDGE_PROVIDER_VAR)
            .ok()
            .as_deref(),
    )
}

/// Build the provider that serves `candidate`, and say which one it is.
pub(crate) async fn provider_for(
    candidate: &Candidate<'_>,
    pinned: Option<ProviderKind>,
) -> Fallible<Arc<dyn LlmProvider>> {
    let registry = model_registry::empty();
    if let Some(provider) = pinned {
        route_to(&registry, candidate.model, provider)?;
    }
    let context = judge_context(candidate.model, &registry);
    let provider = match build_active_provider(None, &context).await? {
        ProviderBuildOutcome::Install { llm, .. } => llm,
        ProviderBuildOutcome::FailFast => return Err("no provider is configured here".into()),
    };
    let configured = provider.configured_providers().unwrap_or_default();
    let served_by = resolve_provider(&registry, candidate.model, pinned, &configured)?;
    eprintln!("[eval] {} on {}", candidate.spec, served_by.as_str());
    Ok(provider)
}

/// The `p` percentile of `sorted`, or 0 when empty.
pub(crate) fn percentile(sorted: &[u128], p: f64) -> u128 {
    sorted
        .get(((sorted.len() as f64 - 1.0) * p).round() as usize)
        .copied()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_candidate_names_its_tier_after_the_at() {
        let c = candidate("claude-haiku-5-5@none");
        assert_eq!((c.model, c.effort), ("claude-haiku-5-5", Some("none")));
        assert_eq!(candidate("gemini-3-flash-preview").effort, None);
    }
}
