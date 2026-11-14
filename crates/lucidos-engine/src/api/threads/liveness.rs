//! `GET /api/v1/threads/:thread_id/liveness`: the thread's *thread liveness*,
//! the object every thread-scoped agent read carries (ADR 0679).

use axum::{
    extract::{Path, State},
    Json,
};
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::thread_liveness::{liveness_for_thread, ThreadLiveness};

pub(in crate::api) async fn get_liveness(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<Json<ThreadLiveness>, ApiError> {
    let thread_id = Uuid::parse_str(&thread_id)
        .map_err(|e| ApiError::bad_request(format!("Invalid thread id: {e}")))?;
    liveness_for_thread(&state.pool, thread_id)
        .await
        .map_err(ApiError::db)?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("Thread {thread_id} does not exist")))
}
