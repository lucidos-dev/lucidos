use super::*;
use std::sync::LazyLock;

/// Mutable workspace-data prefixes — user-owned trees the API may write/delete.
/// `config/` and `auth-modules/` are coupled: `config/apis.json` references
/// signers by name from `auth-modules/`, so deleting one without the other
/// leaves a dangling reference.
const MUTABLE_PREFIXES: &[&str] = &[
    "artifacts/",
    "apps/",
    "knowhow/",
    "triggers/",
    "config/",
    "auth-modules/",
    "scripts/",
];

/// Read-only prefix for engine-shipped reference knowhow served from `<repo>/system-knowhow/`.
/// Allowed for GET so the trigger UI's knowhow links resolve; rejected for PUT/DELETE/edit.
const READ_ONLY_PREFIXES: &[&str] = &["system-knowhow/"];

static READ_PREFIX_ERR: LazyLock<String> = LazyLock::new(|| {
    format!(
        "Path must start with one of: {}",
        MUTABLE_PREFIXES
            .iter()
            .chain(READ_ONLY_PREFIXES.iter())
            .copied()
            .collect::<Vec<_>>()
            .join(", ")
    )
});
static MUTATE_PREFIX_ERR: LazyLock<String> =
    LazyLock::new(|| format!("Path must start with one of: {}", MUTABLE_PREFIXES.join(", ")));

fn validate_path_basics(path: &str) -> Result<(), (StatusCode, String)> {
    if path.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Path is required".to_string()));
    }
    if is_path_traversal(path) {
        return Err((StatusCode::BAD_REQUEST, "Invalid path".to_string()));
    }
    Ok(())
}

fn validate_data_path_read(path: &str) -> Result<(), (StatusCode, String)> {
    validate_path_basics(path)?;
    if !MUTABLE_PREFIXES
        .iter()
        .chain(READ_ONLY_PREFIXES.iter())
        .any(|p| path.starts_with(p))
    {
        return Err((StatusCode::BAD_REQUEST, READ_PREFIX_ERR.to_string()));
    }
    Ok(())
}

fn validate_data_path_mutate(path: &str) -> Result<(), (StatusCode, String)> {
    validate_path_basics(path)?;
    if !MUTABLE_PREFIXES.iter().any(|p| path.starts_with(p)) {
        return Err((StatusCode::BAD_REQUEST, MUTATE_PREFIX_ERR.to_string()));
    }
    Ok(())
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

    let result =
        tokio::task::spawn_blocking(move || list_data_inner(&data_dir, pattern.as_ref())).await;

    match result {
        Ok(files) => Json(files).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to list data: {}", e) })),
        )
            .into_response(),
    }
}

/// Walk every workspace-mutable prefix under `data_dir`. With `pattern` set,
/// only paths the pattern matches are returned; with `None`, all walked paths
/// are returned. Intentionally excludes engine-shipped read-only trees
/// (system-knowhow) — those shouldn't surface in the workspace Files panel.
fn list_data_inner(data_dir: &std::path::Path, pattern: Option<&glob::Pattern>) -> Vec<String> {
    let mut files = Vec::new();
    for prefix in MUTABLE_PREFIXES {
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
            Err(e) => log!("[data_api] Failed to walk {}: {}", subdir.display(), e),
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
    if let Err((code, msg)) = validate_data_path_read(&path) {
        return (code, msg).into_response();
    }

    let file_path = if let Some(rel) = path.strip_prefix(crate::core::knowhow::SYSTEM_KNOWHOW_PREFIX) {
        let Some(dir) = state.engine.system_knowhow_dir() else {
            return (StatusCode::NOT_FOUND, "System knowhow not available").into_response();
        };
        dir.join(rel)
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
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err((code, msg)) = validate_data_path_mutate(&path) {
        return (code, Json(serde_json::json!({ "error": msg }))).into_response();
    }

    let am = match make_artifact_manager(&state.workspace_path) {
        Ok(am) => am,
        Err(resp) => return *resp,
    };

    let _repo_guard = state.engine.lock_workspace_repo().await;

    let (commit_opt, response) = if let Some(artifact_path) = path.strip_prefix("artifacts/") {
        // `write_and_commit` takes `impl AsRef<[u8]>` — pass the raw bytes so
        // binary uploads (PNG, PDF, …) aren't silently mangled by a lossy
        // UTF-8 round-trip that replaces every non-UTF-8 byte with U+FFFD.
        match am
            .write_and_commit(
                artifact_path,
                body.as_ref(),
                &format!("Update {}", artifact_path),
            )
            .await
        {
            Ok(commit) => (
                Some(commit.clone()),
                Json(serde_json::json!({ "success": true, "path": path, "commit": commit }))
                    .into_response(),
            ),
            Err(e) => return (
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
            Ok(commit) => (
                Some(commit.clone()),
                Json(serde_json::json!({ "success": true, "path": path, "commit": commit }))
                    .into_response(),
            ),
            // The file is on disk but the git commit failed — that's a genuine
            // failure, not a no-op: `commit_index` creates an (even empty)
            // commit rather than erroring when there's nothing to commit, so an
            // `Err` here always means git itself failed. Surface it instead of
            // claiming `success: true` with a null commit, matching the
            // artifacts/delete branches. Returning early also skips the
            // `DataFileWritten` audit event, which would otherwise record a
            // commit that never happened.
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("File written to disk but git commit failed: {}", e)
                    })),
                )
                    .into_response();
            }
        }
    };

    state
        .engine
        .event_bus
        .emit_user_system(&headers, &state.pool, "[DataApi] DataFileWritten", |actor| {
            crate::engine::event_bus::SystemEvent::DataFileWritten {
                path: path.clone(),
                commit: commit_opt,
                actor,
            }
        })
        .await;
    response
}

