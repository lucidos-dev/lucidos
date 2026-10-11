//! `GET /api/v1/models/background`: the model each background task runs on,
//! so Settings shows what will run rather than a catalog default.

use std::collections::BTreeMap;

use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;

use super::AppState;
use crate::engine::aux_purpose::{
    model_key, recommended, recommended_effort, ModelSource, BACKGROUND_ROWS,
};
use crate::engine::ContextPurpose;
use crate::llm::model_registry::ModelRegistry;

/// One background row's resolved *model selection*.
#[derive(Debug, Serialize)]
pub(crate) struct BackgroundModel {
    pub(crate) model: String,
    pub(crate) effort: Option<String>,
    pub(crate) source: ModelSource,
    /// Whether a configured provider serves `model` and has not answered
    /// not-found for it. Only a stored pick or the chat model can be
    /// unreachable; its calls then fail naming why.
    pub(crate) reachable: bool,
    /// The models this row would run on ahead of `model`, plus `model`, whose
    /// provider answered not-found within the window (ADR 0403).
    pub(crate) not_served: Vec<String>,
    /// Whether this task sends images, so its model must read them. Settings
    /// then offers only models that do.
    pub(crate) needs_vision: bool,
    /// Whether `model` reads images. With `needs_vision`, a `false` here means
    /// the engine refuses the task rather than calling the model.
    pub(crate) vision: bool,
    /// The models Settings lists first, best first. The first a configured
    /// provider serves is the default.
    pub(crate) recommended: Vec<RecommendedSelection>,
}

/// One recommended model, and the tier its picker badges as recommended.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct RecommendedSelection {
    pub(crate) model: String,
    /// Set only where measurement backs the tier: the compactor's row.
    pub(crate) effort: Option<&'static str>,
}

/// `purpose`'s recommended models, each with its recommended tier.
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

/// Every background row, keyed by the model preference it reads.
async fn get_background_models(
    State(state): State<AppState>,
) -> Json<BTreeMap<&'static str, BackgroundModel>> {
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
                not_served: selection.not_served.clone(),
                needs_vision: selection.needs_vision,
                vision: selection.vision,
                recommended: recommended_selections(*purpose, state.engine.model_registry()),
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

    /// The compactor's row badges each default model at its default tier. No
    /// other row badges anything, since no benchmark backs its tier.
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
