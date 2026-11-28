use super::*;

use crate::core::ArtifactManager;

#[derive(Deserialize)]
pub(super) struct MemoryEntriesQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    source_type: Option<String>,
    sort: Option<String>,
    importance: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct MemorySearchQuery {
    q: String,
    limit: Option<i64>,
}

#[derive(Deserialize)]
pub(super) struct MemorySourceQuery {
    source_id: Option<String>,
    source_type: Option<String>,
    path: Option<String>,
    commit: Option<String>,
}

fn page_bounds(limit: Option<i64>, offset: Option<i64>) -> (i64, i64) {
    (
        limit.unwrap_or(50).clamp(1, 200),
        offset.unwrap_or(0).max(0),
    )
}

pub(super) async fn get_memory_stats(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let index = state.memory_index.as_ref().ok_or((
        StatusCode::NOT_FOUND,
        "Memory index not available".to_string(),
    ))?;
    let stats = index.stats().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get memory stats: {}", e),
        )
    })?;
    let value = serde_json::to_value(stats).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize memory stats: {}", e),
        )
    })?;
    Ok(Json(value))
}

pub(super) async fn get_memory_entries(
    State(state): State<AppState>,
    Query(query): Query<MemoryEntriesQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let index = state.memory_index.as_ref().ok_or((
        StatusCode::NOT_FOUND,
        "Memory index not available".to_string(),
    ))?;
    let (limit, offset) = page_bounds(query.limit, query.offset);
    let source_type = query.source_type.as_deref();
    let sort = query.sort.as_deref();
    let importance_levels: Option<Vec<&str>> = query
        .importance
        .as_deref()
        .map(|s| s.split(',').filter(|l| !l.is_empty()).collect());

    let (entries, total) = index
        .paginated_entries(
            limit,
            offset,
            source_type,
            sort,
            importance_levels.as_deref(),
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get memory entries: {}", e),
            )
        })?;

    let has_more = offset.saturating_add(limit) < total;
    Ok(Json(serde_json::json!({
        "entries": entries,
        "total": total,
        "has_more": has_more,
    })))
}

pub(super) async fn search_memory(
    State(state): State<AppState>,
    Query(query): Query<MemorySearchQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state
        .engine
        .search_memory_ranked(&query.q, query.limit)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))
}

pub(super) async fn get_memory_source(
    State(state): State<AppState>,
    Query(query): Query<MemorySourceQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let index = state.memory_index.as_ref().ok_or((
        StatusCode::NOT_FOUND,
        "Memory index not available".to_string(),
    ))?;

    let source_type = query.source_type.as_deref().unwrap_or("event");

    match source_type {
        "event" => {
            let id = Uuid::from_str(query.source_id.as_deref().ok_or((
                StatusCode::BAD_REQUEST,
                "source_id required for event source".to_string(),
            ))?)
            .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid UUID".to_string()))?;
            state
                .engine
                .memory_source(id)
                .await
                .map(Json)
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))
        }
        "artifact" => {
            let path = query.path.as_deref().ok_or((
                StatusCode::BAD_REQUEST,
                "path required for artifact source".to_string(),
            ))?;
            let commit = query.commit.as_deref().ok_or((
                StatusCode::BAD_REQUEST,
                "commit required for artifact source".to_string(),
            ))?;

            let artifact_manager =
                ArtifactManager::new(state.workspace_path.clone()).map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to access artifacts: {}", e),
                    )
                })?;
            let content = artifact_manager
                .read_artifact_at_commit_string(path, commit)
                .map_err(|e| (StatusCode::NOT_FOUND, format!("Artifact not found: {}", e)))?;

            let source_json =
                serde_json::json!({"type": "artifact", "path": path, "commit": commit});
            let entries = index.entries_for_source(&source_json).await.map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to get entries: {}", e),
                )
            })?;

            Ok(Json(serde_json::json!({
                "source_type": "artifact",
                "artifact": {
                    "path": path,
                    "commit": commit,
                    "content": content,
                },
                "entries": entries,
            })))
        }
        _ => Err((
            StatusCode::BAD_REQUEST,
            format!("Invalid source_type: {}", source_type),
        )),
    }
}

