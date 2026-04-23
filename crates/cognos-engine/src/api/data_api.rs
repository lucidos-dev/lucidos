use super::*;

/// Allowed top-level directories the API will serve.
/// `system-docs/` maps to engine-shipped read-only content (see
/// [`crate::core::is_system_doc_path`]); PUT/DELETE/edit reject it.
const ALLOWED_PREFIXES: &[&str] = &[
    "artifacts/",
    "apps/",
    "knowhow/",
    "triggers/",
    "system-docs/",
];

fn validate_data_path(path: &str) -> Result<(), (StatusCode, String)> {
    if path.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Path is required".to_string()));
    }
    if is_path_traversal(path) {
        return Err((StatusCode::BAD_REQUEST, "Invalid path".to_string()));
    }
    if !ALLOWED_PREFIXES.iter().any(|p| path.starts_with(p)) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Path must start with one of: {}",
                ALLOWED_PREFIXES.join(", ")
            ),
        ));
    }
    Ok(())
}

fn read_only_response(path: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": format!("'{}' is read-only (engine-shipped reference)", path)
        })),
    )
        .into_response()
}

fn make_artifact_manager(
    workspace_path: &std::path::Path,
) -> Result<ArtifactManager, Box<Response>> {
    ArtifactManager::new(workspace_path.to_path_buf()).map_err(|e| {
        Box::new(
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response(),
        )
    })
}

/// GET /api/v1/data?pattern=... — list files under data/
#[derive(Deserialize)]
pub(super) struct DataListQuery {
    pub pattern: Option<String>,
}

pub(super) async fn list_data(
    State(state): State<AppState>,
    Query(query): Query<DataListQuery>,
) -> Response {
    let data_dir = state.workspace_path.join(crate::core::DATA_DIR);
    let system_docs_dir = state.engine.system_docs_dir().map(|p| p.to_path_buf());

    let pattern = match query.pattern.as_deref().map(glob::Pattern::new).transpose() {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Invalid glob pattern: {}", e) })),
            )
                .into_response();
        }
    };

    let result = tokio::task::spawn_blocking(move || {
        let mut files = list_data_inner(&data_dir, pattern.as_ref());
        if let Some(sd_dir) = system_docs_dir.as_deref() {
            files.extend(list_system_docs(sd_dir, pattern.as_ref()));
            files.sort();
        }
        files
    })
    .await;

    match result {
        Ok(files) => Json(files).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to list data: {}", e) })),
        )
            .into_response(),
    }
}

/// Walk the engine's system docs directory and return paths under `system-docs/`.
fn list_system_docs(dir: &std::path::Path, pattern: Option<&glob::Pattern>) -> Vec<String> {
    if !dir.is_dir() {
        return Vec::new();
    }
    // Walk via the same helper so behavior matches `list_data_inner`. We use
    // the parent of `dir` as the root so paths come out as `system-docs/...`.
    let parent = match dir.parent() {
        Some(p) => p,
        None => return Vec::new(),
    };
    walkdir(parent, dir)
        .map(|entries| {
            entries
                .into_iter()
                .filter(|e| pattern.is_none_or(|p| p.matches(e)))
                .collect()
        })
        .unwrap_or_else(|e| {
            log!(
                "[data_api] Failed to walk system docs {}: {}",
                dir.display(),
                e
            );
            Vec::new()
        })
}

