use crate::engine::chat::agent_archive::AgentArchiveExt;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::api::AppState;
use crate::core::subprocess_origin::SubprocessOrigin;
use crate::engine::archive_family::{
    archive_family, ArchiveOutcome, ArchiveRejection, PinnedMembers,
};
use crate::engine::chat::agent_archive::{AgentArchiveAck, AgentArchiveError};

use super::extract_thread_uuid;

fn reach_rejection(
    e: crate::api::thread_reach::ThreadReachError,
) -> (StatusCode, axum::Json<serde_json::Value>) {
    (
        e.status_code(),
        axum::Json(serde_json::json!({
            "reason": e.reason(),
            "message": e.to_string(),
        })),
    )
}

pub(in crate::api) async fn archive_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<axum::Json<serde_json::Value>, ArchiveRejection> {
    let thread_uuid = extract_thread_uuid(&request).map_err(|(s, m)| {
        (
            s,
            axum::Json(serde_json::json!({ "reason": "bad_request", "message": m })),
        )
    })?;
    crate::api::thread_reach::refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        Some(thread_uuid),
        crate::engine::permission::Action::Archive,
    )
    .await
    .map_err(reach_rejection)?;
    let actor = crate::api::actor::user_actor(&headers, None);
    archive_family(&state.engine, thread_uuid, actor, PinnedMembers::ByActor)
        .await
        .map(ArchiveOutcome::into_body)
}

pub(in crate::api) async fn archive_thread_as_caller(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<axum::Json<serde_json::Value>, ArchiveRejection> {
    let caller = match crate::core::subprocess_origin::subprocess_origin(&headers) {
        SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } => Some(source_thread_id),
        SubprocessOrigin::NotSubprocess => None,
    };
    let target = crate::engine::thread_events::resolve_thread_id_arg(&thread_id, caller.flatten())
        .map_err(|m| {
            (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({ "reason": "bad_request", "message": m })),
            )
        })?;
    crate::api::thread_reach::refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        Some(target),
        crate::engine::permission::Action::Archive,
    )
    .await
    .map_err(reach_rejection)?;
    let actor = crate::api::actor::user_actor(&headers, None);
    match caller {
        Some(caller_thread) => match state
            .engine
            .archive_as_agent(caller_thread, target, actor)
            .await
        {
            Ok(ack) => Ok(agent_ack_body(ack)),
            Err(AgentArchiveError::Refused(rejection)) => Err(rejection),
            Err(e) => Err((
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::FORBIDDEN),
                axum::Json(serde_json::json!({ "reason": e.reason(), "message": e.to_string() })),
            )),
        },
        None => archive_family(&state.engine, target, actor, PinnedMembers::ByActor)
            .await
            .map(ArchiveOutcome::into_body),
    }
}

fn agent_ack_body(ack: AgentArchiveAck) -> axum::Json<serde_json::Value> {
    match ack {
        AgentArchiveAck::Archived(outcome) => outcome.into_body(),
        AgentArchiveAck::Requested { thread_id } => axum::Json(serde_json::json!({
            "requested": thread_id,
            "detail": "This thread is archived once its turn ends and it has settled.",
        })),
    }
}
