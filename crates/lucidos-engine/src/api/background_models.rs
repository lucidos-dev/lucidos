//! `GET /api/v1/models/background`: the model each background task runs on,
//! so Settings shows what will run rather than a catalog default.

use std::collections::BTreeMap;

use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;

use super::AppState;
use crate::engine::aux_purpose::{model_key, recommended, ModelSource, BACKGROUND_ROWS};

/// One background row's resolved *model selection*.
#[derive(Debug, Serialize)]
pub(crate) struct BackgroundModel {
    pub(crate) model: String,
    pub(crate) effort: Option<String>,
    pub(crate) source: ModelSource,
    /// Whether a configured provider serves `model`. Only a stored pick can be
    /// unreachable; its calls then fail naming what to configure.
    pub(crate) reachable: bool,
    /// Whether this task sends images, so its model must read them. Settings
    /// then offers only models that do.
    pub(crate) needs_vision: bool,
    /// Whether `model` reads images. With `needs_vision`, a `false` here means
    /// the engine refuses the task rather than calling the model.
    pub(crate) vision: bool,
    /// The models Settings lists first, best first. The first a configured
    /// provider serves is the default.
    pub(crate) recommended: Vec<String>,
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
                needs_vision: selection.needs_vision,
                vision: selection.vision,
                recommended: recommended(*purpose, state.engine.model_registry()),
            },
        );
    }
    Json(rows)
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/models/background", get(get_background_models))
}
