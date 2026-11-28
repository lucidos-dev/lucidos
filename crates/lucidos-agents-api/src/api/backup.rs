use super::secret_reveal::{
    forbidden, reveal_request_allowed, token_required, RefererRule, RevealSubject,
    RevealTokenResponse,
};
use super::*;
use crate::core::backup::{self, key::BackupKeyStore, recipient};
use crate::core::{prefs, PreferenceStore};
use crate::engine::thread_events::MessageOrigin;
#[cfg(test)]
use crate::scheduler::progress_sender;

const KEY_MINT_ROUTE: &str = "/api/v1/backup/key/reveal-token";

const KEY_SUBJECT_LABEL: &str = "the backup key";

#[derive(Serialize)]
pub struct ProviderInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub connected: bool,
    pub ready: bool,
    pub missing_scopes: Vec<&'static str>,
    pub folder_url: Option<String>,
}

#[derive(Serialize)]
pub struct KeyResponse {
    pub key: String,
}

#[derive(Serialize)]
pub struct BackupKeysResponse {
    pub recipient: Option<String>,
    pub pending: bool,
    pub legacy_key: bool,
    pub secs_since_verified: Option<i64>,
}

#[derive(Serialize)]
pub struct BackupKeySetupResponse {
    pub backup_key: String,
}

#[derive(Deserialize)]
pub struct BackupKeyRequest {
    pub backup_key: String,
}

#[derive(Deserialize)]
pub struct OwnRecipientRequest {
    pub recipient: String,
}

#[derive(Serialize)]
pub struct VerifyResponse {
    pub matches: bool,
}

fn canonical_provider_id(raw: &str) -> String {
    backup::PROVIDER_IDS
        .iter()
        .find(|id| id.replace('_', "-") == raw)
        .map_or_else(|| raw.to_string(), |id| (*id).to_string())
}

fn deserialize_provider<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <String as serde::Deserialize>::deserialize(deserializer)?;
    Ok(canonical_provider_id(&raw))
}

#[derive(Deserialize)]
pub struct BackupRequest {
    #[serde(deserialize_with = "deserialize_provider")]
    pub provider: String,
}

#[derive(Deserialize)]
pub struct ScheduleRequest {
    #[serde(deserialize_with = "deserialize_provider")]
    pub provider: String,
    pub schedule: String,
}

#[derive(Serialize)]
pub struct ScheduleResponse {
    pub schedule: Option<String>,
    pub provider: Option<String>,
}

fn resolve_provider(
    provider_id: &str,
    pool: &PgPool,
) -> Result<Box<dyn backup::BackupProvider>, ApiError> {
    backup::get_provider(provider_id, pool).map_err(ApiError::bad_request)
}

const READ_REFERER_RULE: RefererRule = RefererRule::WhenPresent;

const WRITE_REFERER_RULE: RefererRule = RefererRule::Required;

pub async fn list_providers(
    State(state): State<AppState>,
) -> Result<Json<Vec<ProviderInfo>>, ApiError> {
    let metas = backup::list_providers();
    let mut result = Vec::with_capacity(metas.len());
    for meta in metas {
        // INVARIANT: Surface DB errors instead of silently treating them as "not connected" —
        // a transient DB failure must not be reported as "no OAuth account".
        // `provider_readiness` is shared with the agent's `get_backup_status`, so
        // the page and the agent cannot disagree about whether a provider works.
        let readiness = backup::provider_readiness(&state.pool, &meta)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let ready = readiness.ready();
        let folder_url = if ready {
            match backup::get_provider(meta.id, &state.pool) {
                Ok(provider) => {
                    tokio::time::timeout(std::time::Duration::from_secs(8), provider.folder_url())
                        .await
                        .ok()
                        .flatten()
                }
                Err(_) => None,
            }
        } else {
            None
        };
        result.push(ProviderInfo {
            id: meta.id,
            name: meta.name,
            connected: readiness.connected,
            ready,
            missing_scopes: readiness.missing_scopes,
            folder_url,
        });
    }
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct BackupKeyQuery {
    #[serde(default)]
    pub token: Option<String>,
}

async fn check_key_access(
    state: &AppState,
    headers: &HeaderMap,
    token: Option<&str>,
    rule: RefererRule,
) -> Result<MessageOrigin, ApiError> {
    if !reveal_request_allowed(headers, rule) {
        log_legacy!("[Backup] refused a backup-key read from an app document");
        let (status, message) = forbidden(KEY_SUBJECT_LABEL);
        return Err(ApiError::new(status, message));
    }
    crate::api::actor::require_attested_owner(headers, KEY_AGENT_REFUSAL)?;
    let actor = crate::api::actor::require_user_actor(headers, &state.pool, None).await?;
    if !state
        .reveal_tokens
        .redeem(token.unwrap_or_default(), RevealSubject::BackupKey)
    {
        log_legacy!("[Backup] refused a backup-key read: no live reveal token");
        let (status, message) = token_required(KEY_MINT_ROUTE);
        return Err(ApiError::new(status, message));
    }
    Ok(actor)
}

const KEY_AGENT_REFUSAL: &str = "An agent cannot reveal the backup key. It decrypts every \
     legacy archive, so only the workspace owner reads it, in Settings, System, Backup.";

async fn audit_key_revealed(state: &AppState, actor: MessageOrigin) {
    state
        .engine
        .event_bus
        .emit_or_log(
            crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::BackupKeyRevealed { actor: Some(actor) },
            ),
            "[Backup] BackupKeyRevealed",
        )
        .await;
}

