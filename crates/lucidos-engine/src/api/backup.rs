use super::*;
use crate::core::backup::{self, crypto};
use crate::core::PreferenceStore;

#[derive(Serialize)]
pub struct ProviderInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// Whether an OAuth account exists for this provider.
    pub connected: bool,
    /// Whether connected AND the account's scopes contain the required scope.
    pub ready: bool,
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
        let backup::ProviderReadiness { connected, ready } =
            backup::provider_readiness(&state.pool, &meta)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?;
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
            connected,
            ready,
            folder_url,
        });
    }
    Ok(Json(result))
}

/// GET /api/v1/backup/key — reveal the EXISTING key. Read-only: returns 404 when
/// no key has been generated yet (the page then offers "Generate new backup
/// key"). It must NEVER mint a key as a side effect — the old behavior silently
/// generated one here, which orphaned prior backups (encrypted with the now-lost
/// key) and surfaced as a misleading "New backup key generated" toast when a
/// user only meant to view their key. Generation now lives behind the explicit
/// POST below.
pub async fn get_backup_key(State(state): State<AppState>) -> Result<Json<KeyResponse>, ApiError> {
    let key_path = backup::key_file_path(&state.workspace_path);
    match crypto::load_key_file(&key_path) {
        Ok(Some(key)) => Ok(Json(KeyResponse {
            key: crypto::key_to_base64(&key),
            is_new: false,
        })),
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
pub async fn generate_backup_key(
    State(state): State<AppState>,
) -> Result<Json<KeyResponse>, ApiError> {
    let (key, is_new) = crypto::ensure_key(&state.workspace_path)
        .map_err(|e| ApiError::internal(format!("Failed to generate backup key: {e}")))?;
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

/// A backup is "stale" once the newest cloud backup is older than this (or none
/// exists at all). 24h matches the most aggressive built-in schedule.
const BACKUP_STALE_AFTER_SECONDS: i64 = 24 * 60 * 60;

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
    let stale = age_seconds.is_none_or(|age| age > BACKUP_STALE_AFTER_SECONDS);

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
) -> Result<Json<RetentionResponse>, ApiError> {
    let keep = backup::get_retention_count(&state.pool).await;
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
        .route("/backup/key", get(get_backup_key).post(generate_backup_key))
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

    fn entry(id: &str, age: Duration) -> backup::BackupEntry {
        backup::BackupEntry {
            id: id.to_string(),
            filename: format!("lucidos-backup-{id}.enc"),
            size_bytes: 1024,
            created_at: Utc::now() - age,
        }
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
        assert!(status.age_seconds.unwrap() > BACKUP_STALE_AFTER_SECONDS);
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

    /// Disabling writes only `PREF_BACKUP_SCHEDULE = "off"` and leaves the
    /// provider preference untouched, so exactly one row appears: a provider
    /// row here would falsely suggest the user changed their backup
    /// destination.
    #[tokio::test]
    async fn disabling_a_schedule_announces_only_the_schedule_key() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _parent_rx) = EventBus::new(pool.clone());

        PreferenceStore::set(&pool, &bus, backup::PREF_BACKUP_SCHEDULE, "off", None)
            .await
            .unwrap();

        let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
            "SELECT event_type, payload FROM events \
             WHERE event_type = 'PreferencesChanged' ORDER BY sequence",
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        assert_eq!(rows.len(), 1, "expected only the schedule key to emit");
        assert_eq!(
            rows[0].1["data"]["key"].as_str().unwrap(),
            backup::PREF_BACKUP_SCHEDULE
        );
        assert_eq!(rows[0].1["data"]["value"].as_str().unwrap(), "off");

        crate::test_support::teardown_test_db(&db_name).await;
    }
}