/// DELETE /api/v1/data/*path — delete a data file
pub(super) async fn delete_data(
    State(state): State<AppState>,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, msg)) = validate_data_path_mutate(&path) {
        return (code, Json(serde_json::json!({ "error": msg }))).into_response();
    }

    let am = match make_artifact_manager(&state.workspace_path) {
        Ok(am) => am,
        Err(resp) => return *resp,
    };

    let _repo_guard = state.engine.lock_workspace_repo().await;

    let (commit_opt, response) = if let Some(artifact_path) = path.strip_prefix("artifacts/") {
        match am
            .delete_and_commit(artifact_path, &format!("Delete {}", artifact_path))
            .await
        {
            Ok(commit) => (
                Some(commit.clone()),
                Json(serde_json::json!({ "success": true, "path": path, "commit": commit }))
                    .into_response(),
            ),
            Err(e) => return (
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
            Ok(commit) => (
                Some(commit.clone()),
                Json(serde_json::json!({ "success": true, "path": path, "commit": commit }))
                    .into_response(),
            ),
            Err(e) => return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response(),
        }
    };

    state
        .engine
        .event_bus
        .emit_user_system(&headers, &state.pool, "[DataApi] DataFileDeleted", |actor| {
            crate::engine::event_bus::SystemEvent::DataFileDeleted {
                path: path.clone(),
                commit: commit_opt,
                actor,
            }
        })
        .await;
    response
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
    headers: HeaderMap,
    Json(body): Json<DataEditRequest>,
) -> Response {
    if let Err((code, msg)) = validate_data_path_mutate(&body.path) {
        return (code, Json(serde_json::json!({ "error": msg }))).into_response();
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

    // Emit `DataFileEdited` with the count of operations actually applied —
    // even on partial failure, the file has been mutated by ops 0..completed
    // and the audit log must reflect that. The error response still
    // surfaces the failed op to the caller.
    let mut completed: usize = 0;
    let mut failure: Option<(StatusCode, String)> = None;
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
            )
            .await
        {
            let status = if e.starts_with("Failed to") {
                StatusCode::INTERNAL_SERVER_ERROR
            } else {
                StatusCode::BAD_REQUEST
            };
            failure = Some((status, e));
            break;
        }
        completed += 1;
    }

    if completed > 0 {
        state
            .engine
            .event_bus
            .emit_user_system(&headers, &state.pool, "[DataApi] DataFileEdited", |actor| {
                crate::engine::event_bus::SystemEvent::DataFileEdited {
                    path: body.path.clone(),
                    operations_count: completed,
                    actor,
                }
            })
            .await;
    }

    if let Some((status, e)) = failure {
        return (status, Json(serde_json::json!({ "error": e }))).into_response();
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
        touch(data, "config/apis.json");
        touch(data, "auth-modules/binance-hmac.wasm");
        touch(data, "scripts/foo.py");
        touch(data, "postgres/internal.bin"); // Disallowed prefix — must be skipped.

        let result = list_data_inner(data, None);
        assert_eq!(
            result,
            vec![
                "artifacts/x.md",
                "auth-modules/binance-hmac.wasm",
                "config/apis.json",
                "knowhow/y.md",
                "scripts/foo.py",
            ]
        );
    }

    /// system-knowhow files are engine-shipped reference docs and must NOT
    /// surface in the workspace Files panel even if a workspace happens to
    /// contain a `data/system-knowhow/` directory.
    #[test]
    fn list_data_inner_excludes_system_knowhow() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        touch(data, "artifacts/x.md");
        touch(data, "system-knowhow/best-practices.md");

        let result = list_data_inner(data, None);
        assert_eq!(result, vec!["artifacts/x.md"]);
    }

    #[test]
    fn validate_rejects_traversal() {
        assert!(validate_data_path_read("../etc/passwd").is_err());
        assert!(validate_data_path_read("/etc/passwd").is_err());
        assert!(validate_data_path_mutate("../etc/passwd").is_err());
        assert!(validate_data_path_mutate("/etc/passwd").is_err());
    }

    #[test]
    fn validate_rejects_unknown_prefix() {
        assert!(validate_data_path_read("postgres/data").is_err());
        assert!(validate_data_path_read("secret/file").is_err());
        assert!(validate_data_path_mutate("postgres/data").is_err());
        assert!(validate_data_path_mutate("secret/file").is_err());
    }

    #[test]
    fn read_accepts_system_knowhow_but_mutate_rejects() {
        assert!(validate_data_path_read("system-knowhow/best-practices.md").is_ok());
        assert!(validate_data_path_read("system-knowhow/scripts/list.sh").is_ok());
        assert!(validate_data_path_mutate("system-knowhow/best-practices.md").is_err());
        assert!(validate_data_path_mutate("system-knowhow/scripts/list.sh").is_err());
    }

    #[test]
    fn validate_accepts_allowed_paths() {
        for p in [
            "artifacts/report.md",
            "apps/myapp/index.html",
            "knowhow/guide.md",
            "triggers/daily/config.json",
            "config/apis.json",
            "auth-modules/binance-hmac.wasm",
            "auth-modules/binance-hmac.manifest.json",
            "scripts/foo.py",
        ] {
            assert!(validate_data_path_read(p).is_ok());
            assert!(validate_data_path_mutate(p).is_ok());
        }
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
