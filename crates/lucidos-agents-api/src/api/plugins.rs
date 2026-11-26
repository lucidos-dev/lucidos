use crate::engine::tools::plugins::marketplaces::MarketplacesExt;
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::api::AppState;
use crate::core::plugin_catalog_cache;
use crate::core::plugin_marketplaces::{
    apply_engine_compatibility_to_catalog, apply_installed_state_to_catalog, load_registry,
    InstalledPluginSummary, MarketplaceCatalog, PluginMarketplace,
};
use crate::core::plugins::PLUGIN_ARCHIVE_EXT;
use crate::engine::release_notices;
use crate::engine::thread_events::FormRequestOutcome;
use crate::engine::thread_lifecycle::ThreadStatus;
use crate::engine::tools::plugins::marketplaces::MarketplaceWriteError;
use crate::engine::tools::plugins::{
    cancel_pending_install, cancel_pending_uninstall, confirm_pending_install,
    confirm_pending_uninstall, installed_plugin_summaries, propose_local_patch_upstream,
    stage_install_request, stage_uninstall_request, LocalChangeReport,
    PLUGIN_INSTALL_REQUEST_PREFIX, PLUGIN_UNINSTALL_REQUEST_PREFIX,
};
use crate::scheduler::plugin_updates::ScanCause;
use crate::triggers::affected::AffectedTrigger;

pub(crate) const MAX_ARCHIVE_BYTES: usize = 50 * 1024 * 1024;

const UPLOAD_TTL: std::time::Duration = std::time::Duration::from_secs(60 * 60);

fn uploads_root(workspace_path: &std::path::Path) -> std::path::PathBuf {
    workspace_path
        .join(crate::core::TMP_DIR)
        .join("plugins")
        .join("uploads")
}

async fn prune_uploads_older_than(root: &std::path::Path, ttl: std::time::Duration) {
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            log_legacy!(@Plugins, "prune uploads: read {:?} failed: {}", root, e);
            return;
        }
    };
    let now = std::time::SystemTime::now();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !upload_is_stale(&path, now, ttl).await {
            continue;
        }
        match tokio::fs::remove_dir_all(&path).await {
            Ok(()) => log_legacy!(@Plugins, "pruned stale upload {}", path.display()),
            Err(e) => log_legacy!(@Plugins, "prune uploads: remove {:?} failed: {}", path, e),
        }
    }
}

async fn upload_is_stale(
    path: &std::path::Path,
    now: std::time::SystemTime,
    ttl: std::time::Duration,
) -> bool {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    now.duration_since(modified)
        .map(|age| age >= ttl)
        .unwrap_or(false)
}

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

#[derive(Debug, Serialize)]
pub(super) struct InstalledPluginsResponse {
    pub plugins: Vec<InstalledPluginSummary>,
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

#[derive(Debug, Deserialize)]
pub(super) struct ProposeUpstreamRequest {
    pub id: String,
}

#[derive(Debug, Serialize)]
pub(super) struct ProposeUpstreamResponse {
    pub patch_path: String,
    pub thread_id: Uuid,
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
    let actor = super::actor::user_actor(&headers, None);
    let registration = state
        .engine
        .register_plugin_marketplace(&body.source, body.name.as_deref(), actor)
        .await
        .map_err(|e| match e {
            MarketplaceWriteError::InvalidSource(msg) => err(StatusCode::BAD_REQUEST, &msg),
            MarketplaceWriteError::Failed(msg) => err(StatusCode::INTERNAL_SERVER_ERROR, &msg),
        })?;

    Ok(Json(AddMarketplaceResponse {
        marketplace: registration.marketplace,
        marketplaces: registration.marketplaces,
        created: registration.created,
        commit: registration.commit,
    }))
}

pub(super) async fn remove_marketplace_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<RemoveMarketplaceResponse>, (StatusCode, Json<JsonValue>)> {
    let actor = super::actor::user_actor(&headers, None);
    let removal = state
        .engine
        .unregister_plugin_marketplace(&id, actor)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "marketplace not found"))?;

    Ok(Json(RemoveMarketplaceResponse {
        marketplaces: removal.marketplaces,
        removed: true,
        commit: removal.commit,
    }))
}

