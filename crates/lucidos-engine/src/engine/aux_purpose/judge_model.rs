//! INVARIANT: Whether the command judge has a model to run on. It never falls back to
//! the chat model: with the guard and the judge on, an unresolved judge
//! refuses a Lucidos Agent turn, and coding-agent auto-answer cards instead.
//! Plan: `docs/plans/2026-11-10-command-guard-on-by-default.md`, I4 to I6.

use sqlx::PgPool;

use super::{asks_no_model, recommended, resolve_selection, AuxSelection, ModelSource};
use crate::engine::ContextPurpose;
use crate::llm::judgment::{system_one_for, JudgmentSite};
use crate::llm::model_registry::ModelRegistry;
use crate::llm::LlmProvider;

const FIX: &str = "Pick a judge model under Settings → Models → Command judge, or turn \
                   the judge off under Settings → Permissions → Command safety";

pub async fn guarded_turn_refusal(
    pool: &PgPool,
    registry: &ModelRegistry,
    router: &dyn LlmProvider,
    judge_enabled: bool,
) -> Option<String> {
    if !judge_enabled || !router.is_configured() {
        return None;
    }
    command_judge_unresolved(pool, registry, router)
        .await
        .map(|reason| format!("The command guard could not start this turn. {reason}"))
}

pub(crate) async fn command_judge_unresolved(
    pool: &PgPool,
    registry: &ModelRegistry,
    router: &dyn LlmProvider,
) -> Option<String> {
    let selection = resolve_selection(pool, registry, router, ContextPurpose::CommandJudge).await;
    unresolved_judge(pool, &selection, registry).await
}

pub async fn unresolved_judge(
    pool: &PgPool,
    selection: &AuxSelection,
    registry: &ModelRegistry,
) -> Option<String> {
    let reason = unresolved_reason(
        selection,
        &recommended(ContextPurpose::CommandJudge, registry),
    )?;
    (!system_one_judges(pool).await).then_some(reason)
}

async fn system_one_judges(pool: &PgPool) -> bool {
    let unused_timeout = std::time::Duration::from_secs(1);
    system_one_for(pool, JudgmentSite::CommandGuard, unused_timeout)
        .await
        .is_some()
}

pub(crate) fn unresolved_reason(
    selection: &AuxSelection,
    recommended: &[String],
) -> Option<String> {
    if selection.model == crate::llm::MOCK_MODEL {
        return None;
    }
    if asks_no_model(ContextPurpose::CommandJudge, selection.source) {
        return Some(format!(
            "No configured provider serves a command judge model ({}). {FIX}.",
            recommended.join(", ")
        ));
    }
    match (selection.source, selection.reachable) {
        (ModelSource::Preference, false) => Some(format!(
            "No configured provider serves the chosen command judge model '{}'. {FIX}.",
            selection.model
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(model: &str, source: ModelSource, reachable: bool) -> AuxSelection {
        AuxSelection {
            model: model.to_string(),
            reasoning: None,
            source,
            reachable,
            generation: None,
            fallback: None,
            not_served: Vec::new(),
            needs_vision: false,
            vision: false,
        }
    }

    fn list() -> Vec<String> {
        vec![
            "gemini-3.1-flash-lite".to_string(),
            "gpt-6-luna".to_string(),
        ]
    }

    #[test]
    fn a_reachable_default_or_pick_resolves() {
        for source in [ModelSource::Default, ModelSource::Preference] {
            assert_eq!(
                unresolved_reason(&selection("gpt-6-luna", source, true), &list()),
                None,
                "{source:?}"
            );
        }
    }

    #[test]
    fn the_chat_model_never_resolves_and_the_reason_names_the_list() {
        let reason = unresolved_reason(
            &selection("claude-opus-5-5", ModelSource::ChatModel, true),
            &list(),
        )
        .expect("unresolved");
        assert!(
            reason.contains("gemini-3.1-flash-lite, gpt-6-luna"),
            "{reason}"
        );
        assert!(
            reason.contains("Settings → Models → Command judge"),
            "{reason}"
        );
        assert!(!reason.contains("claude-opus-5-5"), "{reason}");
    }

    #[test]
    fn an_unserved_pick_is_named() {
        let reason = unresolved_reason(
            &selection("z-ai/glm-5.2", ModelSource::Preference, false),
            &list(),
        )
        .expect("unresolved");
        assert!(reason.contains("'z-ai/glm-5.2'"), "{reason}");
    }

    #[test]
    fn the_mock_resolves() {
        let mock = selection(crate::llm::MOCK_MODEL, ModelSource::ChatModel, true);
        assert_eq!(unresolved_reason(&mock, &list()), None);
    }
}
