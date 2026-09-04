use super::secret_reveal::{
    forbidden, reveal_request_allowed, token_required, RefererRule, RevealSubject,
    RevealTokenResponse,
};
use super::*;
use crate::core::backup::{self, crypto};
use crate::core::PreferenceStore;
use crate::engine::thread_events::MessageOrigin;

/// The route that mints a backup-key reveal token, named in its own refusal.
const KEY_MINT_ROUTE: &str = "/api/v1/backup/key/reveal-token";

/// What the 403 calls the secret these routes guard.
const KEY_SUBJECT_LABEL: &str = "the backup key";

#[derive(Serialize)]
pub struct ProviderInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// Whether an OAuth account exists for this provider.
    pub connected: bool,
    /// Whether connected AND the account's scopes contain every required scope.
    pub ready: bool,
    /// Which required scopes a CONNECTED account is missing, in the provider's
    /// declared order. Empty for a ready provider and for one with no account.
    ///
    /// The page names them. "Access not granted" on its own cannot distinguish a
    /// grant that never happened from one that came back a scope short, which is
    /// the dead end a user hit after completing a Dropbox authorization and
    /// finding the same red line waiting for them.
    pub missing_scopes: Vec<&'static str>,
    /// Web URL to this provider's backups folder ("View backups folder" link), or
    /// null when the provider can't form one.
    pub folder_url: Option<String>,
}

#[derive(Serialize)]
pub struct KeyResponse {
    pub key: String,
    pub is_new: bool,
}

#[derive(Serialize)]
pub struct KeyExistsResponse {
    pub exists: bool,
}

#[derive(Deserialize)]
pub struct BackupRequest {
    pub provider: String,
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

pub(crate) fn progress_sender(
    event_bus: crate::engine::event_bus::EventBus,
) -> impl Fn(&str, usize, usize) + Send + Sync + 'static {
    move |phase: &str, current: usize, total: usize| {
        // Broadcast synchronously and in-order — NOT via tokio::spawn. A
        // detached task per tick let the final "uploading 100%" land on the
        // wire AFTER the terminal BackupCompleted/BackupFailed that run_backup
        // emits right after, re-setting the frontend's backupProgress signal the
        // terminal event had just cleared and wedging the Backup card on
        // "in progress". A sync send keeps progress strictly before the terminal
        // event and works from both async and spawn_blocking callers.
        event_bus.broadcast_transient_system(
            crate::engine::event_bus::SystemEvent::BackupProgress {
                phase: phase.to_string(),
                progress: current,
                total,
            },
        );
    }
}

fn resolve_provider(
    provider_id: &str,
    pool: &PgPool,
) -> Result<Box<dyn backup::BackupProvider>, ApiError> {
    backup::get_provider(provider_id, pool).map_err(ApiError::bad_request)
}

/// The `Referer` rule a READ of the key runs.
///
/// A `GET` may arrive with no `Referer`, because `public/sw.js` re-issues every
/// same-origin `GET /api/v1/*` except events, health and blobs. Refusing a
/// missing one would take the installed PWA's key read down on every load, and
/// close nothing: app JS reaches `window.top.fetch` and inherits the Settings
/// document's own `Referer` (ADR 0156). The token is what gates this step.
const READ_REFERER_RULE: RefererRule = RefererRule::WhenPresent;

/// The `Referer` rule the two `POST`s run: the mint, and the generate.
///
/// `sw.js` returns early for any non-GET, so a `POST` reaches the engine as the
/// browser sent it and always carries a `Referer`. A page that suppressed its
/// own has removed the one signal telling it apart from an app. That is the
/// credential mint's reasoning too (ADR 0117).
const WRITE_REFERER_RULE: RefererRule = RefererRule::Required;

