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
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::api::AppState;
use crate::core::plugin_marketplaces::{
    add_marketplace, load_registry, remove_marketplace, save_registry, scan_catalog,
    MarketplaceCatalog, PluginMarketplace, MARKETPLACES_DATA_PATH,
};
use crate::core::plugins::PLUGIN_ARCHIVE_EXT;
use crate::core::ArtifactManager;
use crate::engine::tools::plugins::{
    cancel_pending_install, cancel_pending_uninstall, confirm_pending_install,
    confirm_pending_uninstall, installed_plugin_summaries, stage_install_request,
    stage_uninstall_request, PLUGIN_INSTALL_REQUEST_PREFIX, PLUGIN_UNINSTALL_REQUEST_PREFIX,
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

#[derive(Debug, Serialize)]
pub(super) struct MarketplacesResponse {
    pub marketplaces: Vec<PluginMarketplace>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AddMarketplaceRequest {
    pub source: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct AddMarketplaceResponse {
    pub marketplace: PluginMarketplace,
    pub marketplaces: Vec<PluginMarketplace>,
    pub created: bool,
    pub commit: String,
}

#[derive(Debug, Serialize)]
pub(super) struct RemoveMarketplaceResponse {
    pub marketplaces: Vec<PluginMarketplace>,
    pub removed: bool,
    pub commit: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct StageInstallRequest {
    pub source: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct StageUninstallRequest {
    pub id: String,
}

async fn commit_marketplaces_registry(
    state: &AppState,
    headers: &HeaderMap,
    message: &str,
) -> Result<String, (StatusCode, Json<JsonValue>)> {
    let am = ArtifactManager::new(state.workspace_path.clone()).map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("open workspace git repo: {e}"),
        )
    })?;
    let commit = am
        .commit_data_path(MARKETPLACES_DATA_PATH, message)
        .await
        .map_err(|e| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("commit marketplace registry: {e}"),
            )
        })?;

    state
        .engine
        .event_bus
        .emit_user_system(headers, &state.pool, "[Plugins] DataFileWritten", |actor| {
            crate::engine::event_bus::SystemEvent::DataFileWritten {
                path: MARKETPLACES_DATA_PATH.to_string(),
                commit: Some(commit.clone()),
                actor,
            }
        })
        .await;

    Ok(commit)
}

pub(super) async fn list_marketplaces(
    State(state): State<AppState>,
) -> Result<Json<MarketplacesResponse>, (StatusCode, Json<JsonValue>)> {
    let registry = load_registry(&state.workspace_path).map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("read marketplace registry: {e}"),
        )
    })?;
    Ok(Json(MarketplacesResponse {
        marketplaces: registry.marketplaces,
    }))
}

pub(super) async fn add_marketplace_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AddMarketplaceRequest>,
) -> Result<Json<AddMarketplaceResponse>, (StatusCode, Json<JsonValue>)> {
    let _repo_guard = state.engine.lock_workspace_repo().await;
    let mut registry = load_registry(&state.workspace_path).map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("read marketplace registry: {e}"),
        )
    })?;
    let (marketplace, created) = add_marketplace(&mut registry, &body.source, body.name.as_deref())
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
    save_registry(&state.workspace_path, &registry).map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("write marketplace registry: {e}"),
        )
    })?;
    let commit = commit_marketplaces_registry(
        &state,
        &headers,
        if created {
            "Register plugin marketplace"
        } else {
            "Update plugin marketplace"
        },
    )
    .await?;

    let update_engine = state.engine.clone();
    let update_pool = state.pool.clone();
    tokio::spawn(async move {
        crate::scheduler::plugin_updates::run_plugin_marketplace_update_check(
            update_engine,
            update_pool,
        )
        .await;
    });

    Ok(Json(AddMarketplaceResponse {
        marketplace,
        marketplaces: registry.marketplaces,
        created,
        commit,
    }))
}

pub(super) async fn remove_marketplace_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<RemoveMarketplaceResponse>, (StatusCode, Json<JsonValue>)> {
    let _repo_guard = state.engine.lock_workspace_repo().await;
    let mut registry = load_registry(&state.workspace_path).map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("read marketplace registry: {e}"),
        )
    })?;
    let removed = remove_marketplace(&mut registry, &id);
    if !removed {
        return Err(err(StatusCode::NOT_FOUND, "marketplace not found"));
    }
    save_registry(&state.workspace_path, &registry).map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("write marketplace registry: {e}"),
        )
    })?;
    let commit =
        commit_marketplaces_registry(&state, &headers, "Remove plugin marketplace").await?;
    Ok(Json(RemoveMarketplaceResponse {
        marketplaces: registry.marketplaces,
        removed,
        commit,
    }))
}

