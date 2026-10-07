//! `/api/v1/recall/*`: the Tree memory module's recall tools as routes (ADR
//! 0362). The `recall` LLM tool and the generated `lucidos recall` commands
//! reach the same engine methods. A route answers in any workspace, from
//! whatever summary trees are built.

use super::*;

use crate::api::error::ApiError;
use crate::engine::summary_tree::view::NodeId;

#[derive(Deserialize)]
pub(super) struct RecallIdQuery {
    id: String,
    n: Option<u32>,
    thread: Option<Uuid>,
}

#[derive(Deserialize)]
pub(super) struct RecallFindQuery {
    query: String,
    limit: Option<usize>,
    thread: Option<Uuid>,
}

#[derive(Deserialize)]
pub(super) struct RecallSearchQuery {
    text: String,
    limit: Option<usize>,
}

const DEFAULT_RESULTS: usize = 10;

fn node_id(query: &RecallIdQuery) -> Result<NodeId, ApiError> {
    NodeId::parse(&query.id, query.thread).map_err(ApiError::bad_request)
}

pub(super) async fn recall_zoom(
    State(state): State<AppState>,
    Query(query): Query<RecallIdQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = node_id(&query)?;
    state
        .engine
        .recall_zoom(id, query.n.unwrap_or(1))
        .await
        .map(Json)
        .map_err(|e| ApiError::bad_request(e.to_string()))
}

pub(super) async fn recall_date(
    State(state): State<AppState>,
    Query(query): Query<RecallIdQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = node_id(&query)?;
    state
        .engine
        .recall_date(id)
        .await
        .map(Json)
        .map_err(|e| ApiError::bad_request(e.to_string()))
}

pub(super) async fn recall_search(
    State(state): State<AppState>,
    Query(query): Query<RecallSearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let hits = state
        .engine
        .recall_search(&query.text, query.limit.unwrap_or(DEFAULT_RESULTS))
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({ "results": hits })))
}

pub(super) async fn recall_find(
    State(state): State<AppState>,
    Query(query): Query<RecallFindQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let hits = state
        .engine
        .recall_find(
            &query.query,
            query.limit.unwrap_or(DEFAULT_RESULTS),
            query.thread,
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "results": hits })))
}

/// Routes for the `/recall/*` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/recall/zoom", get(recall_zoom))
        .route("/recall/find", get(recall_find))
        .route("/recall/search", get(recall_search))
        .route("/recall/date", get(recall_date))
}