pub async fn list_providers(
    State(state): State<AppState>,
) -> Result<Json<Vec<ProviderInfo>>, ApiError> {
    let metas = backup::list_providers();
    let mut result = Vec::with_capacity(metas.len());
    for meta in metas {
        // Surface DB errors instead of silently treating them as "not connected" —
        // a transient DB failure must not be reported as "no OAuth account".
        // `provider_readiness` is shared with the agent's `get_backup_status`, so
        // the page and the agent cannot disagree about whether a provider works.
        let readiness = backup::provider_readiness(&state.pool, &meta)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let ready = readiness.ready();
        // Best-effort folder link, computed only for a ready provider (it's the
        // only state the link is shown in). For Drive this resolves the folder id
        // live with one Drive lookup, so bound it — a slow or unreachable provider
        // must not stall the settings page; omit the link on timeout or error.
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

/// The token a key-bearing backup route has to be handed.
///
/// Optional in the SHAPE so a caller that omits it meets the handler's own
/// refusal, which names the route that mints one. Left required, axum answers a
/// 400 that says only "failed to deserialize query string".
#[derive(Deserialize)]
pub struct BackupKeyQuery {
    #[serde(default)]
    pub token: Option<String>,
}

/// Refuse this request unless it may see the backup key, and say who is asking.
///
/// Both key-bearing routes run it, so the two cannot drift apart. The `rule`
/// differs by method and the reasoning is ADR 0117's: a `GET` is re-issued by
/// the service worker on iOS and may lose its `Referer`, a `POST` is not.
///
/// The caller identifies itself BEFORE the reveal token is redeemed. A token is
/// one-shot, so refusing after the redeem would spend it on a request that
/// never saw the key.
async fn check_key_access(
    state: &AppState,
    headers: &HeaderMap,
    token: Option<&str>,
    rule: RefererRule,
) -> Result<MessageOrigin, ApiError> {
    if !reveal_request_allowed(headers, rule) {
        log!("[Backup] refused a backup-key read from an app document");
        let (status, message) = forbidden(KEY_SUBJECT_LABEL);
        return Err(ApiError::new(status, message));
    }
    let actor = crate::api::actor::require_user_actor(headers, &state.pool, None).await?;
    if !state
        .reveal_tokens
        .redeem(token.unwrap_or_default(), RevealSubject::BackupKey)
    {
        log!("[Backup] refused a backup-key read: no live reveal token");
        let (status, message) = token_required(KEY_MINT_ROUTE);
        return Err(ApiError::new(status, message));
    }
    Ok(actor)
}

/// Record that the key left the engine, naming the actor and never the key.
async fn audit_key_revealed(state: &AppState, actor: MessageOrigin, minted: bool) {
    state
        .engine
        .event_bus
        .emit_or_log(
            crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::BackupKeyRevealed {
                    minted,
                    actor: Some(actor),
                },
            ),
            "[Backup] BackupKeyRevealed",
        )
        .await;
}

/// POST /api/v1/backup/key/reveal-token: mint a one-shot token for the two
/// routes below.
///
/// Step one of two, matching the credential reveal (ADR 0117). It takes no id:
/// a workspace has exactly one backup key. It does not check that a key exists,
/// because the generate route spends a token precisely when one does not.
pub(super) async fn mint_backup_key_reveal_token(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<RevealTokenResponse>, ApiError> {
    if !reveal_request_allowed(&headers, WRITE_REFERER_RULE) {
        log!("[Backup] refused a backup-key reveal-token mint from an app document");
        let (status, message) = forbidden(KEY_SUBJECT_LABEL);
        return Err(ApiError::new(status, message));
    }
    let token = state
        .reveal_tokens
        .mint(RevealSubject::BackupKey)
        .ok_or_else(|| ApiError::internal("reveal-token store is poisoned"))?;
    Ok(Json(RevealTokenResponse::new(token)))
}

/// GET /api/v1/backup/key — reveal the EXISTING key. Read-only: returns 404 when
/// no key has been generated yet (the page then offers "Generate new backup
/// key"). It must NEVER mint a key as a side effect — the old behavior silently
/// generated one here, which orphaned prior backups (encrypted with the now-lost
/// key) and surfaced as a misleading "New backup key generated" toast when a
/// user only meant to view their key. Generation now lives behind the explicit
/// POST below.
///
/// The key decrypts every archive this workspace uploaded. App UIs are
/// same-origin with the engine, so this route sat one bare GET away from any
/// installed app. It now takes a one-shot token, refuses an app `Referer`, and
/// records the read. See `api::secret_reveal`.
pub async fn get_backup_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<BackupKeyQuery>,
) -> Result<Json<KeyResponse>, ApiError> {
    let actor =
        check_key_access(&state, &headers, query.token.as_deref(), READ_REFERER_RULE).await?;
    let key_path = backup::key_file_path(&state.workspace_path);
    match crypto::load_key_file(&key_path) {
        Ok(Some(key)) => {
            audit_key_revealed(&state, actor, false).await;
            Ok(Json(KeyResponse {
                key: crypto::key_to_base64(&key),
                is_new: false,
            }))
        }
        Ok(None) => Err(ApiError::not_found(
            "No backup key exists yet. Generate one to enable encrypted backups.",
        )),
        Err(e) => Err(ApiError::internal(format!(
            "Failed to read backup key: {e}"
        ))),
    }
}

/// POST /api/v1/backup/key — generate the key if absent, then return it. This is
/// the only user-facing path that mints a key (the backup paths also mint via
/// `ensure_key`). Idempotent: if a key already exists it's returned unchanged
/// with `is_new: false`, so a double-click — or a race with a scheduled backup —
/// can never overwrite the key that protects existing backups.
///
/// That idempotence is why this route is gated exactly like the GET: on a
/// workspace that already has a key, it hands back the same plaintext. It takes
/// the strict `Referer` rule because a `POST` reaches the engine unmediated, so
/// a browser here always has a `Referer` to present.
pub async fn generate_backup_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<BackupKeyQuery>,
) -> Result<Json<KeyResponse>, ApiError> {
    let actor =
        check_key_access(&state, &headers, query.token.as_deref(), WRITE_REFERER_RULE).await?;
    let (key, is_new) = crypto::ensure_key(&state.workspace_path)
        .map_err(|e| ApiError::internal(format!("Failed to generate backup key: {e}")))?;
    audit_key_revealed(&state, actor, is_new).await;
    Ok(Json(KeyResponse {
        key: crypto::key_to_base64(&key),
        is_new,
    }))
}

