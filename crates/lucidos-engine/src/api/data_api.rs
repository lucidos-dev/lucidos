use super::*;
use std::sync::LazyLock;

/// Mutable workspace-data prefixes: user-owned trees the API may write and
/// delete.
///
/// The write side cannot tell an app from the shell. So each of these is
/// guarded at the point of USE instead (ADR 0156 decision 2). A new prefix
/// states its answer in this table, and the test below fails if it does not.
///
/// | Prefix | Guard at use |
/// |---|---|
/// | `artifacts/`, `apps/`, `knowhow/`, `triggers/` | none: content the user owns |
/// | `config/` | credential scope, so `apis.json` cannot send a secret off-scope |
/// | `auth-modules/` | the wasmtime sandbox, plus credential scope per handle |
/// | `scripts/` | the handshake approval record: path plus content hash |
///
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
static MUTATE_PREFIX_ERR: LazyLock<String> = LazyLock::new(|| {
    format!(
        "Path must start with one of: {}",
        MUTABLE_PREFIXES.join(", ")
    )
});

/// A path spelling that resolves to the same file as another one.
/// `artifacts/./notes.md`, `artifacts//notes.md` and `artifacts/notes.md/` all
/// reach the same inode as `artifacts/notes.md`.
///
/// One spelling per file, because everything downstream keys on the STRING it
/// was handed rather than on the inode: the `ArtifactUpdated` event, the memory
/// index that consumes it, and the engine's user-profile cache. A `.` segment
/// slips a write past all three, so the file changes while every reader still
/// believes the old one. `is_path_traversal` does not cover this, because it is
/// asking a different question (can this escape the tree), and the answer there
/// is correctly no.
fn is_noncanonical_path(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment.is_empty() || segment == ".")
}

fn validate_path_basics(path: &str) -> Result<(), (StatusCode, String)> {
    if path.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Path is required".to_string()));
    }
    if is_path_traversal(path) {
        return Err((StatusCode::BAD_REQUEST, "Invalid path".to_string()));
    }
    if is_noncanonical_path(path) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Path must not contain empty or '.' segments".to_string(),
        ));
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

/// The workspace-relative file this `/data` mount request names, or `None` when
/// the mount must not serve it.
///
/// `uri_path` is the nest-stripped path, so `/data/artifacts/a.md` arrives as
/// `/artifacts/a.md`. Percent-decoded first: the allowlist reads a filesystem
/// path, and `%2e%2e` is `..` by the time anything opens a file.
///
/// A trailing slash is trimmed rather than refused. `/data/apps/todo/` is how a
/// directory link reaches the mount, and `ServeDir` answers it with that
/// directory's `index.html`.
fn data_mount_target(uri_path: &str) -> Option<String> {
    let decoded = urlencoding::decode(uri_path).ok()?;
    let rel = decoded.trim_start_matches('/').trim_end_matches('/');
    validate_data_path_read(rel).ok()?;
    Some(rel.to_string())
}

/// The `/data` static mount, as `api::mod` nests it.
///
/// **It runs the same allowlist as `GET /api/v1/data/*path`.** Without that the
/// two siblings disagreed about one tree. The API route refused anything
/// outside the prefixes. This mount handed out `data/postgres/postgresql.conf`
/// and every content-addressed blob to any caller that could name the path, an
/// app iframe included.
///
/// One constructor, so a test exercises the real wiring rather than a
/// restatement of it.
pub(super) fn static_mount(data_dir: std::path::PathBuf) -> axum::routing::MethodRouter {
    get(move |req: axum::extract::Request| serve_workspace_data(data_dir.clone(), req))
}