/// Walk every workspace-data prefix under `data_dir`. With `pattern` set, only
/// paths the pattern matches are returned; with `None`, all walked paths are
/// returned. Skips `system-docs/` (handled separately via [`list_system_docs`])
/// — otherwise a stray workspace-local `data/system-docs/` would surface
/// unreachable filenames that collide with engine-repo paths.
fn list_data_inner(data_dir: &std::path::Path, pattern: Option<&glob::Pattern>) -> Vec<String> {
    let mut files = Vec::new();
    for prefix in ALLOWED_PREFIXES {
        if crate::core::is_system_doc_path(prefix) {
            continue;
        }
        let subdir = data_dir.join(prefix.trim_end_matches('/'));
        if !subdir.is_dir() {
            continue;
        }
        match walkdir(data_dir, &subdir) {
            Ok(entries) => {
                for entry in entries {
                    if pattern.is_none_or(|p| p.matches(&entry)) {
                        files.push(entry);
                    }
                }
            }
            Err(e) => log!(
                "[data_api] Failed to walk {}: {}",
                subdir.display(),
                e
            ),
        }
    }
    files.sort();
    files
}

/// Recursively walk a directory and collect relative paths.
fn walkdir(root: &std::path::Path, dir: &std::path::Path) -> Result<Vec<String>, std::io::Error> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            result.extend(walkdir(root, &path)?);
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                result.push(rel.to_string_lossy().to_string());
            }
        }
    }
    Ok(result)
}

/// GET /api/v1/data/*path — read a data file
pub(super) async fn read_data(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    if let Err((code, msg)) = validate_data_path(&path) {
        return (code, msg).into_response();
    }

    let file_path = if let Some(rel) = path.strip_prefix("system-docs/") {
        match state.engine.system_docs_dir() {
            Some(dir) => dir.join(rel),
            None => return (StatusCode::NOT_FOUND, "System docs not available").into_response(),
        }
    } else {
        state.workspace_path.join(crate::core::DATA_DIR).join(&path)
    };
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    let content_type = content_type_for_ext(&ext);

    match std::fs::read(&file_path) {
        Ok(content) => ([(header::CONTENT_TYPE, content_type)], content).into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "File not found").into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Error reading file: {}", e),
        )
            .into_response(),
    }
}

/// PUT /api/v1/data/*path — write a data file (body is raw content)
pub(super) async fn write_data(
    State(state): State<AppState>,
    Path(path): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    if let Err((code, msg)) = validate_data_path(&path) {
        return (code, Json(serde_json::json!({ "error": msg }))).into_response();
    }
    if crate::core::is_system_doc_path(&path) {
        return read_only_response(&path);
    }

    let am = match make_artifact_manager(&state.workspace_path) {
        Ok(am) => am,
        Err(resp) => return *resp,
    };

    if path.starts_with("artifacts/") {
        let artifact_path = path.strip_prefix("artifacts/").unwrap();
        let content = String::from_utf8_lossy(&body).to_string();
        match am
            .write_and_commit(
                artifact_path,
                &content,
                &format!("Update {}", artifact_path),
            )
            .await
        {
            Ok(commit) => {
                Json(serde_json::json!({ "success": true, "path": path, "commit": commit }))
                    .into_response()
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response(),
        }
    } else {
        let file_path = state.workspace_path.join(crate::core::DATA_DIR).join(&path);
        if let Some(parent) = file_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": format!("Failed to create directory: {}", e) }))).into_response();
            }
        }
        if let Err(e) = std::fs::write(&file_path, &body) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to write file: {}", e) })),
            )
                .into_response();
        }

        match am
            .commit_data_path(&path, &format!("Update {}", path))
            .await
        {
            Ok(commit) => {
                Json(serde_json::json!({ "success": true, "path": path, "commit": commit }))
                    .into_response()
            }
            Err(e) => {
                log!(@data_api, "Warning: file written but commit failed: {}", e);
                Json(serde_json::json!({ "success": true, "path": path, "commit": null }))
                    .into_response()
            }
        }
    }
}