/// GET /api/v1/backup/key/exists — whether a key is already on disk, WITHOUT
/// revealing it. The page calls this on load to label its button correctly
/// ("Show backup key" vs "Generate new backup key") without pulling the secret
/// into the page or minting one.
pub async fn backup_key_exists(State(state): State<AppState>) -> Json<KeyExistsResponse> {
    Json(KeyExistsResponse {
        exists: crypto::key_exists(&state.workspace_path),
    })
}

/// Queues a backup and returns 202 immediately; terminal state arrives via
/// the `BackupCompleted` / `BackupFailed` SSE events. The backup pipeline can
/// run for many minutes, so a synchronous handler would race the frontend's
/// AbortController and discard the result.
pub async fn create_backup(
    State(state): State<AppState>,
    Json(req): Json<BackupRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let guard = crate::scheduler::BackupGuard::try_acquire(&state.engine)
        .ok_or_else(|| ApiError::new(StatusCode::CONFLICT, "Backup already in progress"))?;

    // Validate sync — guard drops on early return so the flag is released.
    let provider = resolve_provider(&req.provider, &state.pool)?;
    let (key, _) = crypto::ensure_key(&state.workspace_path)
        .map_err(|e| ApiError::internal(format!("Failed to get backup key: {e}")))?;

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
) -> Result<Json<Vec<backup::BackupEntry>>, ApiError> {
    let provider = resolve_provider(&params.provider, &state.pool)?;

    let entries = provider
        .list_backups()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list backups: {e}")))?;

    Ok(Json(entries))
}

/// Aggregated backup health for the Settings → Backup page. Answers: is a backup
/// running right now, did the last run succeed or fail (and when), and how old is
/// the last good cloud backup. Survives engine restarts because `last_run` and
/// `latest_backup` come from persisted state, not ephemeral SSE.
#[derive(Serialize)]
pub struct BackupStatusResponse {
    /// True while a backup is in progress (mirrors `engine.backup_in_progress`).
    pub running: bool,
    /// Persisted outcome of the last run, or null if never recorded.
    pub last_run: Option<backup::BackupLastRun>,
    /// Newest backup the provider holds — the authoritative "last good cloud
    /// backup". Null if there are none or the provider couldn't be listed.
    pub latest_backup: Option<backup::BackupEntry>,
    /// Age of `latest_backup` in seconds, or null if none.
    pub age_seconds: Option<i64>,
    /// True when there's no recent good backup (none, or older than 24h).
    pub stale: bool,
    /// Set when listing the provider failed, so the page can still render
    /// running/last_run while surfacing that the cloud list is unavailable.
    pub list_error: Option<String>,
}

