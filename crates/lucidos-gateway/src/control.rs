//! The gateway control plane — `/~/api/v1/control/*`.
//!
//! Lives behind the reserved sigil namespace (ADR 0014 §2) so it can never
//! collide with a workspace slug. Serves the workspace picker's CRUD: list (with
//! per-workspace health), create (provision a stack), rename (registry-only
//! edit), delete-to-trash, and a manual restart for an unhealthy stack.

use crate::error::ApiError;
use crate::server::{GatewayState, RestoreStatus};
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<GatewayState> {
    Router::new()
        .route("/workspaces", get(list).post(create))
        // Restore a local backup archive into a new workspace (picker upload).
        // The body is a multipart upload of a potentially multi-GB `.enc`, so the
        // default 2 MB extractor limit is lifted for this route only.
        .route(
            "/workspaces/restore",
            post(restore).layer(DefaultBodyLimit::disable()),
        )
        .route("/restore-status", get(restore_status).delete(clear_restore))
        // Gateway self-update: is a rebuilt binary waiting, and adopt it (re-exec).
        .route("/gateway/status", get(gateway_status))
        .route("/gateway/reload", post(gateway_reload))
        .route("/workspaces/:id/rename", post(rename))
        .route("/workspaces/:id/restart", post(restart))
        .route("/workspaces/:id/stop", post(stop))
        .route("/workspaces/:id/autostart", post(set_autostart))
        .route("/workspaces/:id", delete(delete_workspace))
}

/// Reject a malformed workspace id (defense in depth — the path-segment lookup
/// already only matches registered slugs, but a clean 400 beats a 404 for a
/// non-slug input, and guards the trash-path construction in delete).
fn reject_invalid_id(id: &str) -> Result<(), ApiError> {
    if crate::registry::is_valid_id(id) {
        Ok(())
    } else {
        Err(ApiError::bad_request("invalid workspace id"))
    }
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
}

#[derive(Deserialize)]
struct RenameBody {
    name: String,
}

#[derive(Deserialize)]
struct AutostartBody {
    enabled: bool,
}

#[derive(Deserialize, Default)]
struct DeleteBody {
    /// Type-the-name confirmation. When present it must match the workspace's
    /// current display name (defense in depth behind the picker's confirm).
    #[serde(default)]
    confirm: Option<String>,
}

async fn list(State(state): State<GatewayState>) -> Json<Value> {
    Json(json!({ "workspaces": state.list_status().await }))
}

async fn create(
    State(state): State<GatewayState>,
    Json(body): Json<CreateBody>,
) -> Result<Json<Value>, ApiError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("workspace name must not be empty"));
    }
    // Picker "+ New": auto-start off by default — the user opens it now; whether
    // it auto-starts on a future gateway boot is their per-workspace toggle.
    let status = state
        .create_workspace(name, false)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!({ "workspace": status })))
}

/// Restore a local encrypted backup archive into a NEW workspace. Multipart body:
/// `file` (the `.enc`), `key` (base64 backup key), and optional `name` (sent only
/// when the derived name collides with an existing workspace). Streams the upload
/// to a temp file, then hands off to the gateway's restore flow (which validates,
/// provisions, shells out to the engine, and registers the workspace). Returns
/// 200 `{id, name}` once the background restore has started — the picker polls
/// `GET /restore-status` for progress.
async fn restore(
    State(state): State<GatewayState>,
    mut multipart: Multipart,
) -> Result<Json<Value>, ApiError> {
    // Reject a concurrent restore before consuming a (possibly multi-GB) upload.
    if matches!(state.restore_status(), RestoreStatus::Running { .. }) {
        return Err(ApiError::conflict("A restore is already in progress"));
    }

    // The streamed temp archive is removed on ANY early return from here on — a
    // connection dropped mid-upload, a malformed trailing field, or a missing
    // key — via this guard, so a (possibly multi-GB) `.enc` is never orphaned in
    // the temp dir. It's disarmed only when ownership passes to
    // `restore_workspace` (which then owns cleanup).
    let mut guard: Option<TempFileGuard> = None;
    let mut filename = String::new();
    let mut key: Option<String> = None;
    let mut name: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("upload error: {e}")))?
    {
        // Capture the field name as owned so the borrow ends before we move the
        // field into the streaming helper.
        let field_name = field.name().map(|s| s.to_string());
        match field_name.as_deref() {
            Some("file") => {
                filename = field.file_name().unwrap_or_default().to_string();
                let path = std::env::temp_dir().join(format!(
                    "lucidos-restore-{}.enc",
                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
                ));
                let g = TempFileGuard::arm(path);
                stream_field_to_file(field, g.path()).await?;
                guard = Some(g);
            }
            Some("key") => {
                key = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad key field: {e}")))?,
                )
            }
            Some("name") => {
                name = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad name field: {e}")))?,
                )
            }
            _ => {}
        }
    }

    let guard = guard.ok_or_else(|| ApiError::bad_request("missing 'file' field"))?;
    let key = key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing backup key"))?;
    let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());

    // Hand the temp file to the restore flow; it removes it when the background
    // restore finishes, or on its own error paths.
    let tmp = guard.disarm();
    let (id, ws_name) = state.restore_workspace(tmp, filename, key, name).await?;
    Ok(Json(json!({ "id": id, "name": ws_name })))
}