/// Serve one file from the mount, or 404.
///
/// A refused path answers 404, which is what a static mount says about a file
/// it does not serve. It is also the smaller disclosure: 403 would confirm the
/// file exists.
async fn serve_workspace_data(
    data_dir: std::path::PathBuf,
    req: axum::extract::Request,
) -> Response {
    let Some(_rel) = data_mount_target(req.uri().path()) else {
        // Logged because `request_logger` skips `/data`, so a refusal is
        // otherwise invisible to whoever has to explain the 404.
        log!("[data] refused {} on the /data mount", req.uri().path());
        return StatusCode::NOT_FOUND.into_response();
    };
    match ServeDir::new(&data_dir).oneshot(req).await {
        Ok(resp) => resp.map(axum::body::Body::new),
        Err(e) => {
            log!("[data] ServeDir failed: {}", e);
            StatusCode::NOT_FOUND.into_response()
        }
    }
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
///
/// The response shape (validators, ranges, streaming) belongs to
/// [`super::file_response::serve_file`]; this handler owns only path resolution.
pub(super) async fn read_data(
    State(state): State<AppState>,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, msg)) = validate_data_path_read(&path) {
        return (code, msg).into_response();
    }

    let file_path =
        if let Some(rel) = path.strip_prefix(crate::core::knowhow::SYSTEM_KNOWHOW_PREFIX) {
            let Some(dir) = state.engine.system_knowhow_dir() else {
                return (StatusCode::NOT_FOUND, "System knowhow not available").into_response();
            };
            dir.join(rel)
        } else {
            state.workspace_path.join(crate::core::DATA_DIR).join(&path)
        };
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    let content_type = content_type_for_ext(&ext);

    super::file_response::serve_file(&file_path, content_type, &headers).await
}

/// The `artifacts/` arm of [`write_data`]: store the file, commit it, announce
/// the entity event, and refresh the engine's user-profile read-cache.
///
/// The cache refresh belongs here rather than in the store: `ArtifactManager`
/// is the shared write sink, but it is built from a workspace path alone and
/// has no route back to the engine that serves the profile to every chat turn,
/// so only a caller holding both can keep the two coherent. Taking the cache as
/// a parameter is also what makes the refresh testable: an `AppState` needs a
/// booted engine, this needs a tempdir.
async fn write_artifact_data(
    am: &ArtifactManager,
    event_bus: &crate::engine::event_bus::EventBus,
    profile_cache: &crate::engine::user_profile::UserProfileCache,
    artifact_path: &str,
    body: &[u8],
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // `write_and_commit` takes `impl AsRef<[u8]>`, so pass the raw bytes: a
    // binary upload (PNG, PDF, …) would otherwise be silently mangled by a
    // lossy UTF-8 round-trip that replaces every non-UTF-8 byte with U+FFFD.
    // The store announces the Artifact* entity event; the DataFileWritten the
    // caller emits is the API-origin audit event on top of it. Before this the
    // entity event was missing entirely for a data-API write, so a CLI-written
    // artifact never reached the memory index and the frontend carried a
    // workaround arm for the gap.
    let commit = am
        .write_and_commit(
            event_bus,
            artifact_path,
            body,
            &format!("Update {}", artifact_path),
            crate::core::WriteAnnouncement::Entity {
                source: Some("data_api".to_string()),
            },
        )
        .await?;
    // Only once the write has landed: a failed one must not publish content
    // that never reached disk.
    profile_cache.artifact_written(artifact_path, body).await;
    Ok(commit)
}