/// Pure assembly of the status response from its inputs. Split out so the
/// staleness/sorting/error-tolerance logic is unit-testable without an engine
/// or a live provider (mirrors `emit_schedule_preferences_changed`).
fn build_backup_status(
    running: bool,
    last_run: Option<backup::BackupLastRun>,
    list_result: Result<Vec<backup::BackupEntry>, String>,
    now: chrono::DateTime<chrono::Utc>,
) -> BackupStatusResponse {
    let (latest_backup, list_error) = match list_result {
        // The provider's order is not guaranteed (Drive returns by file id, not
        // creation time), so pick the newest explicitly rather than trusting [0].
        Ok(entries) => (entries.into_iter().max_by_key(|e| e.created_at), None),
        Err(e) => (None, Some(e)),
    };

    let age_seconds = latest_backup
        .as_ref()
        .map(|b| (now - b.created_at).num_seconds());
    // No backup at all is the worst kind of stale (engine down at cron time).
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

/// GET /api/v1/backup/status?provider=<id> — backup health for the page.
pub async fn get_backup_status(
    State(state): State<AppState>,
    Query(params): Query<BackupRequest>,
) -> Result<Json<BackupStatusResponse>, ApiError> {
    let provider = resolve_provider(&params.provider, &state.pool)?;

    let running = state
        .engine
        .backup_in_progress
        .load(std::sync::atomic::Ordering::SeqCst);
    let last_run = backup::load_last_run(&state.pool).await;
    // Tolerate a list failure (e.g. Drive unreachable): the page must still
    // render running/last_run, just without the latest-backup line.
    let list_result = provider.list_backups().await.map_err(|e| e.to_string());

    Ok(Json(build_backup_status(
        running,
        last_run,
        list_result,
        chrono::Utc::now(),
    )))
}

/// One workspace's backup posture, in the three facts a LIST needs: when it last
/// backed up successfully, whether that is stale, and whether backups are set up
/// at all.
///
/// The gateway polls this for every running workspace on its 2s supervise pass.
/// It reports the answer as part of `WorkspaceStatus`, so the picker can put a
/// backup line on each row. It is therefore deliberately CHEAP: one indexed
/// event read and two preference reads, and never a call to the cloud provider.
/// That last part is what separates it from [`get_backup_status`], which lists
/// the provider's folder over the network for one workspace's Settings page.
#[derive(Serialize)]
pub struct LastSuccessfulBackupResponse {
    /// When the newest successful run finished, or null when there has never
    /// been one.
    pub at: Option<chrono::DateTime<chrono::Utc>>,
    /// True when there is no successful backup at all, or the newest one is
    /// older than `backup::BACKUP_STALE_AFTER_SECONDS`.
    pub stale: bool,
    /// True when a destination AND an active schedule are both set. It is what
    /// separates a workspace whose backups are set up but have never produced an
    /// archive from one where nobody set them up.
    pub configured: bool,
}

/// Pure assembly of the response, so the staleness arithmetic is unit-testable
/// without a pool. Mirrors [`build_backup_status`].
fn build_last_successful(
    at: Option<chrono::DateTime<chrono::Utc>>,
    configured: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> LastSuccessfulBackupResponse {
    let stale = at.is_none_or(|t| (now - t).num_seconds() > backup::BACKUP_STALE_AFTER_SECONDS);
    LastSuccessfulBackupResponse {
        at,
        stale,
        configured,
    }
}

/// GET /api/v1/backup/last-successful: the per-workspace backup line the
/// gateway's picker draws.
///
/// Takes no `provider`, unlike the rest of this surface. The question is about
/// this workspace's own run history, not about a destination.
pub async fn get_last_successful_backup(
    State(state): State<AppState>,
) -> Result<Json<LastSuccessfulBackupResponse>, ApiError> {
    // Both reads propagate. A 500 leaves the gateway with no answer, so the row
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
    Ok(Json(build_last_successful(
        at,
        configured,
        chrono::Utc::now(),
    )))
}

pub async fn get_schedule(
    State(state): State<AppState>,
) -> Result<Json<ScheduleResponse>, ApiError> {
    // Read directly from preferences — no scheduler lock needed
    let cron = PreferenceStore::get(&state.pool, backup::PREF_BACKUP_SCHEDULE)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get schedule: {e}")))?;
    let provider = PreferenceStore::get(&state.pool, backup::PREF_BACKUP_PROVIDER)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get schedule: {e}")))?;

    Ok(Json(schedule_response(cron, provider)))
}

/// Shape a `(backup_schedule, backup_provider)` preference pair for the wire.
///
/// The two fields answer different questions, and the dependency between them
/// runs ONE WAY:
///
/// * `provider` is the CONFIGURED DESTINATION, reported whatever the schedule
///   says, because a destination does not stop existing when the cron is off.
/// * `schedule` is the cron that WILL ACTUALLY RUN, which needs an active
///   expression AND a destination. `reload_backup_schedule` removes the job
///   outright when the provider is unset, so a cron reported without one would
///   promise a backup the engine has not registered.
///
/// The pair used to collapse in both directions: any inactive schedule returned
/// `{schedule: null, provider: null}`, which meant the Backup page could not
/// learn which provider it was configured for and fell back to the first entry
/// in the registry. An install configured for Dropbox rendered every control on
/// the page against Google Drive. Only the provider half of that collapse was
/// wrong; keeping the other half is what holds this in step with
/// `reload_backup_schedule` and with the frontend's `backupIsActive`, which
/// also requires both.
fn schedule_response(cron: Option<String>, provider: Option<String>) -> ScheduleResponse {
    let provider = provider.filter(|p| !p.is_empty());
    // The same predicate the picker's row reads, so the two surfaces cannot
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
    // Validate provider exists
    let _ = resolve_provider(&req.provider, &state.pool)?;

    // Ensure a backup key exists before enabling a schedule
    if backup::is_schedule_active(&req.schedule) {
        crypto::ensure_key(&state.workspace_path)
            .map_err(|e| ApiError::internal(format!("Failed to ensure backup key: {e}")))?;
    }

    let active = backup::is_schedule_active(&req.schedule);
    let cron = if active {
        Some(req.schedule.as_str())
    } else {
        None
    };

    // `set_backup_schedule` writes through `PreferenceStore::set`, which
    // announces each key it touches. The handler used to hand-roll those emits
    // afterwards; moving them into the write path is what makes a second caller
    // of `set_backup_schedule` impossible to get wrong.
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    {
        let mut scheduler = state.scheduler.lock().await;
        scheduler
            .set_backup_schedule(cron, &req.provider, actor)
            .await
            .map_err(|e| ApiError::bad_request(format!("Failed to set schedule: {e}")))?;
    }

    // Shaped by the same rule the GET uses, so a PUT reports back what was
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
    // Display only: showing the default beats failing the Settings page, and
    // nothing is deleted off this read. The prune caller in `scheduler::backup`
    // deliberately does NOT take the default here.
    let keep = backup::get_retention_count(&state.pool)
        .await
        .unwrap_or(backup::DEFAULT_BACKUP_RETENTION);
    Ok(Json(RetentionResponse { keep }))
}

pub async fn set_retention(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RetentionRequest>,
) -> Result<Json<RetentionResponse>, ApiError> {
    if req.keep == 0 {
        return Err(ApiError::bad_request("Must keep at least 1 backup"));
    }
    let value = req.keep.to_string();
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    PreferenceStore::set(
        &state.pool,
        &state.engine.event_bus,
        backup::PREF_BACKUP_RETENTION,
        &value,
        actor,
    )
    .await
    .map_err(|e| ApiError::internal(format!("Failed to save retention: {e}")))?;
    Ok(Json(RetentionResponse { keep: req.keep }))
}

/// Routes for the `/backup*` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/backup", post(create_backup))
        .route("/backup/list", get(list_backups))
        .route("/backup/status", get(get_backup_status))
        .route("/backup/last-successful", get(get_last_successful_backup))
        .route("/backup/key", get(get_backup_key).post(generate_backup_key))
        .route(
            "/backup/key/reveal-token",
            post(mint_backup_key_reveal_token),
        )
        .route("/backup/key/exists", get(backup_key_exists))
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

    /// Which `RefererRule` each route runs is the whole gate, and nothing
    /// pinned it. It flipped `WhenPresent` to `Required` and back inside one
    /// hardening cycle with no test going red either way, which is how the
    /// over-correction got in. The e2e case sends no origin headers at all, so
    /// it passes under either rule and cannot tell them apart.
    ///
    /// The rules are named constants for exactly that reason. Asserting on the
    /// predicate alone repeats the blind spot, because it passes whichever rule
    /// the handler went on to use.
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

        // Naming the rules only pins them while the handlers READ the names. An
        // inline `RefererRule::…` at a call site would flip a route with both
        // assertions above still green, which is the original blind spot.
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

    /// The reason `Required` was reverted on the read. The service worker
    /// re-issues every same-origin `GET /api/v1/*` except events, health and
    /// blobs, so the installed PWA can present no `Referer` at all. A
    /// non-browser caller (the CLI, the API e2e suite) sends none either.
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
        // The write side is stricter, and only a browser is held to it: a
        // caller with no fetch metadata at all is the CLI, not a page.
        assert!(
            !reveal_request_allowed(&browser_no_referer, WRITE_REFERER_RULE),
            "a browser hiding its Referer must not mint or generate"
        );
        assert!(
            reveal_request_allowed(&HeaderMap::new(), WRITE_REFERER_RULE),
            "the CLI and the API e2e suite must still generate"
        );
    }

    /// The Settings page reaches every key route, direct and behind the
    /// gateway. `demo-director` is an app id, so the same shape one segment
    /// over is the refusal below.
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

    fn entry(id: &str, age: Duration) -> backup::BackupEntry {
        backup::BackupEntry {
            id: id.to_string(),
            filename: format!("lucidos-backup-{id}.enc"),
            size_bytes: 1024,
            created_at: Utc::now() - age,
        }
    }

    /// Regression: the backup key was one bare GET away from any installed app.
    ///
    /// App UIs are same-origin with the engine, so `Sec-Fetch-Site` reads
    /// `same-origin` for them exactly as for the Settings page. The `Referer`
    /// is the only thing that differs, and every key route now reads it.
    #[test]
    fn an_app_document_cannot_reach_any_key_route() {
        // Both rules, because the mint, the read and the generate between them
        // use both. No route can be the one that forgets.
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

    /// The refusal names the route that mints, and that route is the one the
    /// router registers. A 403 pointing at a path nobody serves is a dead end.
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

    /// The audit row records the act and the actor, never the key. Same
    /// secrecy contract as `CredentialRevealed`.
    ///
    /// Asserted as the EXACT field set rather than by searching the payload for
    /// the word "key". A substring search over a variant already named
    /// `BackupKeyRevealed` answers on capitalisation, so it would pass whatever
    /// a future field carried.
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

        for minted in [true, false] {
            let event = SystemEvent::BackupKeyRevealed {
                minted,
                actor: None,
            };
            assert_eq!(event.event_type(), "BackupKeyRevealed");
            assert!(event.is_persisted(), "an audit row has to be durable");
            assert_eq!(fields(&event), vec!["minted".to_string()]);
            assert_eq!(event.to_payload()["data"]["minted"], minted);
        }

        // With an actor the row grows exactly one field, so the timeline can
        // attribute the read and still nothing else rides along.
        let attributed = SystemEvent::BackupKeyRevealed {
            minted: false,
            actor: Some(MessageOrigin::Device {
                device_id: "device-1".to_string(),
                label: "My MacBook".to_string(),
            }),
        };
        assert_eq!(
            fields(&attributed),
            vec!["actor".to_string(), "minted".to_string()]
        );
    }

    /// The page and the agent both read `missing_scopes` off this response, so
    /// the field has to survive serialization under that exact name and has to
    /// be present (as `[]`) rather than skipped when there is no gap: an absent
    /// key is what the frontend's nullish guard has to treat as "an engine too
    /// old to answer".
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

        // Ready, and not connected, both report an empty list rather than
        // omitting the key or listing every scope.
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

    /// The picker's line reads a boolean, so the staleness threshold lives in
    /// exactly one place and both surfaces sit on the same side of it.
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

    /// Never backed up is the worst kind of stale, and it is NOT the same fact
    /// as "nobody set backups up". The picker says a different sentence for
    /// each, so both fields travel.
    #[test]
    fn never_backed_up_is_stale_whether_or_not_it_is_configured() {
        let now = Utc::now();
        for configured in [true, false] {
            let s = build_last_successful(None, configured, now);
            assert!(s.stale, "no backup at all is stale");
            assert_eq!(s.configured, configured);
            let v = serde_json::to_value(&s).unwrap();
            assert_eq!(v["at"], serde_json::Value::Null);
        }
    }

    /// The timestamp goes out as RFC 3339, which is what the gateway parses and
    /// what the picker hands to `new Date()`.
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

    /// Pull the SSE `type` discriminator off a broadcast event via its public
    /// wire form — avoids reaching into the typed enum from the test.
    fn sse_event_type(e: &crate::engine::event_bus::EmittedEvent) -> String {
        let v: serde_json::Value = serde_json::from_str(&e.to_sse_json()).unwrap();
        v["type"].as_str().unwrap_or_default().to_string()
    }

    /// A backup's progress ticks must reach SSE subscribers BEFORE the terminal
    /// `BackupCompleted` that `run_backup` emits immediately after the upload
    /// finishes. The original `progress_sender` spawned a detached task per
    /// tick, so the final "uploading 100%" tick could be delivered AFTER
    /// `BackupCompleted` — re-setting the frontend's `backupProgress` signal the
    /// completion had just cleared, wedging the Settings → Backup card on
    /// "Backup in progress — Uploading…" long after the backup had finished.
    #[tokio::test]
    async fn progress_sender_orders_progress_before_terminal_event() {
        use crate::engine::event_bus::{BusEvent, SystemEvent};

        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _parent_rx) = EventBus::new(pool.clone());
        let mut rx = bus.subscribe();

        // Mirror run_backup's tail: a final upload progress tick, then the
        // inline terminal completion emit.
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

    /// The reported regression: with the schedule off, the configured
    /// destination still has to reach the caller. Nulling it is what left the
    /// Backup page with nothing to seed its provider dropdown from, so it fell
    /// back to the first registry entry and rendered Google Drive on an install
    /// configured for Dropbox.
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

    /// The active case is unchanged: both halves reported.
    #[test]
    fn an_active_schedule_reports_both_halves() {
        let r = schedule_response(Some("0 0 3 * * *".to_string()), Some("dropbox".to_string()));
        assert_eq!(r.schedule.as_deref(), Some("0 0 3 * * *"));
        assert_eq!(r.provider.as_deref(), Some("dropbox"));
    }

    /// Nothing configured stays null on both halves, so a fresh workspace does
    /// not read as having a destination it never picked.
    #[test]
    fn an_unconfigured_workspace_reports_neither() {
        let r = schedule_response(None, None);
        assert_eq!(r.schedule, None);
        assert_eq!(r.provider, None);
    }

    /// A blank stored provider is "not configured", not a provider named "".
    /// It would otherwise seed the page with an id matching nothing in the
    /// registry, leaving every provider-scoped control disabled with no hint
    /// why.
    #[test]
    fn a_blank_provider_reads_as_unconfigured() {
        let r = schedule_response(Some("0 0 3 * * *".to_string()), Some(String::new()));
        assert_eq!(r.provider, None);
    }

    /// The decoupling runs ONE WAY. A cron with no destination is not a
    /// schedule that runs: `reload_backup_schedule` removes the job when the
    /// provider is unset, so reporting the cron would show the Backup page
    /// "Daily (03:00)" for a workspace where nothing is registered, and would
    /// contradict the reminder banner's `backupIsActive`, which requires both.
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

    /// `running` must pass straight through to the response in both states —
    /// the handler feeds it the `engine.backup_in_progress` AtomicBool.
    #[test]
    fn build_status_mirrors_running_flag() {
        let now = Utc::now();
        let on = build_backup_status(true, None, Ok(vec![]), now);
        assert!(on.running);
        let off = build_backup_status(false, None, Ok(vec![]), now);
        assert!(!off.running);
    }

    /// Newest entry wins regardless of list order, and a recent backup is not
    /// stale; age is measured from the entry's creation time.
    #[test]
    fn build_status_picks_newest_and_not_stale_when_recent() {
        let now = Utc::now();
        let entries = vec![
            entry("old", Duration::hours(40)),
            entry("newest", Duration::hours(1)),
            entry("mid", Duration::hours(10)),
        ];
        let status = build_backup_status(false, None, Ok(entries), now);
        let latest = status.latest_backup.expect("a latest backup");
        assert_eq!(latest.id, "newest");
        assert!(!status.stale, "a 1h-old backup is fresh");
        let age = status.age_seconds.expect("age");
        assert!((3000..4200).contains(&age), "≈1h in seconds, got {age}");
        assert!(status.list_error.is_none());
    }

    /// A backup older than 24h is stale.
    #[test]
    fn build_status_stale_when_old() {
        let now = Utc::now();
        let status = build_backup_status(
            false,
            None,
            Ok(vec![entry("old", Duration::hours(30))]),
            now,
        );
        assert!(status.stale);
        assert!(status.age_seconds.unwrap() > backup::BACKUP_STALE_AFTER_SECONDS);
    }

    /// No backups at all → stale with null latest/age. This is the "engine was
    /// down at cron time, nothing was ever uploaded" case.
    #[test]
    fn build_status_stale_when_none() {
        let now = Utc::now();
        let status = build_backup_status(false, None, Ok(vec![]), now);
        assert!(status.stale);
        assert!(status.latest_backup.is_none());
        assert!(status.age_seconds.is_none());
        assert!(status.list_error.is_none());
    }

    /// A provider/list failure must not 500 the status: latest is null, the
    /// error surfaces in `list_error`, and the page falls back to "stale".
    #[test]
    fn build_status_tolerates_list_error() {
        let now = Utc::now();
        let status = build_backup_status(
            false,
            Some(backup::BackupLastRun::failure("disk full", now)),
            Err("Drive unreachable".to_string()),
            now,
        );
        assert!(status.latest_backup.is_none());
        assert_eq!(status.list_error.as_deref(), Some("Drive unreachable"));
        assert!(status.stale);
        // last_run still flows through even when the cloud list is unavailable.
        assert!(matches!(
            status.last_run.unwrap().status,
            backup::BackupRunStatus::Failure
        ));
    }

    /// Enabling a schedule writes two preference keys (cron + provider) and
    /// each must announce, because the scheduler's own `PreferencesChanged`
    /// subscriber is what re-registers the backup cron and the Settings page is
    /// what reloads on it.
    ///
    /// Drives `PreferenceStore::set` directly, which is exactly what
    /// `TaskScheduler::set_backup_schedule` calls: the announcement used to be
    /// hand-rolled in the axum handler after the fact, and moving it into the
    /// write path is what makes a second caller of `set_backup_schedule`
    /// impossible to get wrong. Keeping the assertion here means it does not
    /// need a booted scheduler.
    #[tokio::test]
    async fn enabling_a_schedule_announces_both_keys() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _parent_rx) = EventBus::new(pool.clone());

        PreferenceStore::set(
            &pool,
            &bus,
            backup::PREF_BACKUP_SCHEDULE,
            "0 0 3 * * *",
            None,
        )
        .await
        .unwrap();
        PreferenceStore::set(
            &pool,
            &bus,
            backup::PREF_BACKUP_PROVIDER,
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
            by_key.get(backup::PREF_BACKUP_SCHEDULE),
            Some(&"0 0 3 * * *")
        );
        assert_eq!(
            by_key.get(backup::PREF_BACKUP_PROVIDER),
            Some(&"google_drive")
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Disabling announces BOTH keys, because it writes both: the schedule goes
    /// to `"off"` and the destination is written unchanged rather than skipped.
    ///
    /// It used to write only the schedule, and this test used to assert exactly
    /// that, on the reasoning that a provider row "would falsely suggest the
    /// user changed their backup destination". The opposite turned out to be
    /// true: skipping the write meant the destination could not be set at all
    /// while the schedule was off, since `PUT /backup/schedule` is the only
    /// route to `backup_provider` and it took the disable branch.
    ///
    /// Like its sibling above, this drives `PreferenceStore::set` directly to
    /// mirror what `set_backup_schedule`'s disable branch writes, because
    /// calling that method needs a live `SchedulerManager` (a `JobScheduler`
    /// plus a `SharedEngine`). It therefore pins the ANNOUNCEMENT contract, not
    /// the branch; the branch itself is covered end to end by
    /// `backup_schedule_test` in the API e2e suite, which asserts a disable
    /// leaves the destination readable.
    #[tokio::test]
    async fn disabling_a_schedule_announces_both_keys_too() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _parent_rx) = EventBus::new(pool.clone());

        PreferenceStore::set(&pool, &bus, backup::PREF_BACKUP_SCHEDULE, "off", None)
            .await
            .unwrap();
        PreferenceStore::set(&pool, &bus, backup::PREF_BACKUP_PROVIDER, "dropbox", None)
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
        assert_eq!(by_key.get(backup::PREF_BACKUP_SCHEDULE), Some(&"off"));
        assert_eq!(
            by_key.get(backup::PREF_BACKUP_PROVIDER),
            Some(&"dropbox"),
            "the destination survives a disable"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }
}
