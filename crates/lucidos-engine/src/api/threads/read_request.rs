//! The *read request* routes (ADR 0409).
//!
//! A coding agent asks through `lucidos request-read`, which posts to
//! `.../read-request`. The chat agent asks through the `request_read` tool, in
//! process, and both record the same event. The drawer posts to
//! `.../read-request/seen` once the user has seen the reply.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::read_request::{
    record_read_request, record_reply_seen, ReplySeen, READ_REQUESTED_ACK,
};

use super::actions::refuse_an_agent_from_another_thread;
use super::parse_thread_id;

/// The 403 for an agent naming another thread's id on the request route.
pub(super) const READ_REQUEST_IS_THE_CALLERS: &str = "A thread can only ask for its own reply \
    to be read, and the id in the path is not the calling thread. Drop the id: \
    `lucidos request-read` already acts on the thread you are running in.";

/// The 403 for an agent calling the seen route.
pub(super) const SEEN_IS_THE_USERS: &str = "Only the user can mark a reply as read, by \
    looking at it. An agent cannot clear a read request.";

/// POST /api/v1/threads/:thread_id/read-request: the calling thread's agent
/// asks the user to read its latest reply.
///
/// Acts on the calling thread only, as the tool does by taking no thread id.
/// The event records the real caller: a coding agent's token names its thread.
pub(in crate::api) async fn request_thread_read(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_uuid = parse_thread_id(&thread_id)?;
    refuse_an_agent_from_another_thread(&headers, thread_uuid, READ_REQUEST_IS_THE_CALLERS)?;
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None)
        .await
        .map_err(|e| (e.status, e.message))?;
    let known: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1)")
            .bind(thread_uuid)
            .fetch_one(&state.pool)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !known {
        return Err((
            StatusCode::NOT_FOUND,
            format!("No thread {thread_id} in this workspace."),
        ));
    }
    record_read_request(&state.engine.event_bus, thread_uuid, actor)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({
        "status": "requested",
        "message": READ_REQUESTED_ACK,
    })))
}

/// The body of a sighting: the thread's `summary_version` the client saw.
#[derive(serde::Deserialize)]
pub(in crate::api) struct ReplySeenBody {
    summary_version: i64,
}

/// POST /api/v1/threads/:thread_id/read-request/seen: the user saw the reply
/// a read request points at. `seen` says whether a request was pending. A
/// 409 means the thread changed since the client saw it, so it asks again.
///
/// A sighting is the user's alone, so an agent subprocess is refused: its
/// token would otherwise pass as a caller and clear any thread's request.
pub(in crate::api) async fn mark_thread_reply_seen(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ReplySeenBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if matches!(
        crate::api::actor::subprocess_origin(&headers),
        crate::api::actor::SubprocessOrigin::Subprocess { .. }
    ) {
        return Err(ApiError::new(StatusCode::FORBIDDEN, SEEN_IS_THE_USERS));
    }
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let thread_uuid =
        parse_thread_id(&thread_id).map_err(|(status, msg)| ApiError::new(status, msg))?;
    let outcome = record_reply_seen(
        &state.pool,
        &state.engine.event_bus,
        thread_uuid,
        body.summary_version,
        actor,
    )
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    match outcome {
        ReplySeen::Cleared => Ok(Json(serde_json::json!({ "seen": true }))),
        ReplySeen::NothingPending => Ok(Json(serde_json::json!({ "seen": false }))),
        ReplySeen::Stale => Err(ApiError::new(
            StatusCode::CONFLICT,
            "The thread changed after this reply was seen. Look again to clear its read request.",
        )),
        ReplySeen::UnknownThread => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("No thread {thread_id} in this workspace."),
        )),
    }
}