#[derive(Debug, Serialize)]
pub(super) struct CatalogResponse {
    #[serde(flatten)]
    catalog: MarketplaceCatalog,
    scanned_at: Option<String>,
    scanning: bool,
    scan_error: Option<String>,
}

pub(super) async fn catalog(
    State(state): State<AppState>,
) -> Result<Json<CatalogResponse>, (StatusCode, Json<JsonValue>)> {
    read_catalog(&state).await.map(Json)
}

pub(super) async fn read_catalog(
    state: &AppState,
) -> Result<CatalogResponse, (StatusCode, Json<JsonValue>)> {
    let registry = load_registry(&state.workspace_path).map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("read marketplace registry: {e}"),
        )
    })?;
    let installed = installed_plugin_summaries(&state.pool, &state.workspace_path)
        .await
        .map_err(|e| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("read installed plugins: {e}"),
            )
        })?;
    let cached = plugin_catalog_cache::load(&state.workspace_path);
    let now = chrono::Utc::now();

    let due = plugin_catalog_cache::needs_rescan(&cached, now);
    if due {
        spawn_rescan(state, ScanCause::Routine);
    }
    let scanning = due || plugin_catalog_cache::scan_is_running(cached.scan_started_at, now);

    let mut catalog = plugin_catalog_cache::merge_with_registry(&cached, &registry);
    apply_installed_state_to_catalog(&mut catalog, &installed);
    mark_setup_complete(&state.pool, &mut catalog).await;
    apply_engine_compatibility_to_catalog(&mut catalog, &release_notices::running_release());
    Ok(CatalogResponse {
        catalog,
        scanned_at: cached.scanned_at.map(|t| t.to_rfc3339()),
        scanning,
        scan_error: cached.scan_error,
    })
}

/// INVARIANT: `POST /api/v1/plugins/catalog/rescan`, the panel's manual refresh.
///
/// Returns as soon as the scan is queued. A scan already running goes round
/// once more after it, so the answer is a scan that started after the request.
/// The claim is a single flag, so leaning on the button queues one pass at most.
pub(super) async fn rescan_catalog(State(state): State<AppState>) -> Json<JsonValue> {
    spawn_rescan(&state, ScanCause::Requested);
    Json(serde_json::json!({ "queued": true }))
}

fn spawn_rescan(state: &AppState, cause: ScanCause) {
    crate::scheduler::plugin_updates::note_scan_queued(&state.workspace_path);
    let engine = state.engine.clone();
    let pool = state.pool.clone();
    tokio::spawn(async move {
        crate::scheduler::plugin_updates::run_plugin_marketplace_update_check(engine, pool, cause)
            .await;
    });
}

pub(super) async fn installed(
    State(state): State<AppState>,
) -> Result<Json<InstalledPluginsResponse>, (StatusCode, Json<JsonValue>)> {
    read_installed(&state).await.map(Json)
}

pub(super) async fn read_installed(
    state: &AppState,
) -> Result<InstalledPluginsResponse, (StatusCode, Json<JsonValue>)> {
    let plugins = installed_plugin_summaries(&state.pool, &state.workspace_path)
        .await
        .map_err(|e| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("read installed plugins: {e}"),
            )
        })?;
    Ok(InstalledPluginsResponse { plugins })
}

fn setup_status_is_complete(status: ThreadStatus) -> bool {
    !matches!(
        status,
        ThreadStatus::Running | ThreadStatus::WaitingForUserAnswer
    )
}