pub(super) async fn catalog(
    State(state): State<AppState>,
) -> Result<Json<MarketplaceCatalog>, (StatusCode, Json<JsonValue>)> {
    let registry = load_registry(&state.workspace_path).map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("read marketplace registry: {e}"),
        )
    })?;
    let installed = installed_plugin_summaries(&state.pool).await.map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("read installed plugins: {e}"),
        )
    })?;
    let workspace_path = state.workspace_path.clone();
    let mut catalog =
        tokio::task::spawn_blocking(move || scan_catalog(&workspace_path, &registry, &installed))
            .await
            .map_err(|e| {
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("scan marketplaces: {e}"),
                )
            })?;
    mark_setup_complete(&state.pool, &mut catalog).await;
    Ok(Json(catalog))
}

/// Setup counts as "complete" once its thread is no longer actively working or
/// waiting on the user — any `thread_summaries.status` other than `running` /
/// `waiting_for_user_answer` (the two `ThreadStatus` values that mean the setup
/// agent is still mid-turn or blocked on the user). Pure, so the boundary is
/// unit-testable.
fn setup_status_is_complete(status: &str) -> bool {
    !matches!(status, "running" | "waiting_for_user_answer")
}

/// Resolve a setup thread's completion for the App Store card from the two
/// signals we can observe: `summary_status` is its `thread_summaries.status`
/// (`Some` once the thread has a row), `in_queue` is whether a live
/// `thread_queue` entry still exists for it.
/// - present → done once it's neither `running` nor `waiting_for_user_answer`.
/// - absent but queued → still pending (not complete) → card keeps "Setup"
///   through the brief post-spawn lag before the agent's first event lands.
/// - absent and not queued → the thread is *gone* (lost spawn, deleted, or a
///   stale catalog id) → complete, so the card falls through to "Open" instead
///   of a "Setup" button that 404s on click.
/// Pure, so the present/pending/gone boundaries are unit-testable.
fn resolve_setup_complete(summary_status: Option<&str>, in_queue: bool) -> bool {
    match summary_status {
        Some(status) => setup_status_is_complete(status),
        None => !in_queue,
    }
}

/// Fill `MarketplacePlugin::setup_complete` from each setup thread's state. The
/// pure `scan_catalog` can't reach the DB, so it leaves the flag `false`; this
/// enriches the scanned catalog before it ships, routing each id through
/// [`resolve_setup_complete`] (present → lifecycle status; queued → pending;
/// gone → complete). A summary-lookup failure is logged, not fatal — the
/// catalog still renders, just pinned to "Setup". A queue-lookup failure falls
/// back to treating unknown ids as still-queued (pending), so a transient error
/// can never flip a real pending setup to "Open".
async fn mark_setup_complete(pool: &sqlx::PgPool, catalog: &mut MarketplaceCatalog) {
    let ids: Vec<Uuid> = catalog
        .plugins
        .iter()
        .filter_map(|p| p.setup_thread_id.as_deref())
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect();
    if ids.is_empty() {
        return;
    }
    let status_by_id: std::collections::HashMap<Uuid, String> = match sqlx::query_as::<_, (Uuid, String)>(
        "SELECT thread_id, status FROM thread_summaries WHERE thread_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows.into_iter().collect(),
        Err(e) => {
            log!(@Plugins, "setup-complete summary lookup failed (leaving cards on Setup): {}", e);
            return;
        }
    };
    // A setup thread that has been spawned but hasn't emitted its first event
    // yet has no `thread_summaries` row — only a live `thread_queue` entry. On a
    // queue-lookup error we treat every unknown id as still-queued so we never
    // flip a genuinely-pending thread to "Open" (gone) on a transient hiccup.
    let (queued_ids, queue_lookup_failed): (std::collections::HashSet<Uuid>, bool) =
        match sqlx::query_scalar::<_, Uuid>(
            "SELECT thread_id FROM thread_queue WHERE thread_id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(pool)
        .await
        {
            Ok(rows) => (rows.into_iter().collect(), false),
            Err(e) => {
                log!(@Plugins, "setup-complete queue lookup failed (treating unknown threads as pending): {}", e);
                (std::collections::HashSet::new(), true)
            }
        };
    for plugin in &mut catalog.plugins {
        if let Some(tid) = plugin
            .setup_thread_id
            .as_deref()
            .and_then(|s| Uuid::parse_str(s).ok())
        {
            let in_queue = queue_lookup_failed || queued_ids.contains(&tid);
            plugin.setup_complete =
                resolve_setup_complete(status_by_id.get(&tid).map(String::as_str), in_queue);
        }
    }
}

pub(super) async fn stage_install(
    State(state): State<AppState>,
    Json(body): Json<StageInstallRequest>,
) -> Result<Json<JsonValue>, (StatusCode, Json<JsonValue>)> {
    let source = body.source.trim();
    if source.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "source is required"));
    }
    let result = stage_install_request(&state.engine, source).await;
    if let Some(payload) = result.strip_prefix(PLUGIN_INSTALL_REQUEST_PREFIX) {
        let value: JsonValue = serde_json::from_str(payload).map_err(|e| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("parse staged install payload: {e}"),
            )
        })?;
        return Ok(Json(value));
    }
    let msg = result.strip_prefix("Error: ").unwrap_or(&result);
    Err(err(StatusCode::BAD_REQUEST, msg))
}

