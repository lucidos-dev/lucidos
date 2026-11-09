//! Undo for a config change (ADR 0623).

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use uuid::Uuid;

use super::error::ApiError;
use super::AppState;
use crate::engine::config_change;

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/config-changes/:event_id/undo", post(undo))
}

/// POST /api/v1/config-changes/:event_id/undo: write the inverse of the config
/// change that event recorded, as the caller. 404 for an event that records
/// no config change. 409 when it cannot be undone, or the config has moved on.
async fn undo(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let change = config_change::stored_change(&state.pool, event_id)
        .await
        .map_err(ApiError::db)?
        .ok_or_else(|| ApiError::not_found("No config change has this event id."))?;
    let Some(undo) = change.undo else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "This change cannot be undone. Change it back by hand.",
        )
        .with_reason("not_undoable"));
    };
    let applied = config_change::apply_undo(&state.engine.undo_targets(), event_id, undo, actor)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if !applied {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "The config has changed since, so there is nothing to undo.",
        )
        .with_reason("moved_on"));
    }
    Ok(Json(serde_json::json!({ "undone": true })))
}