fn resolve_setup_complete(summary_status: Option<ThreadStatus>, in_queue: bool) -> bool {
    match summary_status {
        Some(status) => setup_status_is_complete(status),
        None => !in_queue,
    }
}

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
    let status_by_id: std::collections::HashMap<Uuid, ThreadStatus> = match sqlx::query_as::<
        _,
        (Uuid, ThreadStatus),
    >(
        "SELECT thread_id, status FROM thread_summaries WHERE thread_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows.into_iter().collect(),
        Err(e) => {
            log_legacy!(@Plugins, "setup-complete summary lookup failed (leaving cards on Setup): {}", e);
            return;
        }
    };
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
                log_legacy!(@Plugins, "setup-complete queue lookup failed (treating unknown threads as pending): {}", e);
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
                resolve_setup_complete(status_by_id.get(&tid).copied(), in_queue);
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

    let uploads = uploads_root(&state.workspace_path);
    prune_uploads_older_than(&uploads, UPLOAD_TTL).await;

    let upload_dir = uploads.join(Uuid::new_v4().simple().to_string());
    tokio::fs::create_dir_all(&upload_dir).await.map_err(|e| {
        log_legacy!(@Plugins, "upload {} ({} bytes): create_dir_all {:?} failed: {}", safe_name, byte_size, upload_dir, e);
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("create upload dir: {e}"),
        )
    })?;

    let dest = upload_dir.join(&safe_name);
    if let Err(e) = tokio::fs::write(&dest, &bytes).await {
        log_legacy!(@Plugins, "upload {} ({} bytes): write {:?} failed: {}", safe_name, byte_size, dest, e);
        let _ = tokio::fs::remove_dir_all(&upload_dir).await;
        return Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("write archive: {e}"),
        ));
    }

    log_legacy!(@Plugins, "staged archive {} ({} bytes) at {}", safe_name, byte_size, dest.display());

    Ok(Json(UploadArchiveResponse {
        path: dest.to_string_lossy().into_owned(),
        filename: safe_name,
        byte_size,
    }))
}

#[derive(Debug, Serialize)]
pub(super) struct ConfirmInstallResponse {
    pub summary: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hooks_needing_review: Vec<String>,
    pub installed_files: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed_files: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub affected_triggers: Vec<AffectedTrigger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_thread_id: Option<uuid::Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_changes: Option<LocalChangesResponse>,
}

#[derive(Debug, Serialize)]
pub(super) struct LocalChangesResponse {
    pub merged: Vec<String>,
    pub conflicted: Vec<String>,
    pub replaced: Vec<String>,
    pub restored: Vec<String>,
    pub removed: Vec<String>,
    pub saved_paths: Vec<String>,
}

