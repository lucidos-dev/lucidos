//! The HTTP face of thread search. The merge, scoring and dampening live on the
//! engine (`engine::thread_search`), because the agent's `threads` tool reaches
//! the same capability without going through a route.

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
    /// Max threads to return. Absent keeps the 20 the UI has always used, so
    /// the search box is unaffected by the parameter existing.
    pub limit: Option<i64>,
}

/// GET /api/v1/threads/search?q=<query>&limit=<n>: search threads by
/// title/content (text + semantic).
pub(in crate::api) async fn search_threads(
    State(state): State<AppState>,
    Query(query): Query<ThreadSearchQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let limit = query.limit.unwrap_or(20).clamp(1, 50);
    let mut merged =
        crate::engine::thread_search::combined_thread_search(&state.engine, &query.q, limit)
            .await
            .map_err(|e| {
                log!("[API] Thread search failed: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Search failed: {}", e),
                )
            })?;
    // `limit` bounds each ARM inside the merge, not the merge itself, so two
    // arms that agree on nothing return up to twice it. This route states a
    // maximum, so it applies one, after the sort rather than before it: cutting
    // earlier would keep an arbitrary half instead of the best-scoring one.
    merged.truncate(limit.max(0) as usize);
    Ok(Json(serde_json::json!({ "results": merged })))
}