pub(super) async fn mint_backup_key_reveal_token(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<RevealTokenResponse>, ApiError> {
    if !reveal_request_allowed(&headers, WRITE_REFERER_RULE) {
        log_legacy!("[Backup] refused a backup-key reveal-token mint from an app document");
        let (status, message) = forbidden(KEY_SUBJECT_LABEL);
        return Err(ApiError::new(status, message));
    }
    crate::api::actor::require_attested_owner(&headers, KEY_AGENT_REFUSAL)?;
    let token = state
        .reveal_tokens
        .mint(RevealSubject::BackupKey)
        .ok_or_else(|| ApiError::internal("reveal-token store is poisoned"))?;
    Ok(Json(RevealTokenResponse::new(token)))
}

/// INVARIANT: GET /api/v1/backup/key: reveal the legacy key, so the user can keep it for
/// the `.enc` archives it sealed. The switch screen shows it one last time.
/// Answers 404 when the workspace holds none. Nothing mints one any more.
///
/// The key decrypts every legacy archive this workspace uploaded. App UIs are
/// same-origin with the engine, so the route takes a one-shot token, refuses
/// an app `Referer`, and records the read. See `api::secret_reveal`.
pub async fn get_backup_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<BackupKeyQuery>,
) -> Result<Json<KeyResponse>, ApiError> {
    let actor =
        check_key_access(&state, &headers, query.token.as_deref(), READ_REFERER_RULE).await?;
    match BackupKeyStore::for_engine(&state.workspace_path).and_then(|keys| keys.load()) {
        Ok(Some(key)) => {
            audit_key_revealed(&state, actor).await;
            Ok(Json(KeyResponse {
                key: key.to_base64(),
            }))
        }
        Ok(None) => Err(ApiError::not_found(
            "This workspace holds no legacy backup key.",
        )),
        Err(e) => Err(ApiError::internal(format!(
            "Failed to read backup key: {e}"
        ))),
    }
}

async fn check_recipient_access(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<MessageOrigin, ApiError> {
    if !reveal_request_allowed(headers, WRITE_REFERER_RULE) {
        log_legacy!("[Backup] refused a backup recipient change from an app document");
        let (status, message) = forbidden(KEY_SUBJECT_LABEL);
        return Err(ApiError::new(status, message));
    }
    crate::api::actor::require_owner_device(
        headers,
        &state.pool,
        &state.engine.event_bus,
        None,
        "manage the backup key",
    )
    .await
}

fn parse_pasted_key(text: &str) -> Result<age::x25519::Identity, ApiError> {
    backup::crypto::parse_backup_key(text)
        .map_err(|e| ApiError::bad_request(e.to_string()).with_reason("not-a-backup-key"))
}

async fn backup_keys(state: &AppState) -> Result<BackupKeysResponse, ApiError> {
    let stored = recipient::load(&state.pool)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to read the backup recipient: {e}")))?;
    Ok(BackupKeysResponse {
        recipient: stored.recipient,
        pending: stored.pending_recipient.is_some(),
        legacy_key: BackupKeyStore::for_engine(&state.workspace_path)
            .is_ok_and(|keys| keys.exists()),
        secs_since_verified: stored.secs_since_verified,
    })
}

/// INVARIANT: GET /api/v1/backup/recipient: what seals this workspace's backups. Reveals
/// no key, so the page reads it on load.
pub async fn get_backup_recipient(
    State(state): State<AppState>,
) -> Result<Json<BackupKeysResponse>, ApiError> {
    backup_keys(&state).await.map(Json)
}

/// INVARIANT: POST /api/v1/backup/recipient/setup: mint a key pair and show its backup
/// key once. Backups keep their current key until the confirm below. A second
/// setup replaces a pending pair, which is how rotation starts.
pub async fn start_recipient_setup(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<BackupKeySetupResponse>, ApiError> {
    check_recipient_access(&state, &headers).await?;
    let backup_key = recipient::start_setup(&state.pool)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to create a backup key: {e}")))?;
    Ok(Json(BackupKeySetupResponse { backup_key }))
}

pub async fn confirm_backup_recipient(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<BackupKeyRequest>,
) -> Result<Json<BackupKeysResponse>, ApiError> {
    let actor = check_recipient_access(&state, &headers).await?;
    let pasted = parse_pasted_key(&req.backup_key)?;
    let check = recipient::confirm(&state.pool, &state.engine.event_bus, &pasted, Some(actor))
        .await
        .map_err(|e| ApiError::internal(format!("Failed to confirm the backup key: {e}")))?;
    match check {
        recipient::KeyCheck::Matches => {
            recipient::forget_legacy_key(&state.workspace_path);
            backup_keys(&state).await.map(Json)
        }
        recipient::KeyCheck::Differs => Err(ApiError::bad_request(
            "That is not the backup key shown. Paste the whole key you saved.",
        )
        .with_reason("key-mismatch")),
        recipient::KeyCheck::NothingToCheck => Err(ApiError::new(
            StatusCode::CONFLICT,
            "No backup key is waiting to be confirmed. Start the setup again.",
        )
        .with_reason("nothing-pending")),
    }
}

pub async fn set_own_backup_recipient(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<OwnRecipientRequest>,
) -> Result<Json<BackupKeysResponse>, ApiError> {
    let actor = check_recipient_access(&state, &headers).await?;
    let own = backup::crypto::parse_pasted_recipient(&req.recipient)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    recipient::use_own(&state.pool, &state.engine.event_bus, &own, Some(actor))
        .await
        .map_err(|e| ApiError::internal(format!("Failed to set the backup recipient: {e}")))?;
    recipient::forget_legacy_key(&state.workspace_path);
    backup_keys(&state).await.map(Json)
}

pub async fn verify_backup_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<BackupKeyRequest>,
) -> Result<Json<VerifyResponse>, ApiError> {
    check_recipient_access(&state, &headers).await?;
    let pasted = parse_pasted_key(&req.backup_key)?;
    match recipient::verify(&state.pool, &pasted)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to check the backup key: {e}")))?
    {
        recipient::KeyCheck::Matches => Ok(Json(VerifyResponse { matches: true })),
        recipient::KeyCheck::Differs => Ok(Json(VerifyResponse { matches: false })),
        recipient::KeyCheck::NothingToCheck => Err(ApiError::new(
            StatusCode::CONFLICT,
            "This workspace has no backup recipient to check against.",
        )
        .with_reason("nothing-to-check")),
    }
}

async fn require_sealing_key(state: &AppState) -> Result<backup::crypto::SealingKey, ApiError> {
    recipient::sealing_key(&state.pool, &state.workspace_path)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to load the backup key: {e}")))?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                crate::scheduler::backups_not_set_up_message(),
            )
            .with_reason("backups-not-set-up")
        })
}

