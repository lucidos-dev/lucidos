//! HTTP endpoint for staging a `.lucidos-plugin` archive uploaded from the
//! browser onto a filesystem path the LLM tool `install_plugin` can consume.
//!
//! The plugins v1 design (`docs/plans/2026-04-29-plugins-v1-design.md`) keeps
//! the install logic in the LLM tool. The browser cannot hand a `File` blob
//! to the tool directly, so we stage the bytes under
//! `.lucidos/tmp/plugins/uploads/<uuid>/<name>` and return the absolute path.
//! The chat layer then sends a message like "Install the plugin at <path>",
//! and the LLM calls `install_plugin` with that path.

use axum::{
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Serialize;
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::api::AppState;
use crate::core::plugins::PLUGIN_ARCHIVE_EXT;
use crate::engine::tools::plugins::{
    cancel_pending_install, cancel_pending_uninstall, confirm_pending_install,
    confirm_pending_uninstall,
};

/// Plugin archives are mostly text bundles; cap well below the router-wide
/// `DefaultBodyLimit`. The route in `api/mod.rs` applies a per-route
/// `DefaultBodyLimit::max(MAX_ARCHIVE_BYTES)` so axum rejects oversized
/// requests before the body is buffered.
pub(crate) const MAX_ARCHIVE_BYTES: usize = 50 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub(super) struct UploadArchiveResponse {
    pub path: String,
    pub filename: String,
    pub byte_size: u64,
}

fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<JsonValue>) {
    (code, Json(serde_json::json!({ "error": msg })))
}

pub(super) async fn upload_archive(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<UploadArchiveResponse>, (StatusCode, Json<JsonValue>)> {
    let field = multipart
        .next_field()
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, &format!("multipart read: {e}")))?
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "missing file part"))?;

    let raw_name = field
        .file_name()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "missing filename"))?
        .to_string();
    let safe_name = sanitize_filename(&raw_name)
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "invalid filename"))?;
    if !safe_name.to_ascii_lowercase().ends_with(PLUGIN_ARCHIVE_EXT) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "filename must end in .lucidos-plugin",
        ));
    }

    let bytes = field
        .bytes()
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, &format!("read body: {e}")))?;
    let byte_size = bytes.len() as u64;

    let upload_dir = state
        .workspace_path
        .join(".lucidos")
        .join("tmp")
        .join("plugins")
        .join("uploads")
        .join(Uuid::new_v4().simple().to_string());
    tokio::fs::create_dir_all(&upload_dir).await.map_err(|e| {
        log!(@Plugins, "upload {} ({} bytes): create_dir_all {:?} failed: {}", safe_name, byte_size, upload_dir, e);
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("create upload dir: {e}"),
        )
    })?;

    let dest = upload_dir.join(&safe_name);
    tokio::fs::write(&dest, &bytes).await.map_err(|e| {
        log!(@Plugins, "upload {} ({} bytes): write {:?} failed: {}", safe_name, byte_size, dest, e);
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("write archive: {e}"),
        )
    })?;

    log!(@Plugins, "staged archive {} ({} bytes) at {}", safe_name, byte_size, dest.display());

    Ok(Json(UploadArchiveResponse {
        path: dest.to_string_lossy().into_owned(),
        filename: safe_name,
        byte_size,
    }))
}

#[derive(Debug, Serialize)]
pub(super) struct ConfirmInstallResponse {
    pub summary: String,
    pub installed_files: Vec<String>,
}