impl From<LocalChangeReport> for LocalChangesResponse {
    fn from(r: LocalChangeReport) -> Self {
        Self {
            merged: r.merged,
            conflicted: r.conflicted,
            replaced: r.replaced,
            restored: r.restored,
            removed: r.removed,
            saved_paths: r.saved_paths,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct ConfirmInstallQuery {
    #[serde(default = "keep_local_changes_default")]
    pub keep_local_changes: bool,
    #[serde(default)]
    pub confirm_hooks: bool,
}

fn keep_local_changes_default() -> bool {
    true
}

fn pending_status(err_msg: &str) -> StatusCode {
    if err_msg.starts_with("no pending ") {
        StatusCode::NOT_FOUND
    } else if err_msg == crate::engine::tools::plugins::hooks::CONFIRMATION_REQUIRED {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn owner_confirms_hooks(headers: &HeaderMap) -> bool {
    use super::actor::{owner_attestation, OwnerAttestation};
    use super::secret_reveal::{reveal_request_allowed, RefererRule};
    owner_attestation(headers) == OwnerAttestation::Owner
        && reveal_request_allowed(headers, RefererRule::Required)
}

async fn settle_plugin_request<T>(
    state: &AppState,
    id: &str,
    result: &Result<T, String>,
    done: FormRequestOutcome,
    actor: Option<crate::engine::thread_events::MessageOrigin>,
) {
    let outcome = match result {
        Ok(_) => done,
        Err(e) if pending_status(e) == StatusCode::NOT_FOUND => FormRequestOutcome::Expired,
        Err(_) => return,
    };
    let Ok(request_id) = id.parse() else { return };
    crate::engine::form_requests::resolve_or_log(
        &state.pool,
        &state.engine.event_bus,
        request_id,
        outcome,
        actor,
    )
    .await;
}

pub(super) async fn confirm_install(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(install_id): Path<String>,
    Query(query): Query<ConfirmInstallQuery>,
) -> Result<Json<ConfirmInstallResponse>, (StatusCode, Json<JsonValue>)> {
    prune_uploads_older_than(&uploads_root(&state.workspace_path), UPLOAD_TTL).await;
    let actor = super::actor::user_actor(&headers, None);
    let result = confirm_pending_install(
        &state.engine,
        &install_id,
        query.keep_local_changes,
        query.confirm_hooks && owner_confirms_hooks(&headers),
        actor.clone(),
    )
    .await;
    settle_plugin_request(
        &state,
        &install_id,
        &result,
        FormRequestOutcome::Completed,
        actor,
    )
    .await;
    match result {
        Ok(outcome) => Ok(Json(ConfirmInstallResponse {
            summary: outcome.summary,
            hooks_needing_review: outcome.hooks_needing_review,
            installed_files: outcome.installed_files,
            removed_files: outcome.removed_files,
            affected_triggers: outcome.affected_triggers,
            setup_thread_id: outcome.setup_thread_id,
            local_changes: outcome.local_changes.map(Into::into),
        })),
        Err(e) => Err(err(pending_status(&e), &e)),
    }
}

pub(super) async fn cancel_install(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(install_id): Path<String>,
) -> Result<Json<JsonValue>, (StatusCode, Json<JsonValue>)> {
    prune_uploads_older_than(&uploads_root(&state.workspace_path), UPLOAD_TTL).await;
    let actor = super::actor::user_actor(&headers, None);
    let result = cancel_pending_install(&state.engine, &install_id, actor.clone()).await;
    settle_plugin_request(
        &state,
        &install_id,
        &result,
        FormRequestOutcome::Canceled,
        actor,
    )
    .await;
    match result {
        Ok(()) => Ok(Json(serde_json::json!({"canceled": true}))),
        Err(e) => Err(err(pending_status(&e), &e)),
    }
}

#[derive(Debug, Serialize)]
pub(super) struct ConfirmUninstallResponse {
    pub summary: String,
    pub files_deleted: Vec<String>,
    pub files_missing: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub affected_triggers: Vec<AffectedTrigger>,
}

pub(super) async fn confirm_uninstall(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(uninstall_id): Path<String>,
) -> Result<Json<ConfirmUninstallResponse>, (StatusCode, Json<JsonValue>)> {
    let actor = super::actor::user_actor(&headers, None);
    let result = confirm_pending_uninstall(
        &state.engine,
        &uninstall_id,
        owner_confirms_hooks(&headers),
        actor.clone(),
    )
    .await;
    settle_plugin_request(
        &state,
        &uninstall_id,
        &result,
        FormRequestOutcome::Completed,
        actor,
    )
    .await;
    match result {
        Ok(outcome) => Ok(Json(ConfirmUninstallResponse {
            summary: outcome.summary,
            files_deleted: outcome.files_deleted,
            files_missing: outcome.files_missing,
            affected_triggers: outcome.affected_triggers,
        })),
        Err(e) => Err(err(pending_status(&e), &e)),
    }
}

pub(super) async fn cancel_uninstall(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(uninstall_id): Path<String>,
) -> Result<Json<JsonValue>, (StatusCode, Json<JsonValue>)> {
    let actor = super::actor::user_actor(&headers, None);
    let result = cancel_pending_uninstall(&state.engine, &uninstall_id, actor.clone()).await;
    settle_plugin_request(
        &state,
        &uninstall_id,
        &result,
        FormRequestOutcome::Canceled,
        actor,
    )
    .await;
    match result {
        Ok(()) => Ok(Json(serde_json::json!({"canceled": true}))),
        Err(e) => Err(err(pending_status(&e), &e)),
    }
}

pub(super) async fn propose_upstream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ProposeUpstreamRequest>,
) -> Result<Json<ProposeUpstreamResponse>, (StatusCode, Json<JsonValue>)> {
    let id = body.id.trim();
    if id.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "id is required"));
    }
    let actor = super::actor::user_actor(&headers, None);
    match propose_local_patch_upstream(&state.engine, id, actor).await {
        Ok(outcome) => Ok(Json(ProposeUpstreamResponse {
            patch_path: outcome.patch_path,
            thread_id: outcome.thread_id,
        })),
        Err(e) => Err(err(StatusCode::BAD_REQUEST, &e)),
    }
}

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
        .route("/plugins/catalog/rescan", post(rescan_catalog))
        .route("/plugins/installed", get(installed))
        .route("/plugins/install-request", post(stage_install))
        .route("/plugins/uninstall-request", post(stage_uninstall))
        .route("/plugins/propose-upstream", post(propose_upstream))
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
    use crate::engine::thread_lifecycle::ThreadStatus;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use uuid::Uuid;