async fn require_ready_provider(provider_id: &str, pool: &PgPool) -> Result<(), ApiError> {
    let meta = backup::provider_meta(provider_id)
        .ok_or_else(|| ApiError::bad_request(format!("Unknown backup provider: {provider_id}")))?;
    let readiness = backup::provider_readiness(pool, &meta)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if readiness.ready() {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::CONFLICT,
        format!("{} is not ready: grant access first", meta.name),
    )
    .with_reason("provider-not-ready"))
}

/// INVARIANT: Queues a backup and returns 202 immediately; terminal state arrives via
/// the `BackupCompleted` / `BackupFailed` SSE events. The backup pipeline can
/// run for many minutes, so a synchronous handler would race the frontend's
/// AbortController and discard the result.
pub async fn create_backup(
    State(state): State<AppState>,
    Json(req): Json<BackupRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let guard = crate::scheduler::BackupGuard::try_acquire(&state.engine).ok_or_else(|| {
        let slot = &state.engine.archive_job_slot;
        let message = if slot.is_running(crate::core::snapshot::ArchiveJobKind::Backup) {
            "Backup already in progress"
        } else {
            "A workspace snapshot job is running. Try the backup again when it finishes."
        };
        ApiError::new(StatusCode::CONFLICT, message)
    })?;

    let provider = resolve_provider(&req.provider, &state.pool)?;
    let key = require_sealing_key(&state).await?;
    require_ready_provider(&req.provider, &state.pool).await?;

    let engine = state.engine.clone();
    let pool = state.pool.clone();
    let workspace = state.workspace_path.clone();
    let database_url = crate::core::database_url();

    tokio::spawn(async move {
        crate::scheduler::run_backup(
            guard,
            &engine,
            &pool,
            &workspace,
            &database_url,
            key,
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
) -> Result<Json<Vec<backup::BackupEntry>>, ApiError> {
    let provider = resolve_provider(&params.provider, &state.pool)?;

    let entries = provider
        .list_backups()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list backups: {e}")))?;

    Ok(Json(entries))
}

#[derive(Serialize)]
pub struct BackupStatusResponse {
    pub running: bool,
    pub last_run: Option<backup::BackupLastRun>,
    pub latest_backup: Option<backup::BackupEntry>,
    pub age_seconds: Option<i64>,
    pub stale: bool,
    pub list_error: Option<String>,
}

fn build_backup_status(
    running: bool,
    last_run: Option<backup::BackupLastRun>,
    list_result: Result<Vec<backup::BackupEntry>, String>,
    workspace_name: &str,
    origin: Option<&backup::BackupOrigin>,
    now: chrono::DateTime<chrono::Utc>,
) -> BackupStatusResponse {
    let (latest_backup, list_error) = match list_result {
        Ok(entries) => (
            entries
                .into_iter()
                .filter(|e| {
                    origin.is_some_and(|origin| {
                        backup::is_own_backup_archive(&e.filename, workspace_name, origin)
                    })
                })
                .max_by_key(|e| e.created_at),
            None,
        ),
        Err(e) => (None, Some(e)),
    };

    let age_seconds = latest_backup
        .as_ref()
        .map(|b| (now - b.created_at).num_seconds());
    let stale = age_seconds.is_none_or(|age| age > backup::BACKUP_STALE_AFTER_SECONDS);

    BackupStatusResponse {
        running,
        last_run,
        latest_backup,
        age_seconds,
        stale,
        list_error,
    }
}

pub async fn get_backup_status(
    State(state): State<AppState>,
    Query(params): Query<BackupRequest>,
) -> Result<Json<BackupStatusResponse>, ApiError> {
    let provider = resolve_provider(&params.provider, &state.pool)?;

    let running = state
        .engine
        .archive_job_slot
        .is_running(crate::core::snapshot::ArchiveJobKind::Backup);
    let last_run = backup::load_last_run(&state.pool).await;
    let (origin, list_result) =
        match backup::read_backup_origin(&state.pool, &state.workspace_path).await {
            Ok(Some(origin)) => (
                Some(origin),
                provider.list_backups().await.map_err(|e| e.to_string()),
            ),
            Ok(None) => (None, Ok(Vec::new())),
            Err(e) => (None, Err(format!("Could not read the backup origin: {e}"))),
        };

    Ok(Json(build_backup_status(
        running,
        last_run,
        list_result,
        backup::workspace_archive_name(&state.workspace_path),
        origin.as_ref(),
        chrono::Utc::now(),
    )))
}

/// INVARIANT: One workspace's backup posture, in the three facts a LIST needs: when it last
/// backed up successfully, whether that is stale, and whether backups are set up
/// at all.
///
/// The gateway reads this for a running workspace when its row stream pokes.
/// It reports the answer as part of `WorkspaceStatus`, so the picker can put a
/// backup line on each row. It is therefore deliberately CHEAP: one indexed
/// event read and two preference reads, and never a call to the cloud provider.
/// That last part is what separates it from [`get_backup_status`], which lists
/// the provider's folder over the network for one workspace's Settings page.
#[derive(Serialize)]
pub struct LastSuccessfulBackupResponse {
    pub at: Option<chrono::DateTime<chrono::Utc>>,
    pub stale: bool,
    pub configured: bool,
    pub overdue: bool,
    pub overdue_at: Option<chrono::DateTime<chrono::Utc>>,
    pub stale_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn build_last_successful(
    at: Option<chrono::DateTime<chrono::Utc>>,
    configured: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> LastSuccessfulBackupResponse {
    let older_than = |limit: i64| at.is_none_or(|t| (now - t).num_seconds() > limit);
    LastSuccessfulBackupResponse {
        at,
        stale: older_than(backup::BACKUP_STALE_AFTER_SECONDS),
        configured,
        overdue: older_than(backup::BACKUP_OVERDUE_AFTER_SECONDS),
        overdue_at: at.map(|t| t + chrono::Duration::seconds(backup::BACKUP_OVERDUE_AFTER_SECONDS)),
        stale_at: at.map(|t| t + chrono::Duration::seconds(backup::BACKUP_STALE_AFTER_SECONDS)),
    }
}

pub async fn get_last_successful_backup(
    State(state): State<AppState>,
) -> Result<Json<LastSuccessfulBackupResponse>, ApiError> {
    read_last_successful_backup(&state).await.map(Json)
}

pub(super) async fn read_last_successful_backup(
    state: &AppState,
) -> Result<LastSuccessfulBackupResponse, ApiError> {
    // INVARIANT: Both reads propagate. A 500 leaves the gateway with no answer, so the row
    // stays silent, which is honest. Degrading either read to a default would
    // put "never backed up" on a workspace that backs up nightly.
    let at = backup::load_last_successful_backup(&state.pool)
        .await
        .map_err(|e| {
            ApiError::internal(format!("Failed to read the last successful backup: {e}"))
        })?;
    let configured = backup::backups_are_configured(&state.pool)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to read the backup schedule: {e}")))?;
    Ok(build_last_successful(at, configured, chrono::Utc::now()))
}

pub async fn get_schedule(
    State(state): State<AppState>,
) -> Result<Json<ScheduleResponse>, ApiError> {
    let cron = prefs::BACKUP_SCHEDULE
        .try_stored(&state.pool)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get schedule: {e}")))?;
    let provider = prefs::BACKUP_PROVIDER
        .try_stored(&state.pool)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get schedule: {e}")))?;

    Ok(Json(schedule_response(cron, provider)))
}

fn schedule_response(cron: Option<String>, provider: Option<String>) -> ScheduleResponse {
    let provider = provider.filter(|p| !p.is_empty());
    // INVARIANT: The same predicate the picker's row reads, so the two surfaces cannot
    // disagree about whether this install is set up.
    let will_run = backup::schedule_will_run(cron.as_deref(), provider.as_deref());
    ScheduleResponse {
        schedule: cron.filter(|_| will_run),
        provider,
    }
}

pub async fn set_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ScheduleRequest>,
) -> Result<Json<ScheduleResponse>, ApiError> {
    let _ = resolve_provider(&req.provider, &state.pool)?;

    // INVARIANT: A schedule never turns on before something can seal its backups.
    if backup::is_schedule_active(&req.schedule) {
        require_sealing_key(&state).await?;
    }

    let active = backup::is_schedule_active(&req.schedule);
    let cron = if active {
        Some(req.schedule.as_str())
    } else {
        None
    };

    // INVARIANT: `set_backup_schedule` writes through `PreferenceStore::set`, which
    // announces each key it touches. The handler used to hand-roll those emits
    // afterwards; moving them into the write path is what makes a second caller
    // of `set_backup_schedule` impossible to get wrong.
    let actor = crate::api::actor::user_actor(&headers, None);
    {
        let mut scheduler = state.scheduler.lock().await;
        scheduler
            .set_backup_schedule(cron, &req.provider, actor)
            .await
            .map_err(|e| ApiError::bad_request(format!("Failed to set schedule: {e}")))?;
    }

    // INVARIANT: Shaped by the same rule the GET uses, so a PUT reports back what was
    // actually written. The old version nulled BOTH fields for an inactive
    // schedule, which told a caller disabling the cron that the destination it
    // had just set did not exist.
    Ok(Json(schedule_response(
        Some(req.schedule),
        Some(req.provider),
    )))
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
) -> Result<Json<RetentionResponse>, ApiError> {
    // INVARIANT: Display only: showing the default beats failing the Settings page, and
    // nothing is deleted off this read. The prune caller in `scheduler::backup`
    // deliberately does NOT take the default here.
    let keep = backup::get_retention_count(&state.pool)
        .await
        .unwrap_or_else(|_| prefs::BACKUP_RETENTION.default_number() as usize);
    Ok(Json(RetentionResponse { keep }))
}

pub async fn set_retention(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RetentionRequest>,
) -> Result<Json<RetentionResponse>, ApiError> {
    let value = req.keep.to_string();
    crate::core::preference_catalog::validate(&prefs::BACKUP_RETENTION.spec, &value)
        .map_err(ApiError::bad_request)?;
    let actor = crate::api::actor::user_actor(&headers, None);
    PreferenceStore::set(
        &state.pool,
        &state.engine.event_bus,
        prefs::BACKUP_RETENTION.key(),
        &value,
        actor,
    )
    .await
    .map_err(|e| ApiError::internal(format!("Failed to save retention: {e}")))?;
    Ok(Json(RetentionResponse { keep: req.keep }))
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/backup", post(create_backup))
        .route("/backup/list", get(list_backups))
        .route("/backup/status", get(get_backup_status))
        .route("/backup/last-successful", get(get_last_successful_backup))
        .route("/backup/key", get(get_backup_key))
        .route(
            "/backup/key/reveal-token",
            post(mint_backup_key_reveal_token),
        )
        .route(
            "/backup/recipient",
            get(get_backup_recipient).put(set_own_backup_recipient),
        )
        .route("/backup/recipient/setup", post(start_recipient_setup))
        .route("/backup/recipient/confirm", post(confirm_backup_recipient))
        .route("/backup/recipient/verify", post(verify_backup_key))
        .route("/backup/providers", get(list_providers))
        .route("/backup/schedule", get(get_schedule).put(set_schedule))
        .route("/backup/retention", get(get_retention).put(set_retention))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_bus::EventBus;
    use chrono::{Duration, Utc};

    fn hdrs(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(value).unwrap(),
            );
        }
        h
    }

    #[test]
    fn each_key_route_runs_the_rule_its_transport_needs() {
        assert_eq!(
            READ_REFERER_RULE,
            RefererRule::WhenPresent,
            "a GET is re-issued by sw.js and may lose its Referer"
        );
        assert_eq!(
            WRITE_REFERER_RULE,
            RefererRule::Required,
            "sw.js returns early for a non-GET, so a POST always carries one"
        );

        let source = include_str!("backup.rs");
        let production = source
            .split_once("mod tests")
            .map(|(head, _)| head)
            .expect("this module's own test block");
        let literals: Vec<&str> = production
            .lines()
            .filter(|l| l.contains("RefererRule::"))
            .collect();
        assert_eq!(
            literals.len(),
            2,
            "a handler names its rule, never spells one inline: {literals:?}"
        );
        assert!(
            literals.iter().all(|l| l.contains("const ")),
            "the only rule literals are the two constants: {literals:?}"
        );
    }

    #[test]
    fn a_caller_presenting_no_referer_still_reads_the_backup_key() {
        assert!(
            reveal_request_allowed(&HeaderMap::new(), READ_REFERER_RULE),
            "a non-browser caller must still reach the key"
        );
        let browser_no_referer = hdrs(&[("sec-fetch-site", "same-origin")]);
        assert!(
            reveal_request_allowed(&browser_no_referer, READ_REFERER_RULE),
            "a browser sending no Referer must still reach the key"
        );
        assert!(
            !reveal_request_allowed(&browser_no_referer, WRITE_REFERER_RULE),
            "a browser hiding its Referer must not mint or change the recipient"
        );
        assert!(
            reveal_request_allowed(&HeaderMap::new(), WRITE_REFERER_RULE),
            "the CLI and the API e2e suite must still reach the write routes"
        );
    }

    #[test]
    fn the_workspace_shell_still_reaches_the_key_routes() {
        for referer in [
            "https://host/settings",
            "https://localhost:5251/",
            "https://localhost:5251/dev/",
        ] {
            let h = hdrs(&[("sec-fetch-site", "same-origin"), ("referer", referer)]);
            for rule in [READ_REFERER_RULE, WRITE_REFERER_RULE] {
                assert!(reveal_request_allowed(&h, rule), "{referer}");
            }
        }
    }

    const WS: &str = "myws";

    fn test_origin() -> backup::BackupOrigin {
        "012345abcdef".parse().unwrap()
    }

    fn entry(id: &str, age: Duration) -> backup::BackupEntry {
        entry_of(WS, id, age)
    }

    fn entry_of(workspace: &str, id: &str, age: Duration) -> backup::BackupEntry {
        backup::BackupEntry {
            id: id.to_string(),
            filename: format!("lucidos-backup-{workspace}-20260601-040254-o012345abcdef.age"),
            size_bytes: 1024,
            created_at: Utc::now() - age,
        }
    }

    #[test]
    fn an_app_document_cannot_reach_any_key_route() {
        for rule in [READ_REFERER_RULE, WRITE_REFERER_RULE] {
            for referer in [
                "https://host/app/demo-director/index.html",
                "https://localhost:5251/app/habit-tracker/",
                "https://localhost:5251/dev/app/habit-tracker/index.html",
            ] {
                let h = hdrs(&[("sec-fetch-site", "same-origin"), ("referer", referer)]);
                assert!(
                    !reveal_request_allowed(&h, rule),
                    "{referer} must not reach the key that decrypts every archive"
                );
            }
        }
    }

    #[test]
    fn the_refusal_names_the_route_this_module_serves() {
        let (status, body) = token_required(KEY_MINT_ROUTE);
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("/backup/key/reveal-token"), "{body}");
        assert!(
            KEY_MINT_ROUTE.starts_with("/api/v1/"),
            "the message quotes a full path, so the constant has to carry the mount"
        );
    }

    #[test]
    fn the_audit_row_carries_no_key_material() {
        use crate::engine::event_bus::SystemEvent;
        use crate::engine::thread_events::MessageOrigin;

        let fields = |event: &SystemEvent| -> Vec<String> {
            let payload = event.to_payload();
            assert_eq!(payload["type"], "BackupKeyRevealed");
            let mut keys: Vec<String> = payload["data"]
                .as_object()
                .unwrap_or_else(|| panic!("a data object: {payload}"))
                .keys()
                .cloned()
                .collect();
            keys.sort();
            keys
        };

        let anonymous = SystemEvent::BackupKeyRevealed { actor: None };
        assert_eq!(anonymous.event_type(), "BackupKeyRevealed");
        assert!(anonymous.is_persisted(), "an audit row has to be durable");
        assert!(fields(&anonymous).is_empty());

        let attributed = SystemEvent::BackupKeyRevealed {
            actor: Some(MessageOrigin::Device {
                device_id: "device-1".to_string(),
            }),
        };
        assert_eq!(fields(&attributed), vec!["actor".to_string()]);
    }

    #[test]
    fn provider_info_reports_the_missing_scopes_it_was_built_with() {
        let short = ProviderInfo {
            id: "dropbox",
            name: "Dropbox",
            connected: true,
            ready: false,
            missing_scopes: vec!["files.metadata.read"],
            folder_url: None,
        };
        let v = serde_json::to_value(&short).unwrap();
        assert_eq!(v["connected"], true);
        assert_eq!(v["ready"], false);
        assert_eq!(
            v["missing_scopes"],
            serde_json::json!(["files.metadata.read"])
        );

        for (connected, ready) in [(true, true), (false, false)] {
            let v = serde_json::to_value(ProviderInfo {
                id: "dropbox",
                name: "Dropbox",
                connected,
                ready,
                missing_scopes: Vec::new(),
                folder_url: None,
            })
            .unwrap();
            assert_eq!(v["missing_scopes"], serde_json::json!([]));
        }
    }

    #[test]
    fn the_picker_line_goes_stale_on_the_shared_threshold() {
        let now = Utc::now();
        let fresh = build_last_successful(Some(now - Duration::hours(3)), true, now);
        assert!(!fresh.stale);
        assert!(fresh.configured);

        let old = build_last_successful(
            Some(now - Duration::seconds(backup::BACKUP_STALE_AFTER_SECONDS + 60)),
            true,
            now,
        );
        assert!(old.stale, "past the shared threshold");
    }

    #[test]
    fn overdue_has_its_own_threshold_apart_from_stale() {
        let now = Utc::now();
        let cases = [
            (Duration::hours(23), false, false),
            (Duration::hours(25), true, false),
            (Duration::days(6), true, false),
            (Duration::days(8), true, true),
        ];
        for (age, stale, overdue) in cases {
            let s = build_last_successful(Some(now - age), true, now);
            assert_eq!(s.stale, stale, "stale at {age}");
            assert_eq!(s.overdue, overdue, "overdue at {age}");
        }
    }

    #[test]
    fn overdue_at_is_where_overdue_flips() {
        let now = Utc::now();
        let at = now - Duration::days(2);
        let s = build_last_successful(Some(at), true, now);
        let due = s.overdue_at.expect("a backup has an overdue instant");
        assert_eq!(
            due,
            at + Duration::seconds(backup::BACKUP_OVERDUE_AFTER_SECONDS)
        );
        assert!(!build_last_successful(Some(at), true, due).overdue);
        assert!(build_last_successful(Some(at), true, due + Duration::seconds(1)).overdue);
        assert!(build_last_successful(None, true, now).overdue_at.is_none());
    }

    #[test]
    fn stale_at_is_where_stale_flips() {
        let now = Utc::now();
        let at = now - Duration::hours(2);
        let s = build_last_successful(Some(at), true, now);
        let due = s.stale_at.expect("a backup has a stale instant");
        assert_eq!(
            due,
            at + Duration::seconds(backup::BACKUP_STALE_AFTER_SECONDS)
        );
        assert!(!build_last_successful(Some(at), true, due).stale);
        assert!(build_last_successful(Some(at), true, due + Duration::seconds(1)).stale);
        assert!(build_last_successful(None, true, now).stale_at.is_none());
    }

    #[test]
    fn never_backed_up_is_stale_whether_or_not_it_is_configured() {
        let now = Utc::now();
        for configured in [true, false] {
            let s = build_last_successful(None, configured, now);
            assert!(s.stale, "no backup at all is stale");
            assert!(s.overdue, "no backup at all is overdue");
            assert_eq!(s.configured, configured);
            let v = serde_json::to_value(&s).unwrap();
            assert_eq!(v["at"], serde_json::Value::Null);
        }
    }

    #[test]
    fn the_wire_carries_an_rfc_3339_timestamp() {
        let at = Utc::now();
        let v = serde_json::to_value(build_last_successful(Some(at), true, at)).unwrap();
        let s = v["at"].as_str().expect("a string timestamp");
        assert_eq!(
            chrono::DateTime::parse_from_rfc3339(s)
                .expect("parses")
                .timestamp(),
            at.timestamp(),
        );
    }

    fn sse_event_type(e: &crate::engine::event_bus::EmittedEvent) -> String {
        let v: serde_json::Value = serde_json::from_str(&e.to_sse_json()).unwrap();
        v["type"].as_str().unwrap_or_default().to_string()
    }

    #[tokio::test]
    async fn progress_sender_orders_progress_before_terminal_event() {
        use crate::engine::event_bus::{BusEvent, SystemEvent};

        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _parent_rx) = EventBus::new(pool.clone());
        let mut rx = bus.subscribe();

        let progress = progress_sender(bus.clone());
        progress("uploading", 100, 100);
        bus.emit_or_log(
            BusEvent::System(SystemEvent::BackupCompleted {
                filename: "lucidos-backup-test.enc".to_string(),
                size_bytes: 1024,
                started_at: chrono::Utc::now(),
                finished_at: chrono::Utc::now(),
            }),
            "[Backup] BackupCompleted",
        )
        .await;

        let first = sse_event_type(&rx.recv().await.expect("a progress event"));
        let second = sse_event_type(&rx.recv().await.expect("a terminal event"));
        assert_eq!(
            first, "BackupProgress",
            "progress must reach subscribers before the terminal event"
        );
        assert_eq!(second, "BackupCompleted");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[test]
    fn a_provider_arrives_in_either_spelling_and_resolves_to_one() {
        assert_eq!(canonical_provider_id("google-drive"), "google_drive");
        assert_eq!(canonical_provider_id("google_drive"), "google_drive");
        assert_eq!(canonical_provider_id("dropbox"), "dropbox");
        assert_eq!(canonical_provider_id("nextcloud"), "nextcloud");
        assert_eq!(canonical_provider_id(""), "");
    }

    #[test]
    fn both_request_shapes_take_the_kebab_spelling() {
        let backup: BackupRequest =
            serde_json::from_value(serde_json::json!({ "provider": "google-drive" }))
                .expect("the kebab spelling parses");
        assert_eq!(backup.provider, "google_drive");

        let schedule: ScheduleRequest = serde_json::from_value(
            serde_json::json!({ "provider": "google-drive", "schedule": "0 0 3 * * *" }),
        )
        .expect("the kebab spelling parses");
        assert_eq!(schedule.provider, "google_drive");
        assert_eq!(schedule.schedule, "0 0 3 * * *");
    }

    #[test]
    fn an_inactive_schedule_still_reports_the_configured_provider() {
        for cron in [Some("off".to_string()), Some(String::new()), None] {
            let r = schedule_response(cron.clone(), Some("dropbox".to_string()));
            assert_eq!(r.schedule, None, "cron {cron:?} is not an active schedule");
            assert_eq!(
                r.provider.as_deref(),
                Some("dropbox"),
                "cron {cron:?} must not erase the destination"
            );
        }
    }

    #[test]
    fn an_active_schedule_reports_both_halves() {
        let r = schedule_response(Some("0 0 3 * * *".to_string()), Some("dropbox".to_string()));
        assert_eq!(r.schedule.as_deref(), Some("0 0 3 * * *"));
        assert_eq!(r.provider.as_deref(), Some("dropbox"));
    }

    #[test]
    fn an_unconfigured_workspace_reports_neither() {
        let r = schedule_response(None, None);
        assert_eq!(r.schedule, None);
        assert_eq!(r.provider, None);
    }

    #[test]
    fn a_blank_provider_reads_as_unconfigured() {
        let r = schedule_response(Some("0 0 3 * * *".to_string()), Some(String::new()));
        assert_eq!(r.provider, None);
    }

    #[test]
    fn a_cron_with_no_destination_is_not_an_active_schedule() {
        for provider in [None, Some(String::new())] {
            let r = schedule_response(Some("0 0 3 * * *".to_string()), provider.clone());
            assert_eq!(
                r.schedule, None,
                "provider {provider:?} registers no job, so no schedule is reported"
            );
            assert_eq!(r.provider, None);
        }
    }

    #[test]
    fn build_status_mirrors_running_flag() {
        let now = Utc::now();
        let on = build_backup_status(true, None, Ok(vec![]), WS, Some(&test_origin()), now);
        assert!(on.running);
        let off = build_backup_status(false, None, Ok(vec![]), WS, Some(&test_origin()), now);
        assert!(!off.running);
    }

    #[test]
    fn build_status_picks_newest_and_not_stale_when_recent() {
        let now = Utc::now();
        let entries = vec![
            entry("old", Duration::hours(40)),
            entry("newest", Duration::hours(1)),
            entry("mid", Duration::hours(10)),
        ];
        let status = build_backup_status(false, None, Ok(entries), WS, Some(&test_origin()), now);
        let latest = status.latest_backup.expect("a latest backup");
        assert_eq!(latest.id, "newest");
        assert!(!status.stale, "a 1h-old backup is fresh");
        let age = status.age_seconds.expect("age");
        assert!((3000..4200).contains(&age), "≈1h in seconds, got {age}");
        assert!(status.list_error.is_none());
    }

    #[test]
    fn build_status_counts_only_this_workspaces_archives() {
        let now = Utc::now();
        let entries = vec![
            entry("mine", Duration::hours(30)),
            entry_of("myws-2", "sibling", Duration::hours(1)),
            entry_of("other", "other", Duration::hours(2)),
            backup::BackupEntry {
                filename: "lucidos-backup-myws-20260601-040254-oabcdef012345.age".into(),
                ..entry("foreign-copy", Duration::hours(1))
            },
            backup::BackupEntry {
                filename: "lucidos-backup-myws-20260601-040254.age".into(),
                ..entry("unmarked", Duration::hours(1))
            },
        ];
        let status = build_backup_status(false, None, Ok(entries), WS, Some(&test_origin()), now);
        assert_eq!(status.latest_backup.expect("own archive").id, "mine");
        assert!(status.stale, "this workspace's newest backup is 30h old");
    }

    #[test]
    fn build_status_stale_when_old() {
        let now = Utc::now();
        let status = build_backup_status(
            false,
            None,
            Ok(vec![entry("old", Duration::hours(30))]),
            WS,
            Some(&test_origin()),
            now,
        );
        assert!(status.stale);
        assert!(status.age_seconds.unwrap() > backup::BACKUP_STALE_AFTER_SECONDS);
    }

    #[test]
    fn build_status_stale_when_none() {
        let now = Utc::now();
        let status = build_backup_status(false, None, Ok(vec![]), WS, Some(&test_origin()), now);
        assert!(status.stale);
        assert!(status.latest_backup.is_none());
        assert!(status.age_seconds.is_none());
        assert!(status.list_error.is_none());
    }

    #[test]
    fn build_status_tolerates_list_error() {
        let now = Utc::now();
        let status = build_backup_status(
            false,
            Some(backup::BackupLastRun::failure("disk full", now)),
            Err("Drive unreachable".to_string()),
            WS,
            Some(&test_origin()),
            now,
        );
        assert!(status.latest_backup.is_none());
        assert_eq!(status.list_error.as_deref(), Some("Drive unreachable"));
        assert!(status.stale);
        assert!(matches!(
            status.last_run.unwrap().status,
            backup::BackupRunStatus::Failure
        ));
    }

    #[tokio::test]
    async fn enabling_a_schedule_announces_both_keys() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _parent_rx) = EventBus::new(pool.clone());

        PreferenceStore::set(
            &pool,
            &bus,
            prefs::BACKUP_SCHEDULE.key(),
            "0 0 3 * * *",
            None,
        )
        .await
        .unwrap();
        PreferenceStore::set(
            &pool,
            &bus,
            prefs::BACKUP_PROVIDER.key(),
            "google_drive",
            None,
        )
        .await
        .unwrap();

        let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
            "SELECT event_type, payload FROM events \
             WHERE event_type = 'PreferencesChanged' ORDER BY sequence",
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        assert_eq!(rows.len(), 2, "expected one row per written preference");
        let by_key: std::collections::HashMap<&str, &str> = rows
            .iter()
            .map(|(_, p)| {
                (
                    p["data"]["key"].as_str().unwrap(),
                    p["data"]["value"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            by_key.get(prefs::BACKUP_SCHEDULE.key()),
            Some(&"0 0 3 * * *")
        );
        assert_eq!(
            by_key.get(prefs::BACKUP_PROVIDER.key()),
            Some(&"google_drive")
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn disabling_a_schedule_announces_both_keys_too() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _parent_rx) = EventBus::new(pool.clone());

        PreferenceStore::set(&pool, &bus, prefs::BACKUP_SCHEDULE.key(), "off", None)
            .await
            .unwrap();
        PreferenceStore::set(&pool, &bus, prefs::BACKUP_PROVIDER.key(), "dropbox", None)
            .await
            .unwrap();

        let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
            "SELECT event_type, payload FROM events \
             WHERE event_type = 'PreferencesChanged' ORDER BY sequence",
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        assert_eq!(rows.len(), 2, "expected one row per written preference");
        let by_key: std::collections::HashMap<&str, &str> = rows
            .iter()
            .map(|(_, p)| {
                (
                    p["data"]["key"].as_str().unwrap(),
                    p["data"]["value"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(by_key.get(prefs::BACKUP_SCHEDULE.key()), Some(&"off"));
        assert_eq!(
            by_key.get(prefs::BACKUP_PROVIDER.key()),
            Some(&"dropbox"),
            "the destination survives a disable"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }
}
