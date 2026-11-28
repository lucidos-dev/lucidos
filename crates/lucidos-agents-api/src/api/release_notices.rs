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

/// INVARIANT: The release this engine reports, which decides what a caller may see or
/// answer.
///
/// Read here rather than taken from the request. A client could otherwise name
/// a release it is not on, and be handed a notice that has not shipped. The
/// rule itself is `release_notices::running_release`, shared with the startup
/// stamp so the two cannot disagree about which notices exist.
fn running_release() -> Result<semver::Version, ApiError> {
    release_notices::running_release().map_err(ApiError::internal)
}

async fn notice_facts(state: &AppState) -> release_notices::NoticeFacts {
    let unapproved_mcp_server = match state.engine.mcp_manager.list_servers().await {
        Ok(servers) => servers
            .iter()
            .any(|server| server.dispatchable && !server.launch_approved),
        Err(e) => {
            log_legacy!("[ReleaseNotices] Failed to list MCP servers: {}", e);
            true
        }
    };
    let judge_on = match COMMAND_GUARD_JUDGE.try_read(&state.pool).await {
        Ok(on) => on,
        Err(e) => {
            log_legacy!("[ReleaseNotices] Failed to read the judge switch: {}", e);
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

async fn list(
    State(state): State<AppState>,
) -> Result<Json<release_notices::NoticeView>, ApiError> {
    read_notices(&state).await.map(Json)
}

pub(super) async fn read_notices(
    state: &AppState,
) -> Result<release_notices::NoticeView, ApiError> {
    let running = running_release()?;
    let facts = notice_facts(state).await;
    let cursor = release_notices::stored_cursor(&state.pool).await;
    Ok(release_notices::view(
        release_notices::all(),
        &running,
        &facts,
        cursor.as_deref(),
    ))
}

#[derive(Deserialize)]
struct ResolveRequest {
    id: String,
    shown_ids: Option<Vec<String>>,
}

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
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let facts = notice_facts(&state).await;
    let moved = release_notices::resolve(
        &state.pool,
        notices,
        &running,
        &facts,
        &body.id,
        body.shown_ids.as_deref(),
    )
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
