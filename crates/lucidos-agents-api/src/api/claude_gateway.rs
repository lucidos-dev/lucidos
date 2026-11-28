use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use super::error::ApiError;
use super::AppState;
use crate::core::prefs;
use crate::engine::provider_host::{
    configured_origin, ClaudeGatewaySignInState, ClaudeGatewayStatus,
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/providers/claude-gateway", get(status))
        .route("/providers/claude-gateway/sign-in", post(sign_in))
        .route("/providers/claude-gateway/sign-out", post(sign_out))
}

async fn status(State(state): State<AppState>) -> Result<Json<ClaudeGatewayStatus>, ApiError> {
    state
        .engine
        .workspace_providers
        .claude_gateway_status(&state.pool)
        .await
        .map(Json)
        .map_err(ApiError::db)
}

#[derive(Deserialize)]
struct SignInRequest {
    #[serde(default)]
    url: Option<String>,
}

async fn sign_in(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SignInRequest>,
) -> Result<Json<ClaudeGatewaySignInState>, ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    if let Some(url) = request.url {
        super::settings::require_owner_for_human_only_preference(
            &headers,
            prefs::CLAUDE_GATEWAY_URL.key(),
        )?;
        state
            .engine
            .apply_preference_write(
                prefs::CLAUDE_GATEWAY_URL.key(),
                &url,
                None,
                Some(actor.clone()),
            )
            .await
            .map_err(ApiError::bad_request)?;
    }
    let origin = configured_origin(&state.pool)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    state
        .engine
        .workspace_providers
        .claude_gateway_sign_in(&state.pool, &state.engine.event_bus, origin, Some(actor))
        .await
        .map(Json)
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, e.to_string()))
}

async fn sign_out(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    let signed_out = state
        .engine
        .workspace_providers
        .claude_gateway_sign_out(&state.pool, &state.engine.event_bus, Some(actor))
        .await
        .map_err(ApiError::db)?;
    Ok(Json(serde_json::json!({ "signed_out": signed_out })))
}