/// Removes its path on drop unless [`disarm`](TempFileGuard::disarm)ed. Guards
/// the uploaded restore archive so a multipart error after the file part was
/// streamed never orphans the (possibly multi-GB) temp file.
struct TempFileGuard(Option<std::path::PathBuf>);

impl TempFileGuard {
    fn arm(path: std::path::PathBuf) -> Self {
        Self(Some(path))
    }
    fn path(&self) -> &std::path::Path {
        self.0.as_deref().expect("guard armed")
    }
    fn disarm(mut self) -> std::path::PathBuf {
        self.0.take().expect("guard armed")
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Current restore-flow state for the picker's poll (idle / running+phase /
/// completed / failed).
async fn restore_status(State(state): State<GatewayState>) -> Json<RestoreStatus> {
    Json(state.restore_status())
}

/// Gateway self-update status for the picker's reload control: this process's
/// build id, and whether a newer gateway binary is on disk waiting to be adopted.
async fn gateway_status(State(state): State<GatewayState>) -> Json<Value> {
    Json(json!({
        "build_id": state.build_id(),
        "update_available": state.gateway_update_available().await,
    }))
}

/// Adopt the on-disk gateway binary by re-exec'ing this process onto it (same
/// PID, supervisor untouched, running engines re-adopted on boot). Returns 202
/// before the re-exec so the picker's request resolves; the gateway then briefly
/// drops while the new image binds, and the picker's poll reconnects.
async fn gateway_reload(State(state): State<GatewayState>) -> Result<StatusCode, ApiError> {
    state
        .reload_gateway()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(StatusCode::ACCEPTED)
}

/// Dismiss a terminal restore result (back to Idle). 409 while one is running.
async fn clear_restore(State(state): State<GatewayState>) -> Result<StatusCode, ApiError> {
    state.clear_restore_status()?;
    Ok(StatusCode::NO_CONTENT)
}

/// Stream one multipart field to `path` without buffering the whole upload in
/// memory (a backup archive can be many GB).
async fn stream_field_to_file(
    mut field: axum::extract::multipart::Field<'_>,
    path: &std::path::Path,
) -> Result<(), ApiError> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| ApiError::internal(format!("temp file: {e}")))?;
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|e| ApiError::bad_request(format!("upload read: {e}")))?
    {
        file.write_all(&chunk)
            .await
            .map_err(|e| ApiError::internal(format!("temp write: {e}")))?;
    }
    file.flush()
        .await
        .map_err(|e| ApiError::internal(format!("temp flush: {e}")))?;
    Ok(())
}

async fn rename(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(body): Json<RenameBody>,
) -> Result<StatusCode, ApiError> {
    reject_invalid_id(&id)?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("workspace name must not be empty"));
    }
    state
        .rename_workspace(&id, name)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn restart(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    reject_invalid_id(&id)?;
    state
        .restart_workspace(&id)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(StatusCode::ACCEPTED)
}

/// Stop a workspace's engine but keep its registry entry (it stays listed in the
/// picker as stopped). The dev `stop.sh` calls this so the shared gateway forgets
/// the stack and its supervisor stops respawning the engine.
async fn stop(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    reject_invalid_id(&id)?;
    state
        .stop_workspace(&id)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(StatusCode::ACCEPTED)
}

/// Flip a workspace's auto-start flag (registry only; does not start/stop the
/// engine). Drives the picker's per-workspace auto-start toggle.
async fn set_autostart(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(body): Json<AutostartBody>,
) -> Result<StatusCode, ApiError> {
    reject_invalid_id(&id)?;
    state
        .set_autostart(&id, body.enabled)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_workspace(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    body: Option<Json<DeleteBody>>,
) -> Result<StatusCode, ApiError> {
    reject_invalid_id(&id)?;
    let confirm = body.and_then(|Json(b)| b.confirm);
    state
        .delete_workspace(&id, confirm.as_deref())
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
