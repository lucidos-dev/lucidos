//! `GET /api/v1/threads/:thread_id/cache-warmth`: how long the thread's prompt
//! cache stays warm. `remaining_secs` is null when it is cold or the thread
//! has made no main-session call.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Serialize;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::switch_estimate::cache_remaining_secs;

#[derive(Debug, Serialize)]
pub(in crate::api) struct CacheWarmth {
    remaining_secs: Option<i64>,
}

pub(in crate::api) async fn get_cache_warmth(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<Json<CacheWarmth>, ApiError> {
    let thread_id = Uuid::parse_str(&thread_id)
        .map_err(|e| ApiError::bad_request(format!("Invalid thread id: {e}")))?;
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
    let remaining_secs = cache_remaining_secs(&state.pool, thread_id)
        .await
        .map_err(ApiError::db)?;
    Ok(Json(CacheWarmth { remaining_secs }))
}
