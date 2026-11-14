//! Sign in with ChatGPT, for Settings and the `lucidos chatgpt-plan` CLI.
//!
//! - `GET /api/v1/providers/chatgpt-plan`: the stored sign-in, the attempt in
//!   progress and the usage-limit pause. Never a token.
//! - `POST /api/v1/providers/chatgpt-plan/sign-in`: start an attempt and answer
//!   with the ChatGPT page the browser must open. Refused for a browser that
//!   is not on a computer running Lucidos (ADR 0636).
//! - `POST /api/v1/providers/chatgpt-plan/sign-out`: stop any attempt in
//!   progress and forget the stored sign-in. `?forget_account=true` also
//!   forgets the registration, so the next sign-in may use another account.

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};

use super::error::ApiError;
use super::AppState;
use crate::core::oauth::CallbackRoute;
use crate::core::oauth_callback::{browser_for_device, BrowserContext};
use crate::engine::chatgpt_plan_sign_in::{ChatGptPlanStatus, StartedSignIn};
use crate::llm::chatgpt_plan::account::SignOutOutcome;
use crate::llm::chatgpt_plan::AuthServer;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/providers/chatgpt-plan", get(status))
        .route("/providers/chatgpt-plan/sign-in", post(sign_in))
        .route("/providers/chatgpt-plan/sign-out", post(sign_out))
}

async fn status(State(state): State<AppState>) -> Result<Json<ChatGptPlanStatus>, ApiError> {
    state
        .engine
        .chatgpt_plan_sign_in
        .status(&state.pool, &state.engine.chatgpt_plan_access.pause)
        .await
        .map(Json)
        .map_err(ApiError::db)
}

/// The browser the ChatGPT page opens in: the one that sent this request, else
/// the one its device last used. A terminal sends neither, and gets the local
/// default of [`CallbackRoute::for_browser`].
fn requesting_browser(headers: &HeaderMap) -> Option<BrowserContext> {
    BrowserContext::from_headers(headers).or_else(|| {
        headers
            .get(super::actor::HEADER_DEVICE_ID)
            .and_then(|v| v.to_str().ok())
            .and_then(browser_for_device)
    })
}

async fn sign_in(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<StartedSignIn>, ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    let route = CallbackRoute::for_browser(requesting_browser(&headers));
    state
        .engine
        .chatgpt_plan_sign_in
        .start(
            &state.pool,
            &state.engine.event_bus,
            AuthServer::openai(),
            &state.engine.workspace_name(),
            route,
            Some(actor),
        )
        .await
        .map(Json)
        .map_err(|e| ApiError::bad_request(e.to_string()))
}

#[derive(serde::Deserialize)]
struct SignOutParams {
    #[serde(default)]
    forget_account: bool,
}

async fn sign_out(
    State(state): State<AppState>,
    Query(params): Query<SignOutParams>,
    headers: HeaderMap,
) -> Result<Json<SignOutOutcome>, ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    state
        .engine
        .chatgpt_plan_sign_in
        .sign_out(
            &state.pool,
            &state.engine.event_bus,
            &AuthServer::openai(),
            params.forget_account,
            Some(actor),
        )
        .await
        .map(Json)
        .map_err(|e| ApiError::internal(e.to_string()))
}
