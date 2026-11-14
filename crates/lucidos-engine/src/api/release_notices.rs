//! HTTP for *release notices*: what this workspace still owes the reader, and
//! the answer that settles one.
//!
//! Two endpoints, one list. The modal draws the notice named by `next_id`, and
//! the What's New panel draws them all. Both read this one response, so they
//! cannot disagree about what is answered.
//!
//! - `GET  /api/v1/release-notices`
//! - `POST /api/v1/release-notices/resolve` with body `{ "id": "<notice id>" }`
//!
//! The rules live in `engine::release_notices`. This module is the boundary.

use axum::extract::State;
use axum::{
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use super::error::ApiError;
use super::AppState;
use crate::core::prefs::COMMAND_GUARD_JUDGE;
use crate::engine::aux_purpose::unresolved_judge;
use crate::engine::event_bus::SystemEvent;
use crate::engine::release_notices;
use crate::engine::ContextPurpose;

/// The release this engine reports, which decides what a caller may see or
/// answer.
///
/// Read here rather than taken from the request. A client could otherwise name
/// a release it is not on, and be handed a notice that has not shipped. The
/// rule itself is `release_notices::running_release`, shared with the startup
/// stamp so the two cannot disagree about which notices exist.
fn running_release() -> Result<semver::Version, ApiError> {
    release_notices::running_release().map_err(ApiError::internal)
}

/// The workspace facts the notices read. The command guard's exposure needs
/// the MCP manager and the judge's model selection, which only the running
/// engine holds.
async fn notice_facts(state: &AppState) -> release_notices::NoticeFacts {
    let unapproved_mcp_server = match state.engine.mcp_manager.list_servers().await {
        Ok(servers) => servers
            .iter()
            .any(|server| server.dispatchable && !server.launch_approved),
        Err(e) => {
            log!("[ReleaseNotices] Failed to list MCP servers: {}", e);
            true
        }
    };
    // A judge switched off needs no model, so nothing refuses a turn.
    let judge_on = match COMMAND_GUARD_JUDGE.try_read(&state.pool).await {
        Ok(on) => on,
        Err(e) => {
            log!("[ReleaseNotices] Failed to read the judge switch: {}", e);
            true
        }
    };
    let judge_unresolved = judge_on && {
        let call = state.engine.aux_call(ContextPurpose::CommandJudge).await;
        unresolved_judge(&state.pool, call.selection(), state.engine.model_registry())
            .await
            .is_some()
    };
    let exposure = release_notices::GuardExposure {
        unapproved_mcp_server,
        judge_unresolved,
    };
    release_notices::NoticeFacts::read(
        &state.pool,
        &state.workspace_path,
        &state.engine.trigger_configs,
        exposure,
    )
    .await
}

/// `GET /api/v1/release-notices`.
async fn list(
    State(state): State<AppState>,
) -> Result<Json<release_notices::NoticeView>, ApiError> {
    let running = running_release()?;
    let facts = notice_facts(&state).await;
    let cursor = release_notices::stored_cursor(&state.pool).await;
    Ok(Json(release_notices::view(
        release_notices::all(),
        &running,
        &facts,
        cursor.as_deref(),
    )))
}

#[derive(Deserialize)]
struct ResolveRequest {
    id: String,
}

/// `POST /api/v1/release-notices/resolve`.
///
/// Answering a notice the workspace has already walked past is a no-op rather
/// than an error. Two devices showing the same modal is ordinary, so the second
/// answer changes nothing and announces nothing.
async fn resolve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ResolveRequest>,
) -> Result<Json<release_notices::NoticeView>, ApiError> {
    let running = running_release()?;
    let notices = release_notices::all();
    if !notices.iter().any(|n| n.id == body.id) {
        return Err(ApiError::not_found(format!(
            "no release notice with id {:?} in this build",
            body.id
        )));
    }
    // Before the resolve, which walks the workspace past the notice.
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let facts = notice_facts(&state).await;
    let moved = release_notices::resolve(&state.pool, notices, &running, &facts, &body.id)
        .await
        .map_err(ApiError::db)?;
    if moved {
        state
            .engine
            .event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(SystemEvent::ReleaseNoticeResolved {
                    notice_id: body.id.clone(),
                    actor: Some(actor),
                }),
                "[ReleaseNotices] ReleaseNoticeResolved",
            )
            .await;
    }
    list(State(state)).await
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/release-notices", get(list))
        .route("/release-notices/resolve", post(resolve))
}
