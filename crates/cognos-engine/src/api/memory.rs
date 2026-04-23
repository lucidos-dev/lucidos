use super::*;

use crate::core::ArtifactManager;

#[derive(Deserialize)]
pub(super) struct MemoryEntriesQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    source_type: Option<String>,
    sort: Option<String>,
    /// Comma-separated importance levels: low,medium,high,critical
    importance: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct MemorySourceQuery {
    source_id: Option<String>,
    source_type: Option<String>,
    path: Option<String>,
    commit: Option<String>,
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
    let limit = query.limit.unwrap_or(50).min(200);
    let offset = query.offset.unwrap_or(0);
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

    let has_more = (offset + limit) < total;
    Ok(Json(serde_json::json!({
        "entries": entries,
        "total": total,
        "has_more": has_more,
    })))
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
            let id_str = query.source_id.as_deref().ok_or((
                StatusCode::BAD_REQUEST,
                "source_id required for event source".to_string(),
            ))?;
            let event_id = Uuid::from_str(id_str)
                .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid UUID".to_string()))?;

            // Get the original event (may not exist for live-indexed text with synthetic IDs)
            let event = state
                .event_store
                .get_event_by_id(event_id)
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to get event: {}", e),
                    )
                })?;

            // Get all memory entries from this source
            let source_json = serde_json::json!({"type": "event", "id": id_str});
            let entries = index.entries_for_source(&source_json).await.map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to get entries: {}", e),
                )
            })?;

            let event_json = event.map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "event_type": e.event_type,
                    "payload": e.payload,
                    "created": e.created,
                })
            });

            Ok(Json(serde_json::json!({
                "source_type": "event",
                "event": event_json,
                "entries": entries,
            })))
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

            // Get the artifact content at that commit
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

            // Get all memory entries from this source
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

/// Trigger a memory rebuild in the background.
/// Query params: `force=true` clears all entries first (full rebuild); default is resume/incremental.
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
    let force = params.get("force").map(|v| v == "true").unwrap_or(false);
    let engine = state.engine.clone();
    let bus = state.engine.event_bus.clone();

    tokio::spawn(async move {
        engine.rebuild_memory(force, Some(bus)).await;
    });
    let mode = if force { "full_rebuild" } else { "incremental" };
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