/// `POST /api/v1/plugins/uninstall-request` — stage an uninstall from the App
/// Store UI. Mirrors `stage_install`: resolves the plugin id, partitions its
/// recorded files into present/missing, and returns the staged
/// `PluginUninstallRequest` JSON for the confirm panel. The same staging the
/// `uninstall_plugin` LLM tool produces, just initiated from a button.
pub(super) async fn stage_uninstall(
    State(state): State<AppState>,
    Json(body): Json<StageUninstallRequest>,
) -> Result<Json<JsonValue>, (StatusCode, Json<JsonValue>)> {
    let id = body.id.trim();
    if id.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "id is required"));
    }
    let result = stage_uninstall_request(&state.engine, id).await;
    if let Some(payload) = result.strip_prefix(PLUGIN_UNINSTALL_REQUEST_PREFIX) {
        let value: JsonValue = serde_json::from_str(payload).map_err(|e| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("parse staged uninstall payload: {e}"),
            )
        })?;
        return Ok(Json(value));
    }
    let msg = result.strip_prefix("Error: ").unwrap_or(&result);
    Err(err(StatusCode::BAD_REQUEST, msg))
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
    let safe_name = super::sanitize_leaf_filename(&raw_name)
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
    /// Set when the installed plugin shipped `setup` instructions: the id of
    /// the Lucidos Agent thread spawned to walk the user through them. The
    /// frontend navigates the user to this thread after a successful install.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_thread_id: Option<uuid::Uuid>,
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
            setup_thread_id: outcome.setup_thread_id,
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

/// Routes for the `/plugins/*` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/plugins/marketplaces",
            get(list_marketplaces).post(add_marketplace_handler),
        )
        .route(
            "/plugins/marketplaces/:id",
            delete(remove_marketplace_handler),
        )
        .route("/plugins/catalog", get(catalog))
        .route("/plugins/install-request", post(stage_install))
        .route("/plugins/uninstall-request", post(stage_uninstall))
        .route(
            "/plugins/upload-archive",
            post(upload_archive).layer(DefaultBodyLimit::max(MAX_ARCHIVE_BYTES)),
        )
        .route(
            "/plugins/install/:install_id/confirm",
            post(confirm_install),
        )
        .route("/plugins/install/:install_id/cancel", post(cancel_install))
        .route(
            "/plugins/uninstall/:uninstall_id/confirm",
            post(confirm_uninstall),
        )
        .route(
            "/plugins/uninstall/:uninstall_id/cancel",
            post(cancel_uninstall),
        )
}

#[cfg(test)]
mod tests {
    use super::{mark_setup_complete, resolve_setup_complete, setup_status_is_complete};
    use crate::core::plugin_marketplaces::{
        MarketplaceCatalog, MarketplacePlugin, MarketplacePluginStatus,
    };
    use crate::test_support::{setup_test_db, teardown_test_db};
    use uuid::Uuid;

    #[test]
    fn setup_complete_only_when_not_running_or_waiting() {
        // The two `ThreadStatus` strings that mean the setup agent is still
        // mid-turn or blocked on the user → not done.
        assert!(!setup_status_is_complete("running"));
        assert!(!setup_status_is_complete("waiting_for_user_answer"));
        // The settled `ThreadStatus` values → done (button flips to Open).
        assert!(setup_status_is_complete("idle"));
        assert!(setup_status_is_complete("failed"));
    }

    fn installed_plugin_with_setup_thread(tid: Uuid) -> MarketplacePlugin {
        MarketplacePlugin {
            marketplace_id: "m".into(),
            marketplace_name: "M".into(),
            id: "p".into(),
            name: "P".into(),
            description: "d".into(),
            version: "0.1.0".into(),
            source: "https://example.invalid/p".into(),
            manifest: serde_json::json!({}),
            content: vec![],
            files_count: 0,
            status: MarketplacePluginStatus::Installed,
            installed_version: Some("0.1.0".into()),
            setup_thread_id: Some(tid.to_string()),
            setup_complete: false,
            app_id: None,
        }
    }

