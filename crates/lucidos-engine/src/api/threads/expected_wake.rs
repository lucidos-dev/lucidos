//! The *expected wake* route (ADR 0603).
//!
//! A coding agent states it through `lucidos expect-wake <duration> --reason
//! <text>`, which posts here. The chat agent states it through the
//! `expect_wake` tool, in process, and both record the same `WakeExpected`.

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::api::AppState;
use crate::engine::expected_wake::{record_expected_wake, ExpectedWake};

use super::actions::refuse_an_agent_from_another_thread;
use super::parse_thread_id;

/// The 403 for an agent naming another thread's id.
pub(super) const EXPECTED_WAKE_IS_THE_CALLERS: &str = "A thread can only state its own \
    expected wake, and the id in the path is not the calling thread. Drop the id: \
    `lucidos expect-wake` already acts on the thread you are running in.";

/// The body of an expected wake. Both fields are required.
#[derive(serde::Deserialize)]
pub(in crate::api) struct ExpectedWakeBody {
    in_secs: f64,
    reason: String,
}

/// POST /api/v1/threads/:thread_id/expected-wake: the calling thread's agent
/// states when it expects to wake, `{"in_secs": 2700, "reason": "..."}`.
///
/// Acts on the calling thread only, as the tool does by taking no thread id.
/// The event records the real caller: a coding agent's token names its thread.
pub(in crate::api) async fn record_thread_expected_wake(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<ExpectedWakeBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_uuid = parse_thread_id(&thread_id)?;
    refuse_an_agent_from_another_thread(&headers, thread_uuid, EXPECTED_WAKE_IS_THE_CALLERS)?;
    let Json(body) = body.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!(
                "The body must be {{\"in_secs\": <seconds>, \"reason\": \"<what you wait \
                 for>\"}}. {e}"
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
    let now = chrono::Utc::now();
    let wake = ExpectedWake::stated(body.in_secs, &body.reason, now)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let ack = wake.ack(now);
    record_expected_wake(&state.engine.event_bus, thread_uuid, wake, Some(actor))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(
        serde_json::json!({ "status": "recorded", "message": ack }),
    ))
}