/// 404 when the pending entry is gone (already consumed, expired, or wrong
/// id); 500 for genuine write/emit failures. Both install and uninstall
/// helpers return their "missing entry" error with the `no pending ` prefix.
fn pending_status(err_msg: &str) -> StatusCode {
    if err_msg.starts_with("no pending ") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// `POST /api/v1/plugins/install/:install_id/confirm` — user accepted the
/// staged install in the install panel. Pops the entry, writes files into
/// `data/`, emits `PluginInstalled` (stamped with the device that clicked
/// Confirm), and (if the install touched any `auth-modules/` paths)
/// auto-reloads the proxy WASM signer map.
pub(super) async fn confirm_install(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(install_id): Path<String>,
) -> Result<Json<ConfirmInstallResponse>, (StatusCode, Json<JsonValue>)> {
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match confirm_pending_install(&state.engine, &install_id, actor).await {
        Ok(outcome) => Ok(Json(ConfirmInstallResponse {
            summary: outcome.summary,
            installed_files: outcome.installed_files,
        })),
        Err(e) => Err(err(pending_status(&e), &e)),
    }
}

/// `POST /api/v1/plugins/install/:install_id/cancel` — user dismissed the
/// staged install. Drops the staged temp dir and emits
/// `PluginInstallCanceled` (stamped with the device that clicked Cancel)
/// for audit. Idempotent: a missing `install_id` returns 404 (cleaner than
/// treating "not pending" as success).
pub(super) async fn cancel_install(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(install_id): Path<String>,
) -> Result<Json<JsonValue>, (StatusCode, Json<JsonValue>)> {
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match cancel_pending_install(&state.engine, &install_id, actor).await {
        Ok(()) => Ok(Json(serde_json::json!({"canceled": true}))),
        Err(e) => Err(err(pending_status(&e), &e)),
    }
}

#[derive(Debug, Serialize)]
pub(super) struct ConfirmUninstallResponse {
    pub summary: String,
    pub files_deleted: Vec<String>,
    pub files_missing: Vec<String>,
}

/// `POST /api/v1/plugins/uninstall/:uninstall_id/confirm` — user accepted the
/// staged uninstall in the panel. Pops the entry, deletes the recorded files
/// from `data/`, prunes empty parent dirs, emits `PluginUninstalled` (stamped
/// with the confirming device), and (if any `auth-modules/` paths were
/// touched) reloads the proxy WASM signer map. Symmetric with `confirm_install`.
pub(super) async fn confirm_uninstall(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(uninstall_id): Path<String>,
) -> Result<Json<ConfirmUninstallResponse>, (StatusCode, Json<JsonValue>)> {
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match confirm_pending_uninstall(&state.engine, &uninstall_id, actor).await {
        Ok(outcome) => Ok(Json(ConfirmUninstallResponse {
            summary: outcome.summary,
            files_deleted: outcome.files_deleted,
            files_missing: outcome.files_missing,
        })),
        Err(e) => Err(err(pending_status(&e), &e)),
    }
}

/// `POST /api/v1/plugins/uninstall/:uninstall_id/cancel` — user dismissed the
/// staged uninstall. No files are touched; emits `PluginUninstallCanceled`
/// for audit. Idempotent (404 on missing id).
pub(super) async fn cancel_uninstall(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(uninstall_id): Path<String>,
) -> Result<Json<JsonValue>, (StatusCode, Json<JsonValue>)> {
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match cancel_pending_uninstall(&state.engine, &uninstall_id, actor).await {
        Ok(()) => Ok(Json(serde_json::json!({"canceled": true}))),
        Err(e) => Err(err(pending_status(&e), &e)),
    }
}

/// Reject filenames that would escape the upload directory or break the
/// `<uuid>/<name>` shape. Mirrors `is_path_traversal` (rejects any `..`
/// substring, not just the whole-name case) plus null-byte and empty-name
/// guards; the upload dir is per-request UUID-named so collisions are
/// impossible and we only need to keep the name a leaf.
fn sanitize_filename(name: &str) -> Option<String> {
    if name.is_empty()
        || name == "."
        || name.contains("..")
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return None;
    }
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_path_separators_and_traversal() {
        assert!(sanitize_filename("foo/bar.lucidos-plugin").is_none());
        assert!(sanitize_filename("foo\\bar.lucidos-plugin").is_none());
        assert!(sanitize_filename("../escape.lucidos-plugin").is_none());
        assert!(sanitize_filename("foo..bar.lucidos-plugin").is_none());
        assert!(sanitize_filename(".").is_none());
        assert!(sanitize_filename("..").is_none());
        assert!(sanitize_filename("").is_none());
        assert!(sanitize_filename("nul\0byte.lucidos-plugin").is_none());
    }

    #[test]
    fn sanitize_accepts_normal_names() {
        assert_eq!(
            sanitize_filename("no-role-playing-0.1.1.lucidos-plugin"),
            Some("no-role-playing-0.1.1.lucidos-plugin".to_string()),
        );
        assert_eq!(
            sanitize_filename("Plugin With Space 1.0.lucidos-plugin"),
            Some("Plugin With Space 1.0.lucidos-plugin".to_string()),
        );
    }
}