    fn empty_catalog(plugin: MarketplacePlugin) -> MarketplaceCatalog {
        MarketplaceCatalog {
            marketplaces: vec![],
            plugins: vec![plugin],
            errors: vec![],
        }
    }

    /// Regression guard: `mark_setup_complete` must read the lifecycle column
    /// (`status`), not the compose-state column (`state`). A running setup
    /// thread must keep the card on "Setup"; once it settles, "Open". Reading
    /// the wrong column made `setup_complete` always true (the `state` column
    /// holds `active`, never `running`), so the "Setup" button never showed.
    #[tokio::test]
    async fn mark_setup_complete_reads_status_not_state() {
        let (pool, db_name) = setup_test_db().await;
        let tid = Uuid::new_v4();
        sqlx::query("INSERT INTO thread_summaries (thread_id) VALUES ($1)")
            .bind(tid)
            .execute(&pool)
            .await
            .expect("seed thread row");

        // Running setup thread → not complete (card shows "Setup").
        sqlx::query("UPDATE thread_summaries SET status = 'running' WHERE thread_id = $1")
            .bind(tid)
            .execute(&pool)
            .await
            .unwrap();
        let mut catalog = empty_catalog(installed_plugin_with_setup_thread(tid));
        mark_setup_complete(&pool, &mut catalog).await;
        assert!(
            !catalog.plugins[0].setup_complete,
            "a running setup thread must leave setup_complete=false"
        );

        // Settled (idle) → complete (card shows "Open").
        sqlx::query("UPDATE thread_summaries SET status = 'idle' WHERE thread_id = $1")
            .bind(tid)
            .execute(&pool)
            .await
            .unwrap();
        let mut catalog = empty_catalog(installed_plugin_with_setup_thread(tid));
        mark_setup_complete(&pool, &mut catalog).await;
        assert!(
            catalog.plugins[0].setup_complete,
            "an idle setup thread must set setup_complete=true"
        );

        teardown_test_db(&db_name).await;
    }

    #[test]
    fn resolve_setup_complete_present_pending_gone() {
        // Present → lifecycle status decides.
        assert!(!resolve_setup_complete(Some("running"), false));
        assert!(!resolve_setup_complete(Some("waiting_for_user_answer"), false));
        assert!(resolve_setup_complete(Some("idle"), false));
        // Absent but queued → still pending (card keeps Setup, no flicker).
        assert!(!resolve_setup_complete(None, true));
        // Absent and not queued → gone → complete (card falls through to Open).
        assert!(resolve_setup_complete(None, false));
    }

    /// A setup thread that is *gone* — no `thread_summaries` row and no live
    /// `thread_queue` entry (lost spawn, deleted thread, or a stale catalog id)
    /// — must resolve to complete so the card falls through to "Open" instead
    /// of a "Setup" button that 404s on click.
    #[tokio::test]
    async fn mark_setup_complete_gone_thread_falls_through_to_open() {
        let (pool, db_name) = setup_test_db().await;
        let mut catalog = empty_catalog(installed_plugin_with_setup_thread(Uuid::new_v4()));
        mark_setup_complete(&pool, &mut catalog).await;
        assert!(
            catalog.plugins[0].setup_complete,
            "a setup thread with no row and no queue entry must be treated as complete"
        );
        teardown_test_db(&db_name).await;
    }

    /// A setup thread spawned but not yet materialized (a live `thread_queue`
    /// entry, no `thread_summaries` row) is genuinely pending — keep the card on
    /// "Setup" through the post-spawn lag.
    #[tokio::test]
    async fn mark_setup_complete_queued_thread_stays_incomplete() {
        let (pool, db_name) = setup_test_db().await;
        let tid = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO thread_queue (id, kind, thread_id, request) \
             VALUES ($1, 'sub-thread', $2, '{}'::jsonb)",
        )
        .bind(Uuid::new_v4())
        .bind(tid)
        .execute(&pool)
        .await
        .expect("seed thread_queue row");
        let mut catalog = empty_catalog(installed_plugin_with_setup_thread(tid));
        mark_setup_complete(&pool, &mut catalog).await;
        assert!(
            !catalog.plugins[0].setup_complete,
            "a queued-but-not-yet-materialized setup thread must keep the card on Setup"
        );
        teardown_test_db(&db_name).await;
    }
}
