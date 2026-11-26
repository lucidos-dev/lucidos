use std::collections::BTreeMap;

use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;

use super::AppState;
use crate::engine::aux_purpose::{
    asks_no_model, model_key, recommended, recommended_effort, unresolved_judge, GenerationStep,
    ModelSource, BACKGROUND_ROWS,
};
use crate::engine::ContextPurpose;
use crate::llm::model_registry::ModelRegistry;

#[derive(Debug, Serialize)]
pub(crate) struct BackgroundModel {
    pub(crate) model: String,
    pub(crate) effort: Option<String>,
    pub(crate) source: ModelSource,
    pub(crate) reachable: bool,
    pub(crate) generation: Option<GenerationStep>,
    pub(crate) not_served: Vec<String>,
    pub(crate) needs_vision: bool,
    pub(crate) vision: bool,
    pub(crate) asks_no_model: bool,
    pub(crate) unresolved: Option<String>,
    pub(crate) recommended: Vec<RecommendedSelection>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct RecommendedSelection {
    pub(crate) model: String,
    pub(crate) effort: Option<&'static str>,
}

fn recommended_selections(
    purpose: ContextPurpose,
    registry: &ModelRegistry,
) -> Vec<RecommendedSelection> {
    recommended(purpose, registry)
        .into_iter()
        .map(|model| RecommendedSelection {
            effort: recommended_effort(purpose, &model),
            model,
        })
        .collect()
}

async fn get_background_models(
    State(state): State<AppState>,
) -> Json<BTreeMap<&'static str, BackgroundModel>> {
    let registry = state.engine.model_registry();
    let mut rows = BTreeMap::new();
    for purpose in BACKGROUND_ROWS {
        let Some(key) = model_key(*purpose) else {
            continue;
        };
        let call = state.engine.aux_call(*purpose).await;
        let selection = call.selection();
        rows.insert(
            key,
            BackgroundModel {
                model: selection.model.clone(),
                effort: selection.reasoning.clone(),
                source: selection.source,
                reachable: selection.reachable,
                generation: selection.generation.clone(),
                not_served: selection.not_served.clone(),
                needs_vision: selection.needs_vision,
                vision: selection.vision,
                asks_no_model: asks_no_model(*purpose, selection.source),
                unresolved: match purpose {
                    ContextPurpose::CommandJudge => {
                        unresolved_judge(&state.pool, selection, registry).await
                    }
                    _ => None,
                },
                recommended: recommended_selections(*purpose, registry),
            },
        );
    }
    Json(rows)
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/models/background", get(get_background_models))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::summary_tree::COMPACTOR_DEFAULTS;

    #[test]
    fn only_the_compactor_row_carries_a_recommended_tier() {
        let registry = crate::llm::model_registry::empty();
        for purpose in BACKGROUND_ROWS {
            let selections = recommended_selections(*purpose, &registry);
            if *purpose == ContextPurpose::SummaryCompaction {
                let expected: Vec<_> = COMPACTOR_DEFAULTS
                    .iter()
                    .map(|entry| RecommendedSelection {
                        model: entry.model.to_string(),
                        effort: Some(entry.effort),
                    })
                    .collect();
                assert_eq!(selections, expected);
            } else {
                assert!(
                    selections.iter().all(|s| s.effort.is_none()),
                    "{purpose:?}: {selections:?}"
                );
            }
        }
    }
}
