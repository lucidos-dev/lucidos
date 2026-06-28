use super::*;

/// Query params for commits list
#[derive(Deserialize)]
pub(super) struct CommitsQuery {
    pub path: Option<String>,
    #[serde(default = "default_commits_limit")]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: usize,
}

fn default_commits_limit() -> Option<usize> {
    Some(30)
}

#[derive(Serialize)]
pub(super) struct CommitsResponse {
    pub commits: Vec<GitVersion>,
    pub has_more: bool,
}

/// Handle file upload and import
pub(super) async fn upload_file(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Json<UploadResponse> {
    // Get the file from multipart
    let field = match multipart.next_field().await {
        Ok(Some(field)) => field,
        Ok(None) => {
            return Json(UploadResponse {
                success: false,
                filename: None,
                error: Some("No file provided".to_string()),
            });
        }
        Err(e) => {
            return Json(UploadResponse {
                success: false,
                filename: None,
                error: Some(format!("Failed to read upload: {}", e)),
            });
        }
    };

    let raw_filename = field.file_name().unwrap_or("unnamed").to_string();
    let filename = match super::sanitize_leaf_filename(&raw_filename) {
        Some(name) => name,
        None => {
            return Json(UploadResponse {
                success: false,
                filename: Some(raw_filename),
                error: Some("Invalid filename".to_string()),
            });
        }
    };
    let data = match field.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => {
            return Json(UploadResponse {
                success: false,
                filename: Some(filename),
                error: Some(format!("Failed to read file data: {}", e)),
            });
        }
    };

    // Write to a temp file first, then import (since import_file expects a path)
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(&filename);

    if let Err(e) = std::fs::write(&temp_path, &data) {
        return Json(UploadResponse {
            success: false,
            filename: Some(filename),
            error: Some(format!("Failed to write temp file: {}", e)),
        });
    }

    // Import using the engine's import functionality — no lock needed
    let dest = format!("imported/{}", filename);
    log!(@upload, "Starting import for {}", filename);
    match state.engine.import_file_from_path(&temp_path, &dest).await {
        Ok((result, commit_sha, resolved_dest)) => {
            log!(@upload, "Import succeeded: {}", result);
            let _ = std::fs::remove_file(&temp_path);

            // Reflect the ACTUAL final filename — a collision auto-suffix may
            // have landed the file at e.g. "imported/Brev (1).pdf".
            let final_filename = resolved_dest
                .rsplit('/')
                .next()
                .map(str::to_string)
                .unwrap_or(filename);

            // Spawn the post-import hook. Today it's a near no-op (memory
            // indexing for text artifacts runs via `memory_consumer`); kept
            // as the upload-side extension point.
            let engine = Arc::clone(&state.engine);
            tokio::spawn(async move {
                engine.post_import_index(&resolved_dest, &commit_sha).await;
            });

            Json(UploadResponse {
                success: true,
                filename: Some(final_filename),
                error: None,
            })
        }
        Err(e) => {
            log!(@upload, "Import failed: {}", e);
            let _ = std::fs::remove_file(&temp_path);
            Json(UploadResponse {
                success: false,
                filename: Some(filename),
                error: Some(format!("Import failed: {}", e)),
            })
        }
    }
}

/// Get the commit hash at or before a specific Unix timestamp
pub(super) async fn get_commit_at_timestamp(
    State(state): State<AppState>,
    Query(query): Query<BeforeTimestampQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let timestamp = query.before;
    let artifact_manager = ArtifactManager::new(state.workspace_path.clone()).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to access artifacts: {}", e),
        )
    })?;

    let commit = artifact_manager
        .find_commit_at_timestamp(timestamp)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("No commit found: {}", e)))?;

    Ok(Json(serde_json::json!({ "commit": commit })))
}

/// GET /api/v1/commits?path=...&limit=...&offset=... - List commits touching a path prefix
pub(super) async fn list_commits(
    State(state): State<AppState>,
    Query(query): Query<CommitsQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(30).min(200);
    let offset = query.offset;
    let prefix = query.path.as_deref().unwrap_or("data/");

    if is_path_traversal(prefix) {
        return (StatusCode::BAD_REQUEST, "Invalid path").into_response();
    }

    match super::apps::get_git_history_with_paths(&state.workspace_path, prefix, limit, offset).await {
        Ok((commits, has_more)) => Json(CommitsResponse { commits, has_more }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Routes for the `/commits*` surface (artifact git history).
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/commits", get(list_commits))
        .route("/commits/before", get(get_commit_at_timestamp))
}