pub(super) async fn rebuild_memory(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> (StatusCode, Json<serde_json::Value>) {
    if state.engine.is_rebuilding_memory() {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "status": "already_running" })),
        );
    }
    if !state.engine.embedder().is_ready() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "embedder_not_ready",
                "error": "The embedding model is still loading — memory rebuild is unavailable until it finishes (running it now would clear memory it can't re-index). Try again once memory is active."
            })),
        );
    }
    let force = params.get("force").map(|v| v == "true").unwrap_or(false);
    let re_extract_stale = params
        .get("re_extract_stale")
        .map(|v| v == "true")
        .unwrap_or(false);
    let engine = state.engine.clone();
    let bus = state.engine.event_bus.clone();

    tokio::spawn(async move {
        engine
            .rebuild_memory(force, re_extract_stale, Some(bus))
            .await;
    });
    let mode = if force {
        "full_rebuild"
    } else if re_extract_stale {
        "incremental_re_extract_stale"
    } else {
        "incremental"
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({ "status": "started", "mode": mode })),
    )
}

pub(super) async fn cancel_rebuild_memory(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !state.engine.is_rebuilding_memory() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "not_running" })),
        );
    }
    state.engine.cancel_memory_rebuild();
    (
        StatusCode::OK,
        Json(serde_json::json!({ "status": "cancelling" })),
    )
}

/// INVARIANT: Snapshot of where the background embedding-model loader has got to.
///
/// The live signal is the `EmbeddingModelStatusChanged` SSE frame, but a client
/// that loads DURING a cold first-run download (the normal case on a fresh
/// workspace, where the download starts at engine boot and the app connects
/// seconds later) has missed every frame so far. This is how it catches up, and
/// it serves the identical shape.
pub(super) async fn get_embedding_model_status(
    State(state): State<AppState>,
) -> Json<crate::memory::EmbeddingModelStatus> {
    Json(read_embedding_model_status(&state))
}

pub(super) fn read_embedding_model_status(state: &AppState) -> crate::memory::EmbeddingModelStatus {
    state.embedder.status()
}

pub(super) async fn get_tree_backfill(
    State(state): State<AppState>,
) -> Result<Json<crate::engine::summary_tree::TreeBackfill>, ApiError> {
    crate::engine::summary_tree::tree_backfill(&state.engine)
        .await
        .map(Json)
        .map_err(|e| ApiError::internal(format!("Could not read the Tree backfill: {e}")))
}

pub(super) async fn get_tree_backfill_estimate(
    State(state): State<AppState>,
) -> Result<Json<crate::engine::summary_tree::TreeBackfillEstimate>, ApiError> {
    crate::engine::summary_tree::tree_backfill_estimate(&state.engine)
        .await
        .map(Json)
        .map_err(ApiError::db)
}

#[derive(Deserialize)]
pub(super) struct RaiseTreeBackfillCap {
    from_cap_usd: f64,
}

/// INVARIANT: `POST /api/v1/memory/tree-backfill/cap`: Continue a backfill held at its
/// spending cap. The cap moves one step, which the engine sets, and the
/// compactor picks the raise up from its event. A press against a cap that
/// has moved since, a second click or another device, is refused.
pub(super) async fn raise_tree_backfill_cap(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RaiseTreeBackfillCap>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::engine::event_bus::{BusEvent, SystemEvent};
    let actor = super::actor::require_user_actor(&headers, state.engine.pool(), None).await?;
    let episode = crate::engine::summary_tree::spend::load(state.engine.pool())
        .await
        .map_err(ApiError::db)?
        .ok_or_else(|| ApiError::new(StatusCode::CONFLICT, "No Tree memory backfill is running"))?;
    // INVARIANT: The compactor's running total can read held a little before the events
    // do, so either counts: a refusal there would leave a held backfill stuck.
    let held = episode.held()
        || state
            .engine
            .summary_tree()
            .backfill_progress()
            .is_some_and(|p| p.held);
    if !held && episode.cap_usd.is_some() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "This Tree memory backfill has not reached its spending cap",
        )
        .with_reason("not-held"));
    }
    if episode
        .cap_usd
        .is_some_and(|cap| (cap - body.from_cap_usd).abs() > 1e-9)
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "The Tree memory spending cap already moved",
        )
        .with_reason("cap-moved"));
    }
    let cap_usd = episode.next_cap().ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "This Tree memory backfill has no spending cap: its model is not priced",
        )
    })?;
    state
        .engine
        .event_bus
        .emit(BusEvent::System(SystemEvent::TreeBackfillCapRaised {
            cap_usd,
            actor: Some(actor),
        }))
        .await
        .map_err(|e| ApiError::internal(format!("Could not raise the backfill cap: {e}")))?;
    Ok(Json(serde_json::json!({ "cap_usd": cap_usd })))
}

