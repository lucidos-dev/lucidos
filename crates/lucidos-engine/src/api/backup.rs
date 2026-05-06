use super::*;
use crate::core::backup::{self, crypto};
use crate::core::oauth::OAuthStore;
use crate::core::PreferenceStore;
use std::path::PathBuf;

#[derive(Serialize)]
pub struct ProviderInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// Whether an OAuth account exists for this provider.
    pub connected: bool,
    /// Whether connected AND the account's scopes contain the required scope.
    pub ready: bool,
    /// The scope substring required for this provider (e.g. "drive"), empty if none needed.
    pub required_scope: &'static str,
}

#[derive(Serialize)]
pub struct KeyResponse {
    pub key: String,
    pub is_new: bool,
}

#[derive(Deserialize)]
pub struct BackupRequest {
    pub provider: String,
}

#[derive(Deserialize)]
pub struct RestoreRequest {
    pub provider: String,
    pub backup_id: String,
    pub key: String,
    pub workspace_name: String,
}

#[derive(Deserialize)]
pub struct ValidateNameQuery {
    pub name: String,
}

#[derive(Serialize)]
pub struct ValidateNameResponse {
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Deserialize)]
pub struct StartWorkspaceRequest {
    pub workspace_path: String,
}

#[derive(Serialize)]
pub struct StartWorkspaceResponse {
    pub url: String,
}

#[derive(Deserialize)]
pub struct ScheduleRequest {
    pub provider: String,
    /// Cron expression, or "off" / empty to disable
    pub schedule: String,
}

#[derive(Serialize)]
pub struct ScheduleResponse {
    pub schedule: Option<String>,
    pub provider: Option<String>,
}

/// JSON error response so the frontend can parse the actual error message.
#[derive(Serialize)]
pub(crate) struct ErrorResponse {
    error: String,
}

fn json_error(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (status, Json(ErrorResponse { error: msg.into() }))
}

pub(crate) fn progress_sender(
    sender: tokio::sync::broadcast::Sender<crate::engine::event_bus::EmittedEvent>,
) -> impl Fn(&str, usize, usize) + Send + Sync + 'static {
    move |phase: &str, current: usize, total: usize| {
        let _ = sender.send(crate::engine::event_bus::EmittedEvent {
            event_id: uuid::Uuid::new_v4(),
            seq: None,
            created: chrono::Utc::now(),
            typed: crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::BackupProgress {
                    phase: phase.to_string(),
                    progress: current,
                    total,
                },
            ),
            aggregate: None,
        });
    }
}

fn resolve_provider(
    provider_id: &str,
    pool: &PgPool,
) -> Result<Box<dyn backup::BackupProvider>, (StatusCode, Json<ErrorResponse>)> {
    backup::get_provider(provider_id, pool).map_err(|e| json_error(StatusCode::BAD_REQUEST, e))
}

pub async fn list_providers(
    State(state): State<AppState>,
) -> Result<Json<Vec<ProviderInfo>>, (StatusCode, Json<ErrorResponse>)> {
    let metas = backup::list_providers();
    let mut result = Vec::with_capacity(metas.len());
    for meta in metas {
        // Surface DB errors instead of silently treating them as "not connected" —
        // a transient DB failure must not be reported as "no OAuth account".
        let account = OAuthStore::get_by_provider(&state.pool, meta.oauth_provider)
            .await
            .map_err(|e| {
                json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "Failed to query OAuth account for {}: {e}",
                        meta.oauth_provider
                    ),
                )
            })?;
        let connected = account.is_some();
        let ready = connected
            && (meta.required_scope.is_empty()
                || account
                    .as_ref()
                    .is_some_and(|a| a.scopes.contains(meta.required_scope)));
        result.push(ProviderInfo {
            id: meta.id,
            name: meta.name,
            connected,
            ready,
            required_scope: meta.required_scope,
        });
    }
    Ok(Json(result))
}

pub async fn get_backup_key(
    State(state): State<AppState>,
) -> Result<Json<KeyResponse>, (StatusCode, Json<ErrorResponse>)> {
    let (key, is_new) = crypto::ensure_key(&state.workspace_path).map_err(|e| {
        json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get backup key: {e}"),
        )
    })?;
    Ok(Json(KeyResponse {
        key: crypto::key_to_base64(&key),
        is_new,
    }))
}

