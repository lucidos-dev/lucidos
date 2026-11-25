//! The *read decision* and *read request* routes (ADR 0409, ADR 0417).
//!
//! A coding agent decides through `lucidos request-read yes|no`, which posts
//! to `.../read-request`. The chat agent decides through the `request_read`
//! tool, in process, and both record the same events. The drawer posts to
//! `.../read-request/seen` once the user has seen the reply.

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::read_request::{
    record_read_decision, record_reply_seen, ReadDecision, ReplySeen, READ_ARG,
};

use super::actions::refuse_an_agent_from_another_thread;
use super::parse_thread_id;

/// The 403 for an agent naming another thread's id on the request route.
pub(super) const READ_REQUEST_IS_THE_CALLERS: &str = "A thread can only decide whether its \
    own reply is read, and the id in the path is not the calling thread. Drop the id: \
    `lucidos request-read` already acts on the thread you are running in.";

/// The 403 for an agent calling the seen route.
pub(super) const SEEN_IS_THE_USERS: &str = "Only the user can mark a reply as read, by \
    looking at it. An agent cannot clear a read request.";

/// The body of a read decision. `read` is required: there is no default.
#[derive(serde::Deserialize)]
pub(in crate::api) struct ReadDecisionBody {
    read: bool,
}

/// POST /api/v1/threads/:thread_id/read-request: the calling thread's agent
/// records its read decision on its latest reply, `{"read": true|false}`.
///
/// Acts on the calling thread only, as the tool does by taking no thread id.
/// The guard runs before the body is read, so a decision of either kind for
/// another thread is refused. The event records the real caller: a coding
/// agent's token names its thread.
pub(in crate::api) async fn request_thread_read(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<ReadDecisionBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_uuid = parse_thread_id(&thread_id)?;
    refuse_an_agent_from_another_thread(&headers, thread_uuid, READ_REQUEST_IS_THE_CALLERS)?;
    let Json(body) = body.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!(
                "The body must be {{\"{READ_ARG}\": true}} or {{\"{READ_ARG}\": false}}: \
                 every turn decides whether its reply is worth reading. {e}"
            ),
        )
    })?;
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
    let decision = ReadDecision::from_read(body.read);
    record_read_decision(&state.engine.event_bus, thread_uuid, decision, Some(actor))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({
        "status": match decision {
            ReadDecision::Requested => "requested",
            ReadDecision::NotRequested => "not-requested",
        },
        "message": decision.ack(),
    })))
}

/// The body of a sighting: the thread's `summary_version` the client saw.
#[derive(serde::Deserialize)]
pub(in crate::api) struct ReplySeenBody {
    summary_version: i64,
}

pub(in crate::api) async fn mark_thread_reply_seen(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ReplySeenBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    refuse_an_agent_sighting(&headers)?;
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let thread_uuid =
        parse_thread_id(&thread_id).map_err(|(status, msg)| ApiError::new(status, msg))?;
    let outcome = record_reply_seen(
        &state.pool,
        &state.engine.event_bus,
        thread_uuid,
        body.summary_version,
        actor.clone(),
        &crate::gateway_users::Reader::of(Some(&actor), &crate::gateway_users::current()),
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

fn refuse_an_agent_sighting(headers: &HeaderMap) -> Result<(), ApiError> {
    if matches!(
        crate::core::subprocess_origin::subprocess_origin(headers),
        crate::core::subprocess_origin::SubprocessOrigin::Subprocess { .. }
    ) {
        return Err(ApiError::new(StatusCode::FORBIDDEN, SEEN_IS_THE_USERS));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sighting_is_refused_to_an_agent_subprocess() {
        crate::core::subprocess_origin::init_agent_origin_secret(
            "subprocess-test-secret".to_string(),
        );
        let token = crate::core::subprocess_origin::mint_agent_origin_token(None, 0, None)
            .expect("secret installed");
        let mut agent = HeaderMap::new();
        agent.insert(
            crate::core::subprocess_origin::HEADER_AGENT_ORIGIN_TOKEN,
            token.parse().unwrap(),
        );
        let refusal =
            refuse_an_agent_sighting(&agent).expect_err("an agent cannot clear a request");
        assert_eq!(refusal.status, StatusCode::FORBIDDEN);
        assert_eq!(refusal.message, SEEN_IS_THE_USERS);
        assert!(refuse_an_agent_sighting(&HeaderMap::new()).is_ok());
    }
}