#[derive(Deserialize)]
pub(super) struct SummaryTreeQuery {
    thread: Option<Uuid>,
}

pub(super) async fn get_summary_tree(
    State(state): State<AppState>,
    Query(query): Query<SummaryTreeQuery>,
) -> Result<Json<crate::engine::summary_tree::recall::TreeTop>, ApiError> {
    use crate::engine::summary_tree::SummaryScope;
    let scope = query
        .thread
        .map_or(SummaryScope::Workspace, SummaryScope::Thread);
    crate::engine::summary_tree::recall::top(state.engine.pool(), scope)
        .await
        .map(Json)
        .map_err(|e| ApiError::bad_request(format!("Could not read the summary tree: {e}")))
}

#[derive(Deserialize)]
pub(super) struct SummaryTreeThreadsQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

pub(super) async fn get_summary_tree_threads(
    State(state): State<AppState>,
    Query(query): Query<SummaryTreeThreadsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::engine::summary_tree::store;
    let (limit, offset) = page_bounds(query.limit, query.offset);
    let pool = state.engine.pool();
    let (threads, total) = tokio::try_join!(
        store::tree_threads(pool, limit, offset),
        store::in_scope_thread_count(pool),
    )
    .map_err(ApiError::db)?;
    let total = total as i64;
    Ok(Json(serde_json::json!({
        "threads": threads,
        "total": total,
        "has_more": offset.saturating_add(limit) < total,
    })))
}

pub(super) async fn correct_memory_by_id(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::engine::tools::memory::{CorrectionRefusal, MemoryExt};
    let actor = super::actor::user_actor(&headers, None);
    let reply = state
        .engine
        .correct_memory_by_id(&body, actor)
        .await
        .map_err(|e| ApiError::internal(format!("Could not correct the memory: {e}")))?;
    match reply.refusal {
        None => Ok(Json(serde_json::json!({ "result": reply.text }))),
        Some(CorrectionRefusal::InvalidArgs) => Err(ApiError::bad_request(reply.text)),
        Some(CorrectionRefusal::NoSuchEntry) => Err(ApiError::not_found(reply.text)),
        Some(CorrectionRefusal::NotRecorded) => Err(ApiError::internal(reply.text)),
    }
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/memory/correct-by-id", post(correct_memory_by_id))
        .route("/memory/stats", get(get_memory_stats))
        .route("/memory/entries", get(get_memory_entries))
        .route("/memory/search", get(search_memory))
        .route("/memory/source", get(get_memory_source))
        .route(
            "/memory/embedding-model-status",
            get(get_embedding_model_status),
        )
        .route("/memory/tree", get(get_summary_tree))
        .route("/memory/tree/threads", get(get_summary_tree_threads))
        .route("/memory/tree-backfill", get(get_tree_backfill))
        .route(
            "/memory/tree-backfill/estimate",
            get(get_tree_backfill_estimate),
        )
        .route("/memory/tree-backfill/cap", post(raise_tree_backfill_cap))
        .route(
            "/memory/rebuild",
            post(rebuild_memory).delete(cancel_rebuild_memory),
        )
}