/// Queues a backup and returns 202 immediately; terminal state arrives via
/// the `BackupCompleted` / `BackupFailed` SSE events. The backup pipeline can
/// run for many minutes, so a synchronous handler would race the frontend's
/// AbortController and discard the result.
pub async fn create_backup(
    State(state): State<AppState>,
    Json(req): Json<BackupRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<ErrorResponse>)> {
    let guard = crate::scheduler::BackupGuard::try_acquire(&state.engine)
        .ok_or_else(|| json_error(StatusCode::CONFLICT, "Backup already in progress"))?;

    // Validate sync — guard drops on early return so the flag is released.
    let provider = resolve_provider(&req.provider, &state.pool)?;
    let (key, _) = crypto::ensure_key(&state.workspace_path).map_err(|e| {
        json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get backup key: {e}"),
        )
    })?;

    let engine = state.engine.clone();
    let pool = state.pool.clone();
    let workspace = state.workspace_path.clone();
    let database_url = crate::core::database_url();

    tokio::spawn(async move {
        let _guard = guard;
        crate::scheduler::run_backup(
            &engine,
            &pool,
            &workspace,
            &database_url,
            &key,
            provider.as_ref(),
        )
        .await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "started" })),
    ))
}

pub async fn list_backups(
    State(state): State<AppState>,
    Query(params): Query<BackupRequest>,
) -> Result<Json<Vec<backup::BackupEntry>>, (StatusCode, Json<ErrorResponse>)> {
    let provider = resolve_provider(&params.provider, &state.pool)?;

    let entries = provider.list_backups().await.map_err(|e| {
        json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to list backups: {e}"),
        )
    })?;

    Ok(Json(entries))
}

pub async fn validate_workspace_name(
    Query(params): Query<ValidateNameQuery>,
) -> Json<ValidateNameResponse> {
    match backup::resolve_restore_workspace_path(&params.name) {
        Ok(_) => Json(ValidateNameResponse {
            valid: true,
            reason: None,
        }),
        Err(e) => Json(ValidateNameResponse {
            valid: false,
            reason: Some(e.to_string()),
        }),
    }
}

pub async fn restore_backup(
    State(state): State<AppState>,
    Json(req): Json<RestoreRequest>,
) -> Result<Json<backup::RestoredWorkspace>, (StatusCode, Json<ErrorResponse>)> {
    let provider = resolve_provider(&req.provider, &state.pool)?;

    let key = crypto::key_from_base64(&req.key)
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, format!("Invalid key: {e}")))?;

    let progress = progress_sender(state.engine.event_bus.sender());

    let result = backup::restore_backup(
        &req.workspace_name,
        &key,
        &req.backup_id,
        provider.as_ref(),
        progress,
    )
    .await
    .map_err(|e| {
        json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Restore failed: {e}"),
        )
    })?;

    Ok(Json(result))
}

pub async fn start_workspace(
    Json(req): Json<StartWorkspaceRequest>,
) -> Result<Json<StartWorkspaceResponse>, (StatusCode, Json<ErrorResponse>)> {
    let workspace_path = PathBuf::from(&req.workspace_path);

    // Validate the path is under ~/workspaces/ to prevent arbitrary command execution
    let allowed_parent = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join("workspaces"))
        .map_err(|_| json_error(StatusCode::INTERNAL_SERVER_ERROR, "HOME not set"))?;
    let canonical = workspace_path
        .canonicalize()
        .map_err(|_| json_error(StatusCode::BAD_REQUEST, "Workspace path does not exist"))?;
    if !canonical.starts_with(&allowed_parent) {
        return Err(json_error(
            StatusCode::BAD_REQUEST,
            "Workspace path must be under ~/workspaces/",
        ));
    }

    // Read ports file to determine the URL
    let ports_file = canonical.join(".lucidos").join("ports");
    let ports_content = std::fs::read_to_string(&ports_file).map_err(|_| {
        json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Workspace not initialized — no ports file",
        )
    })?;
    let port = ports_content
        .lines()
        .find_map(|l| l.strip_prefix("API_PORT="))
        .ok_or_else(|| {
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "No API_PORT in ports file",
            )
        })?
        .to_string();

    let web_dev = crate::paths::script("web-dev.sh").map_err(|e| {
        json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Cannot find scripts: {e}"),
        )
    })?;

    let ws_name = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| json_error(StatusCode::BAD_REQUEST, "Invalid workspace path"))?
        .to_string();

    std::process::Command::new("bash")
        .arg(&web_dev)
        .arg("-w")
        .arg(&ws_name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to start workspace: {e}"),
            )
        })?;

    // Determine protocol
    let proto = if std::env::var("LUCIDOS_TLS_CERT").is_ok() {
        "https"
    } else {
        "http"
    };

    // Poll for health (up to 60s)
    let url = format!("{proto}://localhost:{port}");
    let health_url = format!("{url}/api/health");
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| {
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build HTTP client: {e}"),
            )
        })?;

    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if client.get(&health_url).send().await.is_ok() {
            return Ok(Json(StartWorkspaceResponse { url }));
        }
    }

    // Workspace started but not healthy yet — return URL anyway
    Ok(Json(StartWorkspaceResponse { url }))
}

