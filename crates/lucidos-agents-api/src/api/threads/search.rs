use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::api::AppState;

#[derive(Deserialize)]
pub struct ThreadSearchQuery {
    pub q: String,
    pub limit: Option<i64>,
}

pub(in crate::api) async fn search_threads(
    State(state): State<AppState>,
    Query(query): Query<ThreadSearchQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let limit = query.limit.unwrap_or(20).clamp(1, 50);
    let mut merged =
        crate::engine::thread_search::combined_thread_search(&state.engine, &query.q, limit)
            .await
            .map_err(|e| {
                log_legacy!("[API] Thread search failed: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Search failed: {}", e),
                )
            })?;
    // INVARIANT: `limit` bounds each ARM inside the merge, not the merge itself, so two
    // arms that agree on nothing return up to twice it. This route states a
    // maximum, so it applies one, after the sort rather than before it: cutting
    // earlier would keep an arbitrary half instead of the best-scoring one.
    merged.truncate(limit.max(0) as usize);
    crate::core::store::attach_reader_fields(
        merged.iter_mut().map(|r| &mut r.info),
        &state.engine.workspace_name(),
    );
    crate::engine::thread_liveness::attach(
        state.engine.pool(),
        merged.iter_mut().map(|r| &mut r.info),
    )
    .await;
    Ok(Json(serde_json::json!({ "results": merged })))
}