    #[tokio::test]
    async fn a_stale_upload_is_reclaimed_and_a_live_one_is_left_alone() {
        let workspace = tempfile::tempdir().expect("a workspace");
        let root = super::uploads_root(workspace.path());
        let staged = root.join("0123abcd");
        tokio::fs::create_dir_all(&staged).await.unwrap();
        tokio::fs::write(staged.join("p.lucidos-plugin"), b"archive bytes")
            .await
            .unwrap();

        super::prune_uploads_older_than(&root, std::time::Duration::from_secs(3600)).await;
        assert!(
            staged.exists(),
            "an upload inside its TTL is still an open install panel's input"
        );

        super::prune_uploads_older_than(&root, std::time::Duration::ZERO).await;
        assert!(!staged.exists(), "an upload past its TTL is reclaimed");
        assert!(root.exists(), "the uploads root survives its own sweep");
    }

    #[tokio::test]
    async fn pruning_a_workspace_that_never_uploaded_is_a_no_op() {
        let workspace = tempfile::tempdir().expect("a workspace");
        let root = super::uploads_root(workspace.path());
        super::prune_uploads_older_than(&root, std::time::Duration::ZERO).await;
        assert!(!root.exists(), "the sweep creates nothing");
    }

    #[test]
    fn setup_complete_only_when_not_running_or_waiting() {
        assert!(!setup_status_is_complete(ThreadStatus::Running));
        assert!(!setup_status_is_complete(
            ThreadStatus::WaitingForUserAnswer
        ));
        assert!(setup_status_is_complete(ThreadStatus::Idle));
        assert!(setup_status_is_complete(ThreadStatus::Failed));
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
            categories: vec![],
            files_count: 0,
            status: MarketplacePluginStatus::Installed,
            installed_version: Some("0.1.0".into()),
            setup_thread_id: Some(tid.to_string()),
            setup_complete: false,
            app_id: None,
            modified: false,
            modified_paths: vec![],
            engine_requirement: None,
            engine_compatible: true,
            engine_incompatible_reason: None,
            media: Default::default(),
        }
    }

    fn empty_catalog(plugin: MarketplacePlugin) -> MarketplaceCatalog {
        MarketplaceCatalog {
            marketplaces: vec![],
            plugins: vec![plugin],
            errors: vec![],
        }
    }

    #[tokio::test]
    async fn mark_setup_complete_reads_status_not_state() {
        let (pool, db_name) = setup_test_db().await;
        let tid = Uuid::new_v4();
        sqlx::query("INSERT INTO thread_summaries (thread_id) VALUES ($1)")
            .bind(tid)
            .execute(&pool)
            .await
            .expect("seed thread row");

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
        assert!(!resolve_setup_complete(Some(ThreadStatus::Running), false));
        assert!(!resolve_setup_complete(
            Some(ThreadStatus::WaitingForUserAnswer),
            false
        ));
        assert!(resolve_setup_complete(Some(ThreadStatus::Idle), false));
        assert!(!resolve_setup_complete(None, true));
        assert!(resolve_setup_complete(None, false));
    }

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

    #[tokio::test]
    async fn an_upload_with_an_unreadable_timestamp_is_not_stale() {
        let workspace = tempfile::tempdir().expect("a workspace");
        let now = std::time::SystemTime::now() + std::time::Duration::from_secs(1);
        let ttl = std::time::Duration::ZERO;
        assert!(super::upload_is_stale(workspace.path(), now, ttl).await);
        assert!(!super::upload_is_stale(&workspace.path().join("missing"), now, ttl).await);
        let before_it_existed = std::time::SystemTime::UNIX_EPOCH;
        assert!(!super::upload_is_stale(workspace.path(), before_it_existed, ttl).await);
    }

    #[tokio::test]
    async fn a_root_that_cannot_be_read_is_left_alone_and_does_not_fail() {
        let workspace = tempfile::tempdir().expect("a workspace");
        let not_a_directory = workspace.path().join("uploads");
        tokio::fs::write(&not_a_directory, b"file").await.unwrap();
        super::prune_uploads_older_than(&not_a_directory, std::time::Duration::ZERO).await;
        assert!(not_a_directory.exists());
    }

    #[test]
    fn plugin_hooks_confirm_requires_an_attested_owner_outside_an_app_document() {
        use crate::api::gateway_stamp::test_support::stamped_device_headers;
        assert!(!super::owner_confirms_hooks(&super::HeaderMap::new()));
        let mut owner = stamped_device_headers("owner");
        assert!(super::owner_confirms_hooks(&owner));
        owner.insert("sec-fetch-site", "same-origin".parse().unwrap());
        assert!(!super::owner_confirms_hooks(&owner));
        owner.insert("referer", "https://example.test/dev/".parse().unwrap());
        assert!(super::owner_confirms_hooks(&owner));
        owner.insert(
            "referer",
            "https://example.test/dev/app/tool/".parse().unwrap(),
        );
        assert!(!super::owner_confirms_hooks(&owner));
    }

    #[test]
    fn a_confirm_that_says_nothing_keeps_the_users_edits() {
        let absent: super::ConfirmInstallQuery = serde_json::from_str("{}").unwrap();
        assert!(absent.keep_local_changes);
        let off: super::ConfirmInstallQuery =
            serde_json::from_str(r#"{"keep_local_changes":false}"#).unwrap();
        assert!(!off.keep_local_changes);
    }

    #[tokio::test]
    async fn a_failed_queue_lookup_leaves_a_pending_setup_on_setup() {
        let (pool, db_name) = setup_test_db().await;
        sqlx::query("DROP TABLE thread_queue CASCADE")
            .execute(&pool)
            .await
            .expect("drop the queue table");
        let mut catalog = empty_catalog(installed_plugin_with_setup_thread(Uuid::new_v4()));
        mark_setup_complete(&pool, &mut catalog).await;
        assert!(
            !catalog.plugins[0].setup_complete,
            "an unreadable queue must read as still queued, never as gone"
        );
        teardown_test_db(&db_name).await;
    }

    mod guard_tests {
        fn production_fn_body(name: &str) -> String {
            let source = crate::test_support::source_scan::read_production_source(
                &crate::test_support::source_scan::engine_source("api/plugins.rs"),
            );
            let start = source
                .find(&format!("fn {name}("))
                .unwrap_or_else(|| panic!("{name} is still here"));
            let rest = &source[start..];
            let end = rest.find("\n}\n").expect("the function ends");
            rest[..end].to_string()
        }

        #[test]
        fn a_catalog_request_never_scans_and_only_queues_a_background_scan() {
            let body = production_fn_body("read_catalog");
            for forbidden in [
                "scan_catalog",
                "run_plugin_marketplace_update_check",
                "git_cmd",
                "Command::new",
            ] {
                assert!(!body.contains(forbidden), "read_catalog runs {forbidden}");
            }
            assert!(body.contains("spawn_rescan(state, ScanCause::Routine)"));
        }

        #[test]
        fn the_catalog_overlays_live_state_after_merging_the_cache() {
            let body = production_fn_body("read_catalog");
            let at = |needle: &str| {
                body.find(needle)
                    .unwrap_or_else(|| panic!("read_catalog lost {needle}"))
            };
            let order = [
                at("merge_with_registry"),
                at("apply_installed_state_to_catalog"),
                at("mark_setup_complete"),
                at("apply_engine_compatibility_to_catalog"),
            ];
            assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
        }
    }
    #[tokio::test]
    async fn a_pending_entry_that_is_gone_answers_404_from_every_confirm_and_cancel() {
        use crate::api::permission_requests::tests::permission_test_state;
        use crate::test_support::boot_mock_engine;
        if !crate::test_support::in_engine_child(
            "api::plugins::tests::a_pending_entry_that_is_gone_answers_404_from_every_confirm_and_cancel",
        )
        .await
        {
            return;
        }
        let (_pool, db) = setup_test_db().await;
        let (engine, workspace) = boot_mock_engine(&db).await;
        let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
        let gone = || super::Path("no-such-staging".to_string());
        let statuses = [
            super::confirm_install(
                super::State(state.clone()),
                super::HeaderMap::new(),
                gone(),
                super::Query(super::ConfirmInstallQuery {
                    keep_local_changes: true,
                    confirm_hooks: false,
                }),
            )
            .await
            .expect_err("nothing is staged")
            .0,
            super::cancel_install(super::State(state.clone()), super::HeaderMap::new(), gone())
                .await
                .expect_err("nothing is staged")
                .0,
            super::confirm_uninstall(super::State(state.clone()), super::HeaderMap::new(), gone())
                .await
                .expect_err("nothing is staged")
                .0,
            super::cancel_uninstall(super::State(state.clone()), super::HeaderMap::new(), gone())
                .await
                .expect_err("nothing is staged")
                .0,
        ];
        for status in statuses {
            assert_eq!(status, super::StatusCode::NOT_FOUND);
        }
        teardown_test_db(&db).await;
    }

    fn archive_upload(payload_bytes: usize) -> axum::http::Request<axum::body::Body> {
        let boundary = "plugin-archive-boundary";
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"p.lucidos-plugin\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .into_bytes();
        body.resize(body.len() + payload_bytes, b'x');
        body.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
        axum::http::Request::builder()
            .method("POST")
            .uri("/plugins/upload-archive")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(axum::body::Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn the_upload_route_caps_an_archive_below_the_wider_api_limit() {
        use crate::api::permission_requests::tests::permission_test_state;
        use crate::test_support::boot_mock_engine;
        use tower::ServiceExt;
        if !crate::test_support::in_engine_child(
            "api::plugins::tests::the_upload_route_caps_an_archive_below_the_wider_api_limit",
        )
        .await
        {
            return;
        }
        let (_pool, db) = setup_test_db().await;
        let (engine, workspace) = boot_mock_engine(&db).await;
        let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
        let router = || {
            super::router()
                .layer(axum::extract::DefaultBodyLimit::max(
                    crate::api::API_BODY_LIMIT,
                ))
                .with_state(state.clone())
        };
        let within = router().oneshot(archive_upload(1024 * 1024)).await.unwrap();
        assert_eq!(within.status(), super::StatusCode::OK);
        let over = router()
            .oneshot(archive_upload(super::MAX_ARCHIVE_BYTES + 1024))
            .await
            .unwrap();
        assert_eq!(over.status(), super::StatusCode::BAD_REQUEST);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_queued_scan_is_stamped_before_its_task_has_run() {
        use crate::api::permission_requests::tests::permission_test_state;
        use crate::test_support::boot_mock_engine;
        if !crate::test_support::in_engine_child(
            "api::plugins::tests::a_queued_scan_is_stamped_before_its_task_has_run",
        )
        .await
        {
            return;
        }
        let (_pool, db) = setup_test_db().await;
        let (engine, workspace) = boot_mock_engine(&db).await;
        let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
        let cause = crate::scheduler::plugin_updates::ScanCause::Requested;
        super::spawn_rescan(&state, cause);
        let cached = crate::core::plugin_catalog_cache::load(workspace.path());
        assert!(crate::core::plugin_catalog_cache::scan_is_running(
            cached.scan_started_at,
            chrono::Utc::now()
        ));
        teardown_test_db(&db).await;
    }
}