/// The `artifacts/` arm of [`delete_data`], the mirror of
/// [`write_artifact_data`]. Deleting the profile clears the cache: without it
/// the deleted profile stays in every chat turn's context until a restart.
async fn delete_artifact_data(
    am: &ArtifactManager,
    event_bus: &crate::engine::event_bus::EventBus,
    profile_cache: &crate::engine::user_profile::UserProfileCache,
    artifact_path: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let commit = am
        .delete_and_commit(
            event_bus,
            artifact_path,
            &format!("Delete {}", artifact_path),
        )
        .await?;
    profile_cache.artifact_deleted(artifact_path).await;
    Ok(commit)
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

    // Before the write, not before the emit. A refusal after the bytes reach
    // disk would be a mutation nobody is recorded as making.
    let actor = match crate::api::actor::require_user_actor_response(&headers, &state.pool).await {
        Ok(a) => a,
        Err(resp) => return resp,
    };

    let am = match make_artifact_manager(&state.workspace_path) {
        Ok(am) => am,
        Err(resp) => return *resp,
    };

    let _repo_guard = state.engine.lock_workspace_repo().await;

    let (commit_opt, response) = if let Some(artifact_path) = path.strip_prefix("artifacts/") {
        match write_artifact_data(
            &am,
            &state.engine.event_bus,
            state.engine.user_profile_cache(),
            artifact_path,
            body.as_ref(),
        )
        .await
        {
            Ok(commit) => (
                Some(commit.clone()),
                Json(serde_json::json!({ "success": true, "path": path, "commit": commit }))
                    .into_response(),
            ),
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
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
        .emit_or_log(
            crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::DataFileWritten {
                    path: path.clone(),
                    commit: commit_opt,
                    actor: Some(actor),
                },
            ),
            "[DataApi] DataFileWritten",
        )
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

    // Before the delete, for the reason `write_data` gives above.
    let actor = match crate::api::actor::require_user_actor_response(&headers, &state.pool).await {
        Ok(a) => a,
        Err(resp) => return resp,
    };

    let am = match make_artifact_manager(&state.workspace_path) {
        Ok(am) => am,
        Err(resp) => return *resp,
    };

    let _repo_guard = state.engine.lock_workspace_repo().await;

    let (commit_opt, response) = if let Some(artifact_path) = path.strip_prefix("artifacts/") {
        match delete_artifact_data(
            &am,
            &state.engine.event_bus,
            state.engine.user_profile_cache(),
            artifact_path,
        )
        .await
        {
            Ok(commit) => (
                Some(commit.clone()),
                Json(serde_json::json!({ "success": true, "path": path, "commit": commit }))
                    .into_response(),
            ),
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    } else {
        match am
            .delete_data_path_and_commit(
                &state.engine.event_bus,
                &path,
                &format!("Delete {}", path),
            )
            .await
        {
            Ok(commit) => (
                Some(commit.clone()),
                Json(serde_json::json!({ "success": true, "path": path, "commit": commit }))
                    .into_response(),
            ),
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    };

    state
        .engine
        .event_bus
        .emit_or_log(
            crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::DataFileDeleted {
                    path: path.clone(),
                    commit: commit_opt,
                    actor: Some(actor),
                },
            ),
            "[DataApi] DataFileDeleted",
        )
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

    // Before the first operation is applied, for the reason `write_data` gives.
    let actor = match crate::api::actor::require_user_actor_response(&headers, &state.pool).await {
        Ok(a) => a,
        Err(resp) => return resp,
    };

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
            .edit_file_at_path(crate::engine::tools::files::EditFileArgs {
                raw_path: &body.path,
                // An HTTP caller, so a `scripts/` edit here never makes the
                // file runnable: an app UI reaching this route is
                // indistinguishable from the Files panel (ADR 0144).
                authorship: crate::engine::tools::files::WriteAuthorship::ApiCaller,
                // The HTTP data surface is `data/`-only, so it never names a
                // repo and always takes the committing default.
                repo: None,
                json_path: op.json_path.as_deref(),
                new_value: op.json_value.clone(),
                old_string: op.find.as_deref(),
                new_string: op.replace.as_deref(),
                replace_all: false,
                commit: None,
                message: None,
            })
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
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::DataFileEdited {
                        path: body.path.clone(),
                        operations_count: completed,
                        actor: Some(actor),
                    },
                ),
                "[DataApi] DataFileEdited",
            )
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

/// Routes for the `/data*` surface. The wildcard `/data/*path` write route
/// relies on the 100 MiB `DefaultBodyLimit` that `create_router` layers over
/// the merged API router — larger binary writes (PNG, PDF) would otherwise
/// be rejected by axum's 2 MiB default with "Failed to buffer the request
/// body".
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/data", get(list_data))
        .route("/data/edit", post(edit_data))
        .route("/data/upload", post(upload_data))
        .route(
            "/data/*path",
            get(read_data).put(write_data).delete(delete_data),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::user_profile::UserProfileCache;

    fn pat(p: &str) -> glob::Pattern {
        glob::Pattern::new(p).expect("valid pattern")
    }

    fn touch(dir: &std::path::Path, rel: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }

    /// Regression: the `/data` static mount is gated by the same allowlist the
    /// API route runs.
    ///
    /// It had none. `GET /data/postgres/postgresql.conf` and every blob under
    /// `data/blobs/` were served verbatim to anything that could name the path,
    /// while the sibling `GET /api/v1/data/*path` refused both.
    #[test]
    fn the_data_mount_serves_only_the_read_allowlist() {
        for allowed in [
            "/artifacts/report.pdf",
            "/apps/habit-tracker/index.html",
            "/apps/habit-tracker/",
            "/knowhow/how-to.md",
            "/triggers/nightly/trigger.toml",
            "/config/apis.json",
            "/auth-modules/binance-hmac.wasm",
            "/scripts/auth/login.py",
        ] {
            assert!(
                data_mount_target(allowed).is_some(),
                "{allowed} must still be served"
            );
        }
        for refused in [
            "/postgres/postgresql.conf",
            "/blobs/ab/abcdef.png",
            "/data/artifacts/nested.md",
            "/.backupignore",
            "/.lucidos/tmp/x",
            "/",
            "",
        ] {
            assert!(
                data_mount_target(refused).is_none(),
                "{refused:?} must not be served"
            );
        }
    }

    /// A percent-encoded `..` is `..` by the time a file is opened, so the
    /// allowlist has to see the decoded path.
    #[test]
    fn the_data_mount_decodes_before_it_validates() {
        assert!(data_mount_target("/artifacts/%2e%2e/postgres/x.conf").is_none());
        assert_eq!(
            data_mount_target("/artifacts/my%20report.pdf").as_deref(),
            Some("artifacts/my report.pdf"),
        );
    }

    /// The predicate above decides; this proves it is wired in front of
    /// `ServeDir`. A file that exists on disk under a refused prefix must not
    /// come back, which is the whole defect.
    #[tokio::test]
    async fn the_mount_gates_serve_dir_and_not_just_the_predicate() {
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "artifacts/ok.md");
        touch(tmp.path(), "blobs/ab/secret.png");
        let app = Router::new().nest_service("/data", static_mount(tmp.path().to_path_buf()));

        let get_status = |path: &'static str| {
            let app = app.clone();
            async move {
                app.oneshot(
                    axum::http::Request::builder()
                        .uri(path)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
            }
        };

        assert_eq!(get_status("/data/artifacts/ok.md").await, StatusCode::OK);
        assert_eq!(
            get_status("/data/blobs/ab/secret.png").await,
            StatusCode::NOT_FOUND,
            "a blob that exists on disk must not be served by the mount"
        );
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

    /// Every mutable prefix names what stops it at the point of use.
    ///
    /// No header tells an app from the shell (ADR 0156 decision 1). So a
    /// prefix is safe because of what refuses it downstream, never because of
    /// who wrote it. `scripts/` and `config/` sat here for a long time with
    /// that answer written down nowhere, and a write-then-execute chain grew
    /// in the gap. Adding a prefix now costs one table row.
    #[test]
    fn every_mutable_prefix_states_what_guards_it_at_use() {
        // The doc block immediately above the const, read out of this file's
        // own source. A table in a comment nothing checks is a table that goes
        // stale the first time someone is in a hurry.
        let source = include_str!("data_api.rs");
        let (before, _) = source
            .split_once("const MUTABLE_PREFIXES")
            .expect("the const this test is about");
        let doc: String = before
            .lines()
            .rev()
            .take_while(|l| l.trim_start().starts_with("///"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            doc.contains("Guard at use"),
            "the doc block above MUTABLE_PREFIXES must carry the guard table"
        );
        for prefix in MUTABLE_PREFIXES {
            assert!(
                doc.contains(&format!("`{prefix}`")),
                "{prefix} is writable over the API and the table does not say \
                 what refuses it at the point of use"
            );
        }
        // The check can say no. Without this, a long table would pass the
        // loop above whatever it actually listed.
        assert!(
            !doc.contains("`postgres/`"),
            "a prefix absent from the table must not read as present"
        );
    }

    #[test]
    fn validate_rejects_traversal() {
        assert!(validate_data_path_read("../etc/passwd").is_err());
        assert!(validate_data_path_read("/etc/passwd").is_err());
        assert!(validate_data_path_mutate("../etc/passwd").is_err());
        assert!(validate_data_path_mutate("/etc/passwd").is_err());
    }

    /// A second spelling of a path is not a traversal, so the traversal guard
    /// waves it through: no `..`, no leading slash. It still has to be rejected,
    /// because the file it reaches and the string every consumer keys on stop
    /// agreeing. `artifacts/./user_profile.md` would rewrite the profile while
    /// the engine's cache, the artifact event, and the memory index all recorded
    /// a different artifact, which is the stale-profile bug by another door.
    #[test]
    fn validate_rejects_a_second_spelling_of_the_same_file() {
        for p in [
            "artifacts/./user_profile.md",
            "artifacts//user_profile.md",
            "artifacts/user_profile.md/",
            "artifacts/sub/./notes.md",
            "./artifacts/notes.md",
        ] {
            assert!(validate_data_path_read(p).is_err(), "read accepted {}", p);
            assert!(
                validate_data_path_mutate(p).is_err(),
                "mutate accepted {}",
                p
            );
        }
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

    /// A workspace to write through, plus the profile cache the running engine
    /// would be serving from: the same pair `write_data` holds when it reaches
    /// the artifacts arm.
    fn write_fixture(
        seeded_profile: Option<&str>,
    ) -> (tempfile::TempDir, ArtifactManager, UserProfileCache) {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = dir.path().join(crate::core::ARTIFACTS_DIR);
        std::fs::create_dir_all(&artifacts).unwrap();
        if let Some(content) = seeded_profile {
            std::fs::write(artifacts.join("user_profile.md"), content).unwrap();
        }
        let am = ArtifactManager::new(dir.path().to_path_buf()).unwrap();
        // Loaded from disk the way engine startup does, so the test starts from
        // the state a running engine is actually in.
        let cache = UserProfileCache::load_from_workspace(dir.path());
        (dir, am, cache)
    }

    fn profile_on_disk(dir: &tempfile::TempDir) -> String {
        std::fs::read_to_string(
            dir.path()
                .join(crate::core::ARTIFACTS_DIR)
                .join("user_profile.md"),
        )
        .unwrap()
    }

    /// The bug this guards: a profile written through the data API landed on
    /// disk while the engine kept serving the copy it loaded at startup, so
    /// every chat turn rendered a stale (usually empty) profile until a restart.
    #[tokio::test]
    async fn writing_the_profile_through_the_data_api_refreshes_the_engine_cache() {
        let (dir, am, cache) = write_fixture(None);
        let bus = crate::test_support::offline_event_bus();
        assert_eq!(cache.snapshot().await, "");

        write_artifact_data(
            &am,
            &bus,
            &cache,
            "user_profile.md",
            b"# Profile\n\nDrinks tea.\n",
        )
        .await
        .expect("write must land");

        assert_eq!(cache.snapshot().await, "# Profile\n\nDrinks tea.\n");
        assert_eq!(profile_on_disk(&dir), "# Profile\n\nDrinks tea.\n");
    }

    /// The match is on the whole artifact path, so an imported profile or a
    /// same-suffix sibling is a different artifact and leaves the cache alone.
    #[tokio::test]
    async fn writing_another_artifact_through_the_data_api_leaves_the_profile_cache_alone() {
        let (_dir, am, cache) = write_fixture(Some("mine"));
        let bus = crate::test_support::offline_event_bus();

        for path in [
            "notes.md",
            "imported/user_profile.md",
            "old_user_profile.md",
        ] {
            write_artifact_data(&am, &bus, &cache, path, b"theirs")
                .await
                .expect("write must land");
            assert_eq!(cache.snapshot().await, "mine", "{} touched the cache", path);
        }
    }

    /// Deleting the profile has to clear the cache too: otherwise the deleted
    /// profile keeps being rendered into every chat turn, forever.
    #[tokio::test]
    async fn deleting_the_profile_through_the_data_api_clears_the_engine_cache() {
        let (_dir, am, cache) = write_fixture(Some("mine"));
        let bus = crate::test_support::offline_event_bus();

        delete_artifact_data(&am, &bus, &cache, "user_profile.md")
            .await
            .expect("delete must land");

        assert_eq!(cache.snapshot().await, "");
    }

    #[tokio::test]
    async fn deleting_another_artifact_leaves_the_profile_cache_alone() {
        let (dir, am, cache) = write_fixture(Some("mine"));
        let bus = crate::test_support::offline_event_bus();
        let imported = dir.path().join(crate::core::ARTIFACTS_DIR).join("imported");
        std::fs::create_dir_all(&imported).unwrap();
        std::fs::write(imported.join("user_profile.md"), "theirs").unwrap();

        delete_artifact_data(&am, &bus, &cache, "imported/user_profile.md")
            .await
            .expect("delete must land");

        assert_eq!(cache.snapshot().await, "mine");
    }

    /// A write that never reached disk must not be published to the cache: the
    /// engine would then serve a profile no restart could reproduce.
    #[tokio::test]
    async fn a_failed_profile_write_leaves_the_cache_at_what_landed() {
        let (dir, am, cache) = write_fixture(Some("mine"));
        let bus = crate::test_support::offline_event_bus();

        // Make the write fail at the filesystem: a directory where the file
        // should go is the cheapest reachable failure.
        let path = dir
            .path()
            .join(crate::core::ARTIFACTS_DIR)
            .join("user_profile.md");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        let result = write_artifact_data(&am, &bus, &cache, "user_profile.md", b"never landed")
            .await
            .err();

        assert!(result.is_some(), "the write must report failure");
        assert_eq!(cache.snapshot().await, "mine");
    }
}