/// DELETE /api/v1/data/*path — delete a data file
pub(super) async fn delete_data(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Response {
    if let Err((code, msg)) = validate_data_path(&path) {
        return (code, Json(serde_json::json!({ "error": msg }))).into_response();
    }
    if crate::core::is_system_doc_path(&path) {
        return read_only_response(&path);
    }

    let am = match make_artifact_manager(&state.workspace_path) {
        Ok(am) => am,
        Err(resp) => return *resp,
    };

    if path.starts_with("artifacts/") {
        let artifact_path = path.strip_prefix("artifacts/").unwrap();
        match am
            .delete_and_commit(artifact_path, &format!("Delete {}", artifact_path))
            .await
        {
            Ok(commit) => {
                Json(serde_json::json!({ "success": true, "path": path, "commit": commit }))
                    .into_response()
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response(),
        }
    } else {
        match am
            .delete_data_path_and_commit(&path, &format!("Delete {}", path))
            .await
        {
            Ok(commit) => {
                Json(serde_json::json!({ "success": true, "path": path, "commit": commit }))
                    .into_response()
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response(),
        }
    }
}

/// POST /api/v1/data/edit — edit a data file (JSON path or text find-replace)
#[derive(Deserialize)]
pub(super) struct DataEditRequest {
    pub path: String,
    pub operations: Vec<DataEditOp>,
}

#[derive(Deserialize)]
pub(super) struct DataEditOp {
    pub json_path: Option<String>,
    pub json_value: Option<serde_json::Value>,
    pub find: Option<String>,
    pub replace: Option<String>,
}

pub(super) async fn edit_data(
    State(state): State<AppState>,
    Json(body): Json<DataEditRequest>,
) -> Response {
    if let Err((code, msg)) = validate_data_path(&body.path) {
        return (code, Json(serde_json::json!({ "error": msg }))).into_response();
    }
    if crate::core::is_system_doc_path(&body.path) {
        return read_only_response(&body.path);
    }

    if body.operations.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "operations must be a non-empty array of {json_path, json_value} or {find[, replace]} objects"
            })),
        )
            .into_response();
    }

    for (i, op) in body.operations.iter().enumerate() {
        let has_json = op.json_path.is_some() && op.json_value.is_some();
        let has_text = op.find.is_some();
        if !has_json && !has_text {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!(
                        "operations[{}]: each operation must have {{json_path, json_value}} or {{find[, replace]}}",
                        i
                    )
                })),
            )
                .into_response();
        }
    }

    for op in &body.operations {
        if let Err(e) = state
            .engine
            .edit_file_at_path(
                &body.path,
                op.json_path.as_deref(),
                op.json_value.clone(),
                op.find.as_deref(),
                op.replace.as_deref(),
                false,
                None,
                None,
            )
            .await
        {
            let status = if e.starts_with("Failed to") {
                StatusCode::INTERNAL_SERVER_ERROR
            } else {
                StatusCode::BAD_REQUEST
            };
            return (status, Json(serde_json::json!({ "error": e }))).into_response();
        }
    }

    Json(serde_json::json!({ "success": true })).into_response()
}

