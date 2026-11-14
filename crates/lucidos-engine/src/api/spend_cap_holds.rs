//! Reload-safe provider hold status and an authenticated manual probe.
use super::{error::ApiError, AppState};
use crate::engine::model_call::spend_cap;
use crate::llm::{spend_cap::SpendCapHold, ProviderKind};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};

async fn status(State(state): State<AppState>) -> Json<Vec<SpendCapHold>> {
    Json(spend_cap::status(&state.engine.event_bus).await)
}

async fn clear(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    let provider = ProviderKind::ALL
        .into_iter()
        .find(|kind| kind.as_str() == provider)
        .ok_or_else(|| ApiError::with_code(400, "Unknown provider"))?;
    spend_cap::clear(&state.engine.event_bus, provider, actor).await;
    Ok(Json(serde_json::json!({"success": true})))
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/models/spend-cap-holds", get(status))
        .route("/models/spend-cap-holds/:provider/clear", post(clear))
}
