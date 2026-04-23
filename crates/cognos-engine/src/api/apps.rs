use super::app_ui::rewrite_for_commit;
use super::*;

use crate::engine::event_bus::{BusEvent, SystemEvent};

/// Query params for apps list
#[derive(Debug, Deserialize)]
pub(super) struct AppsQuery {
    /// Optional commit hash for time travel
    pub commit: Option<String>,
}

/// Query params for single app
#[derive(Debug, Deserialize)]
pub(super) struct AppQuery {
    /// App ID
    pub id: String,
}

/// Validate that an app ID is safe (no path traversal)
fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.contains("..")
        && !id.contains('/')
        && !id.contains('\\')
        && !id.starts_with('.')
}

/// GET /api/apps - List all apps
pub(super) async fn list_apps(
    State(state): State<AppState>,
    Query(query): Query<AppsQuery>,
) -> Response {
    if query.commit.is_some() {
        // Time travel not supported for new app structure yet
        return (
            StatusCode::NOT_IMPLEMENTED,
            "Time travel not yet supported for apps",
        )
            .into_response();
    }

    match state.app_manager.list_apps() {
        Ok(apps) => Json(apps).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// GET /api/app?id=... - Get app details
pub(super) async fn get_app(
    State(state): State<AppState>,
    Query(query): Query<AppQuery>,
) -> Response {
    if !is_valid_id(&query.id) {
        return (StatusCode::BAD_REQUEST, "Invalid app ID").into_response();
    }

    match state.app_manager.get_app(&query.id) {
        Ok(app) => Json(app).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// DELETE /api/app?id=... - Delete an app
pub(super) async fn delete_app(
    State(state): State<AppState>,
    Query(query): Query<AppQuery>,
    headers: HeaderMap,
) -> Response {
    if !is_valid_id(&query.id) {
        return (StatusCode::BAD_REQUEST, "Invalid app ID").into_response();
    }

    let actor = super::actor::user_actor(&headers, None, None);
    match state.app_manager.delete_app(&query.id) {
        Ok(commit) => {
            if let Err(e) = state
                .engine
                .event_bus
                .emit(BusEvent::System(SystemEvent::AppDeleted {
                    app_id: query.id.clone(),
                    actor,
                }))
                .await
            {
                log!("[Apps] Failed to emit AppDeleted event: {}", e);
            }
            Json(serde_json::json!({ "commit": commit })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// PUT /api/app?id=... - Update app metadata (name, description)
pub(super) async fn update_app(
    State(state): State<AppState>,
    Query(query): Query<AppQuery>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if !is_valid_id(&query.id) {
        return (StatusCode::BAD_REQUEST, "Invalid app ID").into_response();
    }

    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return (StatusCode::BAD_REQUEST, "name is required").into_response(),
    };
    let description = body
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let actor = super::actor::user_actor(&headers, None, None);

    match state
        .app_manager
        .update_app_metadata(&query.id, name, description)
    {
        Ok(commit) => {
            if let Err(e) = state
                .engine
                .event_bus
                .emit(BusEvent::System(SystemEvent::AppUpdated {
                    app_id: query.id.clone(),
                    name: Some(name.to_string()),
                    actor,
                }))
                .await
            {
                log!("[Apps] Failed to emit AppUpdated event: {}", e);
            }
            Json(serde_json::json!({ "commit": commit })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// GET /api/app/:app_id/source - Read app source files
pub(super) async fn read_app_source(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
) -> Response {
    let app_id = &app_id;
    if !is_valid_id(app_id) {
        return (StatusCode::BAD_REQUEST, "Invalid ID").into_response();
    }

    match state.app_manager.read_app_source(app_id) {
        Ok(files) => {
            let entries: Vec<serde_json::Value> = files
                .into_iter()
                .map(|(name, content)| serde_json::json!({ "name": name, "content": content }))
                .collect();
            Json(serde_json::json!({ "files": entries })).into_response()
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// PUT /api/app/:app_id/source - Write app source files
pub(super) async fn write_app_source(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let app_id = &app_id;
    if !is_valid_id(app_id) {
        return (StatusCode::BAD_REQUEST, "Invalid ID").into_response();
    }

    let files_val = match body.get("files").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return (StatusCode::BAD_REQUEST, "files array is required").into_response(),
    };

    let mut files = Vec::new();
    for entry in files_val {
        let name = match entry.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return (StatusCode::BAD_REQUEST, "each file needs a name").into_response(),
        };
        let content = match entry.get("content").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => return (StatusCode::BAD_REQUEST, "each file needs content").into_response(),
        };
        files.push((name, content));
    }
    let actor = super::actor::user_actor(&headers, None, None);

    match state.app_manager.write_app_source(app_id, &files) {
        Ok(commit) => {
            if let Err(e) = state
                .engine
                .event_bus
                .emit(BusEvent::System(SystemEvent::AppUpdated {
                    app_id: app_id.to_string(),
                    name: None,
                    actor,
                }))
                .await
            {
                log!("[Apps] Failed to emit AppUpdated event: {}", e);
            }
            Json(serde_json::json!({ "commit": commit })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// GET /api/app/:app_id/ - Serve app UI index.html.
///
/// Apps are static. The engine rewrites HTML only when serving a historical
/// version (`?commit=X`) — it appends the commit suffix to relative
/// `src`/`href` so sub-resources hit the same commit. Current-version requests
/// are returned untouched. SDK, theme CSS, audio shim, favicon, `<title>` are
/// opt-in via tags in the app's HTML (see `knowhow/js-sdk.md`).
pub(super) async fn serve_app_ui(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    Query(query): Query<AppCommitQuery>,
) -> Response {
    let app_id = &app_id;

    if !is_valid_id(app_id) {
        return (StatusCode::BAD_REQUEST, "Invalid ID").into_response();
    }

    let commit = query.commit.as_deref();

    // If commit is specified, serve from git history
    if let Some(commit_hash) = commit {
        let ui_path = format!("data/apps/{}/index.html", app_id);
        return match artifacts::read_file_at_commit(&state.workspace_path, &ui_path, commit_hash) {
            Ok(content) => {
                let rewritten = rewrite_for_commit(&String::from_utf8_lossy(&content), commit);
                (
                    [
                        (header::CONTENT_TYPE, "text/html"),
                        (header::CACHE_CONTROL, "no-store"),
                    ],
                    rewritten,
                )
                    .into_response()
            }
            Err(e) => (StatusCode::NOT_FOUND, format!("Error: {}", e)).into_response(),
        };
    }

    let ui_path = state.app_manager.get_app_path(app_id);

    if !ui_path.exists() {
        return (StatusCode::NOT_FOUND, "App UI not found").into_response();
    }

    match std::fs::read_to_string(&ui_path) {
        Ok(content) => (
            [
                (header::CONTENT_TYPE, "text/html"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            content,
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// GET /api/app/:app_id/*path - Serve app sub-files (CSS, JS, images, etc.)
pub(super) async fn serve_app_file(
    State(state): State<AppState>,
    Path((app_id, file_path)): Path<(String, String)>,
    Query(query): Query<AppCommitQuery>,
) -> Response {
    if !is_valid_id(&app_id) {
        return (StatusCode::BAD_REQUEST, "Invalid ID").into_response();
    }

    if is_path_traversal(&file_path) {
        return (StatusCode::BAD_REQUEST, "Invalid path").into_response();
    }

    let ext = std::path::Path::new(&file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let content_type = content_type_for_ext(ext);

    // If commit is specified, serve from git history
    if let Some(ref commit_hash) = query.commit {
        let git_path = format!("data/apps/{}/{}", app_id, file_path);
        return match artifacts::read_file_at_commit(&state.workspace_path, &git_path, commit_hash) {
            Ok(content) => (
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                content,
            )
                .into_response(),
            Err(e) => (StatusCode::NOT_FOUND, format!("Error: {}", e)).into_response(),
        };
    }

    let full_path = state
        .workspace_path
        .join("data/apps")
        .join(&app_id)
        .join(&file_path);

    match std::fs::read(&full_path) {
        Ok(content) => (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "no-store"),
            ],
            content,
        )
            .into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "File not found").into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// GET /api/app/:app_id/artifacts/*path - Serve shared artifacts for app iframes.
/// Apps using relative paths like `../artifacts/...` resolve here.
pub(super) async fn serve_app_artifact(
    State(state): State<AppState>,
    Path((_app_id, artifact_path)): Path<(String, String)>,
) -> Response {
    if is_path_traversal(&artifact_path) {
        return (StatusCode::BAD_REQUEST, "Invalid path").into_response();
    }

    let full_path = state
        .workspace_path
        .join("data/artifacts")
        .join(&artifact_path);

    let ext = std::path::Path::new(&artifact_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let content_type = content_type_for_ext(ext);

    match std::fs::read(&full_path) {
        Ok(content) => (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "no-store"),
            ],
            content,
        )
            .into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "File not found").into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// GET /api/app/versions?id=...&limit=10&skip=0 - Get git history for app (paginated)
pub(super) async fn get_app_versions(
    State(state): State<AppState>,
    Query(query): Query<AppVersionsQuery>,
) -> Response {
    let app_id = query.id;
    let limit = query.limit.unwrap_or(10).min(100);
    let skip = query.skip.unwrap_or(0);

    if !is_valid_id(&app_id) {
        return (StatusCode::BAD_REQUEST, "Invalid app ID").into_response();
    }

    let app_path = format!("data/apps/{}", app_id);

    match git_log_for_path(&state.workspace_path, &app_path, limit, skip, false) {
        Ok((versions, has_more)) => Json(serde_json::json!({
            "versions": versions,
            "has_more": has_more,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// POST /api/app/restore?id=...&commit=... - Restore app to a historical version
pub(super) async fn restore_app_version(
    State(state): State<AppState>,
    Query(query): Query<AppRestoreQuery>,
    headers: HeaderMap,
) -> Response {
    let app_id = query.id;
    let commit_hash = query.commit;
    let actor = super::actor::user_actor(&headers, None, None);

    if !is_valid_id(&app_id) {
        return (StatusCode::BAD_REQUEST, "Invalid app ID").into_response();
    }
    if super::is_dangerous_git_ref(&commit_hash) {
        return (StatusCode::BAD_REQUEST, "Invalid commit hash").into_response();
    }

    let git_tree_path = format!("data/apps/{}", app_id);
    let app_dir = state.workspace_path.join("data/apps").join(&app_id);

    // Collect current files before restore so we can remove extras
    let files_before: std::collections::HashSet<String> = state
        .app_manager
        .list_app_files(&app_id)
        .into_iter()
        .collect();

    // Restore files from the historical commit
    let output = std::process::Command::new("git")
        .current_dir(&state.workspace_path)
        .args(["checkout", &commit_hash, "--", &git_tree_path])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            // Remove files that existed before but weren't part of the target commit.
            // git checkout only touches files that existed at the commit — any files
            // present before the restore that aren't in the target commit must be
            // explicitly removed to leave the app in a clean restored state.
            let files_after: std::collections::HashSet<String> = state
                .app_manager
                .list_app_files(&app_id)
                .into_iter()
                .collect();
            for file in &files_before {
                if !files_after.contains(file) {
                    let path = app_dir.join(file);
                    if let Err(e) = std::fs::remove_file(&path) {
                        log!(
                            "[Apps] Failed to remove stale file {} during restore: {}",
                            path.display(),
                            e
                        );
                    }
                }
            }

            let short_hash = &commit_hash[..commit_hash.floor_char_boundary(7)];
            let files: Vec<String> = state
                .app_manager
                .list_app_files(&app_id)
                .into_iter()
                .map(|f| format!("{}/{}", app_id, f))
                .collect();
            match state.app_manager.commit_batch(
                &files,
                &format!("Restore app {} to version {}", app_id, short_hash),
            ) {
                Ok(commit) => {
                    if let Err(e) = state
                        .engine
                        .event_bus
                        .emit(BusEvent::System(SystemEvent::AppUpdated {
                            app_id: app_id.clone(),
                            name: None,
                            actor: actor.clone(),
                        }))
                        .await
                    {
                        log!("[Apps] Failed to emit AppUpdated event: {}", e);
                    }
                    Json(serde_json::json!({ "commit": commit })).into_response()
                }
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("{}", e) })),
                )
                    .into_response(),
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("git checkout failed: {}", stderr) })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Run `git log` for a path, returning commits that touched it.
fn git_log_for_path(
    workspace: &std::path::Path,
    path: &str,
    limit: usize,
    skip: usize,
    include_paths: bool,
) -> Result<(Vec<GitVersion>, bool), Box<dyn std::error::Error + Send + Sync>> {
    let fetch_count = limit + 1;

    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(workspace)
        .arg("log")
        .arg(format!("--max-count={}", fetch_count))
        .arg(format!("--skip={}", skip))
        .arg("--format=%H%x00%s%x00%at%x00%an");

    if include_paths {
        cmd.arg("--name-only");
    }

    cmd.arg("--").arg(path);

    let output = cmd.output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git log failed: {}", stderr).into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut versions = Vec::new();

    if include_paths {
        for block in stdout.split("\n\n") {
            let block = block.trim();
            if block.is_empty() {
                continue;
            }
            let mut lines = block.lines();
            if let Some(header) = lines.next() {
                if let Some(version) = parse_git_log_line(header) {
                    let paths: Vec<String> = lines
                        .filter(|l| !l.is_empty())
                        .map(|l| l.to_string())
                        .collect();
                    versions.push(GitVersion {
                        paths: if paths.is_empty() { None } else { Some(paths) },
                        ..version
                    });
                }
            }
        }
    } else {
        for line in stdout.lines() {
            if let Some(version) = parse_git_log_line(line) {
                versions.push(version);
            }
        }
    }

    let has_more = versions.len() > limit;
    if has_more {
        versions.pop();
    }

    Ok((versions, has_more))
}

fn parse_git_log_line(line: &str) -> Option<GitVersion> {
    let parts: Vec<&str> = line.splitn(4, '\0').collect();
    if parts.len() < 4 {
        return None;
    }
    Some(GitVersion {
        commit: parts[0].to_string(),
        message: parts[1].to_string(),
        timestamp: parts[2].parse().unwrap_or(0),
        author: parts[3].to_string(),
        paths: None,
    })
}

pub(super) fn get_git_history_with_paths(
    workspace: &std::path::Path,
    prefix: &str,
    limit: usize,
    offset: usize,
) -> Result<(Vec<GitVersion>, bool), Box<dyn std::error::Error + Send + Sync>> {
    git_log_for_path(workspace, prefix, limit, offset, true)
}

pub(super) async fn submit_app_capture(
    State(state): State<AppState>,
    Json(body): Json<AppCaptureRequest>,
) -> impl IntoResponse {
    let sender = {
        let mut captures = state.engine.pending_captures.lock().unwrap();
        captures.remove(&body.request_id)
    };
    match sender {
        Some(tx) => {
            let _ = tx.send(CaptureResult {
                screenshot: body.screenshot,
                dom: body.dom,
            });
            StatusCode::OK
        }
        None => StatusCode::NOT_FOUND,
    }
}

pub(super) async fn serve_html2canvas() -> impl IntoResponse {
    let js = include_bytes!("../../static/html2canvas.min.js");
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "public, max-age=31536000"),
        ],
        js.as_slice(),
    )
}
