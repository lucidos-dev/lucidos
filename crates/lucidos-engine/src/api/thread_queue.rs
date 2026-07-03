//! HTTP surface for the *Thread Queue* panel: list the queue + capacity
//! policy, run a queued entry now, drop one, or update the policy. All
//! mutations route through the `ThreadQueue` manager, which emits the
//! corresponding `ThreadQueue*` / `CapacityPolicyChanged` events (the panel
//! refreshes off those over SSE).

use super::error::ApiError;
use super::*;

use crate::engine::thread_queue::{CapacityPolicy, ThreadQueueSnapshot};

#[derive(Deserialize)]
pub struct ThreadQueueEntryAction {
    pub entry_id: Uuid,
}

/// GET /api/v1/thread-queue — every live occupant of the shared pool plus the
/// active capacity policy. Background entries (queued + admitted, FIFO) come
/// from the projection; user-initiated entries (`kind: "user-chat"`) are
/// merged from the manager's in-memory state — they share the pool and the
/// Running/Queued view but are never persisted as rows. Delegates to
/// `ThreadQueue::snapshot`, the single materialization shared with the
/// `list_thread_queue` LLM tool so the two read paths can never diverge.
pub(super) async fn list_thread_queue(
    State(state): State<AppState>,
) -> Result<Json<ThreadQueueSnapshot>, ApiError> {
    let snapshot = state
        .engine
        .thread_queue
        .snapshot()
        .await
        .map_err(|e| ApiError::internal(format!("thread_queue query failed: {e}")))?;
    Ok(Json(snapshot))
}

/// POST /api/v1/thread-queue/run-now — force-admit a queued entry, ignoring
/// every cap. The resolved actor stamps the `ThreadQueueAdmitted` event.
pub(super) async fn run_thread_queue_entry_now(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ThreadQueueEntryAction>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = super::actor::user_actor_resolved(&headers, state.engine.pool(), None).await;
    state
        .engine
        .thread_queue
        .run_now(body.entry_id, actor)
        .await
        .map_err(ApiError::not_found)?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /api/v1/thread-queue/drop — drop a queued entry without running it.
pub(super) async fn drop_thread_queue_entry(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ThreadQueueEntryAction>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = super::actor::user_actor_resolved(&headers, state.engine.pool(), None).await;
    state
        .engine
        .thread_queue
        .drop_entry(body.entry_id, "dropped by user", actor)
        .await
        .map_err(ApiError::not_found)?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// PUT /api/v1/thread-queue/policy — replace the capacity policy. The body
/// may be partial (`#[serde(default)]` on `CapacityPolicy` fills the rest
/// with defaults); the full resulting policy is returned. Persisted via
/// `CapacityPolicyChanged`, so it survives restarts.
///
/// Concurrency caps of 0 are legal and mean "hold" — `max_concurrent_total: 0`
/// pauses ALL background admission (the queue accumulates until the cap is
/// raised). Only `max_queued_per_trigger` must be ≥ 1: at 0 every trigger
/// fire would instantly overflow, which under `drop-oldest` (nothing older
/// to drop) silently degrades to an unbounded queue.
pub(super) async fn update_capacity_policy(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(policy): Json<CapacityPolicy>,
) -> Result<Json<CapacityPolicy>, ApiError> {
    if policy.max_queued_per_trigger == 0 {
        return Err(ApiError::bad_request(
            "max_queued_per_trigger must be at least 1",
        ));
    }
    let actor = super::actor::user_actor_resolved(&headers, state.engine.pool(), None).await;
    state
        .engine
        .thread_queue
        .set_policy(policy.clone(), actor)
        .await
        .map_err(|e| ApiError::internal(format!("failed to persist capacity policy: {e}")))?;
    Ok(Json(policy))
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/thread-queue", get(list_thread_queue))
        .route("/thread-queue/run-now", post(run_thread_queue_entry_now))
        .route("/thread-queue/drop", post(drop_thread_queue_entry))
        .route("/thread-queue/policy", put(update_capacity_policy))
}