/// POST /api/v1/data/upload — upload a file into data/artifacts/imported/
pub(super) async fn upload_data(
    State(state): State<AppState>,
    multipart: Multipart,
) -> impl IntoResponse {
    super::artifacts::upload_file(State(state), multipart).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pat(p: &str) -> glob::Pattern {
        glob::Pattern::new(p).expect("valid pattern")
    }

    fn touch(dir: &std::path::Path, rel: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }

    #[test]
    fn list_data_inner_filters_by_single_segment_glob() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        for f in ["a.json", "b.json", "c.json", "d.json", "index.json"] {
            touch(data, &format!("artifacts/plans/{}", f));
        }
        touch(data, "artifacts/plans/notes.txt");
        touch(data, "artifacts/other/ignore.json");

        let result = list_data_inner(data, Some(&pat("artifacts/plans/*.json")));
        assert_eq!(
            result,
            vec![
                "artifacts/plans/a.json",
                "artifacts/plans/b.json",
                "artifacts/plans/c.json",
                "artifacts/plans/d.json",
                "artifacts/plans/index.json",
            ]
        );
    }

    #[test]
    fn list_data_inner_filters_by_double_star_recursive() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        touch(data, "artifacts/imported/foo.csv");
        touch(data, "artifacts/imported/a/bar.csv");
        touch(data, "artifacts/imported/a/b/baz.csv");
        touch(data, "artifacts/imported/skip.txt");
        touch(data, "artifacts/other/wrong.csv");

        let result = list_data_inner(data, Some(&pat("artifacts/imported/**/*.csv")));
        assert_eq!(
            result,
            vec![
                "artifacts/imported/a/b/baz.csv",
                "artifacts/imported/a/bar.csv",
                "artifacts/imported/foo.csv",
            ]
        );
    }

    #[test]
    fn list_data_inner_returns_all_when_no_pattern() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        touch(data, "artifacts/x.md");
        touch(data, "knowhow/y.md");
        touch(data, "postgres/internal.bin"); // Disallowed prefix — must be skipped.

        let result = list_data_inner(data, None);
        assert_eq!(result, vec!["artifacts/x.md", "knowhow/y.md"]);
    }

    #[test]
    fn list_data_inner_skips_workspace_system_docs() {
        // A stray <workspace>/data/system-docs/ must NOT surface here:
        // system-docs is served exclusively from the engine repo, and
        // listing both would create unreachable name collisions.
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        touch(data, "artifacts/x.md");
        touch(data, "system-docs/stray.md");

        let result = list_data_inner(data, None);
        assert_eq!(result, vec!["artifacts/x.md"]);
    }

    #[test]
    fn validate_rejects_traversal() {
        assert!(validate_data_path("../etc/passwd").is_err());
        assert!(validate_data_path("/etc/passwd").is_err());
    }

    #[test]
    fn validate_rejects_unknown_prefix() {
        assert!(validate_data_path("postgres/data").is_err());
        assert!(validate_data_path("secret/file").is_err());
    }

    #[test]
    fn validate_accepts_allowed_paths() {
        assert!(validate_data_path("artifacts/report.md").is_ok());
        assert!(validate_data_path("apps/myapp/index.html").is_ok());
        assert!(validate_data_path("knowhow/guide.md").is_ok());
        assert!(validate_data_path("triggers/daily/config.json").is_ok());
        assert!(validate_data_path("system-docs/best-practices.md").is_ok());
    }

    #[test]
    fn list_system_docs_walks_engine_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let sd_dir = repo.join("system-docs");
        std::fs::create_dir_all(&sd_dir).unwrap();
        std::fs::write(sd_dir.join("best-practices.md"), "x").unwrap();
        std::fs::create_dir_all(sd_dir.join("scripts")).unwrap();
        std::fs::write(sd_dir.join("scripts/list.sh"), "#!/bin/sh").unwrap();

        let mut listed = list_system_docs(&sd_dir, None);
        listed.sort();
        assert_eq!(
            listed,
            vec![
                "system-docs/best-practices.md".to_string(),
                "system-docs/scripts/list.sh".to_string(),
            ]
        );
    }

    #[test]
    fn list_system_docs_returns_empty_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        assert!(list_system_docs(&missing, None).is_empty());
    }

    #[test]
    fn serde_rejects_operations_as_string() {
        let json = serde_json::json!({
            "path": "artifacts/test.json",
            "operations": "sections[0].title"
        });
        assert!(serde_json::from_value::<DataEditRequest>(json).is_err());
    }

    #[test]
    fn serde_rejects_operations_as_object() {
        let json = serde_json::json!({
            "path": "artifacts/test.json",
            "operations": { "json_path": "title", "json_value": "x" }
        });
        assert!(serde_json::from_value::<DataEditRequest>(json).is_err());
    }

    #[test]
    fn serde_accepts_valid_operations() {
        let json = serde_json::json!({
            "path": "artifacts/test.json",
            "operations": [{ "json_path": "title", "json_value": "x" }]
        });
        let req: DataEditRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.operations.len(), 1);
        assert_eq!(req.operations[0].json_path.as_deref(), Some("title"));
    }
}
