//! `POST /api/v1/threads/:thread_id/cache-keep-alive`: the user turns the
//! cache keep-alive off for this thread, `{"state": "off"}`, or back on,
//! `{"state": "on"}` (ADR 0554). It records the event under the caller's
//! device, and answers 204.

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::event_wait::keep_alive::KeepAliveSwitch;
use crate::engine::thread_events::EventMeta;

#[derive(serde::Deserialize)]
pub(in crate::api) struct KeepAliveSwitchBody {
    state: KeepAliveSwitch,
}

pub(in crate::api) async fn switch_cache_keep_alive(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<KeepAliveSwitchBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let thread_id = Uuid::parse_str(&thread_id)
        .map_err(|e| ApiError::bad_request(format!("Invalid thread id: {e}")))?;
    let Json(body) = body.map_err(|e| {
        ApiError::bad_request(format!(
            "The body must be {{\"state\": \"off\"}} or {{\"state\": \"on\"}}: {e}"
        ))
    })?;
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1)")
            .bind(thread_id)
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::db)?;
    if !exists {
        return Err(ApiError::not_found(format!(
            "Thread {thread_id} does not exist"
        )));
    }
    state
        .engine
        .switch_cache_keep_alive(thread_id, body.state, EventMeta::with_actor(Some(actor)))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
