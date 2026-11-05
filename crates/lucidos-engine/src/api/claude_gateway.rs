//! The Claude gateway sign-in, for Settings and the `lucidos claude-gateway`
//! CLI. The gateway URL itself is the `claude_gateway_url` preference.
//!
//! - `GET /api/v1/providers/claude-gateway`: the URL, the stored sign-in and
//!   the sign-in in progress. Never a token.
//! - `POST /api/v1/providers/claude-gateway/sign-in`: start a device sign-in
//!   and answer with the code the user enters on the gateway's page. A `url`
//!   in the body is stored as the preference first.
//! - `POST /api/v1/providers/claude-gateway/sign-out`: stop any sign-in in
//!   progress and forget the stored one.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use super::error::ApiError;
use super::AppState;
use crate::core::prefs;
use crate::engine::claude_gateway_sign_in::{configured_origin, ClaudeGatewayStatus, SignInState};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/providers/claude-gateway", get(status))
        .route("/providers/claude-gateway/sign-in", post(sign_in))
        .route("/providers/claude-gateway/sign-out", post(sign_out))
}

async fn status(State(state): State<AppState>) -> Result<Json<ClaudeGatewayStatus>, ApiError> {
    state
        .engine
        .claude_gateway_sign_in
        .status(&state.pool)
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
) -> Result<Json<SignInState>, ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    if let Some(url) = request.url {
        super::settings::refuse_agent_human_only_preference(
            &headers,
            prefs::CLAUDE_GATEWAY_URL.key(),
        )?;
        // The shared preference write, so its check and the sign-out of an
        // old gateway apply exactly as they do to a Settings save.
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
        .claude_gateway_sign_in
        .start(&state.pool, &state.engine.event_bus, origin, Some(actor))
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
        .claude_gateway_sign_in
        .sign_out(&state.pool, &state.engine.event_bus, Some(actor))
        .await
        .map_err(ApiError::db)?;
    Ok(Json(serde_json::json!({ "signed_out": signed_out })))
}