pub async fn get_schedule(
    State(state): State<AppState>,
) -> Result<Json<ScheduleResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Read directly from preferences — no scheduler lock needed
    let cron = PreferenceStore::get(&state.pool, backup::PREF_BACKUP_SCHEDULE)
        .await
        .map_err(|e| {
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get schedule: {e}"),
            )
        })?;
    let provider = PreferenceStore::get(&state.pool, backup::PREF_BACKUP_PROVIDER)
        .await
        .map_err(|e| {
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get schedule: {e}"),
            )
        })?;

    match (cron, provider) {
        (Some(c), Some(p)) if backup::is_schedule_active(&c) => Ok(Json(ScheduleResponse {
            schedule: Some(c),
            provider: Some(p),
        })),
        _ => Ok(Json(ScheduleResponse {
            schedule: None,
            provider: None,
        })),
    }
}

pub async fn set_schedule(
    State(state): State<AppState>,
    Json(req): Json<ScheduleRequest>,
) -> Result<Json<ScheduleResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate provider exists
    let _ = resolve_provider(&req.provider, &state.pool)?;

    // Ensure a backup key exists before enabling a schedule
    if backup::is_schedule_active(&req.schedule) {
        crypto::ensure_key(&state.workspace_path).map_err(|e| {
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to ensure backup key: {e}"),
            )
        })?;
    }

    let active = backup::is_schedule_active(&req.schedule);
    let cron = if active {
        Some(req.schedule.as_str())
    } else {
        None
    };

    let mut scheduler = state.scheduler.lock().await;
    scheduler
        .set_backup_schedule(cron, &req.provider)
        .await
        .map_err(|e| {
            json_error(
                StatusCode::BAD_REQUEST,
                format!("Failed to set schedule: {e}"),
            )
        })?;

    if active {
        Ok(Json(ScheduleResponse {
            schedule: Some(req.schedule),
            provider: Some(req.provider),
        }))
    } else {
        Ok(Json(ScheduleResponse {
            schedule: None,
            provider: None,
        }))
    }
}

#[derive(Serialize)]
pub struct RetentionResponse {
    pub keep: usize,
}

#[derive(Deserialize)]
pub struct RetentionRequest {
    pub keep: usize,
}

pub async fn get_retention(
    State(state): State<AppState>,
) -> Result<Json<RetentionResponse>, (StatusCode, Json<ErrorResponse>)> {
    let keep = backup::get_retention_count(&state.pool).await;
    Ok(Json(RetentionResponse { keep }))
}

pub async fn set_retention(
    State(state): State<AppState>,
    Json(req): Json<RetentionRequest>,
) -> Result<Json<RetentionResponse>, (StatusCode, Json<ErrorResponse>)> {
    if req.keep == 0 {
        return Err(json_error(
            StatusCode::BAD_REQUEST,
            "Must keep at least 1 backup",
        ));
    }
    PreferenceStore::set(
        &state.pool,
        backup::PREF_BACKUP_RETENTION,
        &req.keep.to_string(),
    )
    .await
    .map_err(|e| {
        json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save retention: {e}"),
        )
    })?;
    Ok(Json(RetentionResponse { keep: req.keep }))
}
