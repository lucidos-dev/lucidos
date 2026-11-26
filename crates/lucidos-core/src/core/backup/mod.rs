pub mod capture;
pub mod crypto;
pub mod dropbox;
pub mod google_drive;
pub mod key;
pub mod pending_bundles;
pub mod recipient;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use lucidos_gateway_constants::backup_archive;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::core::{oauth, prefs};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// INVARIANT: A transport failure as the user reads it: the provider, the step that lost
/// the connection, then every cause in the chain.
///
/// reqwest's own text stops at "error sending request for url (…)", which hides
/// the reason. The URL is dropped too: an upload session URI carries its
/// upload id, and every backup error is persisted.
fn transport_failure(provider: &str, step: &str, e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut chain = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(cause) = source {
        let text = cause.to_string();
        if !chain.ends_with(&text) {
            chain.push_str(": ");
            chain.push_str(&text);
        }
        source = cause.source();
    }
    format!("Lost the connection to {provider} while {step} ({chain}).")
}

/// INVARIANT: Registry entry: (id, name, oauth_provider, required_scopes, grant_scopes,
/// constructor).
/// The constructor receives the pool AND the entry's `required_scopes`. The
/// readiness verdict the Settings page renders and the preflight the backup
/// runs therefore check one list, not two that can disagree. `grant_scopes` is
/// what a user actually grants, used only to NAME an unmet requirement (see
/// [`name_missing_scopes`]).
type BackupProviderEntry = (
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    fn(PgPool, &'static [&'static str]) -> Box<dyn BackupProvider>,
);

pub const BACKUP_STALE_AFTER_SECONDS: i64 = 24 * 60 * 60;

pub const BACKUP_OVERDUE_AFTER_SECONDS: i64 = 7 * 24 * 60 * 60;

pub fn is_schedule_active(value: &str) -> bool {
    !value.is_empty() && value != "off"
}

/// INVARIANT: Will a backup actually run, given the stored `(schedule, provider)` pair?
///
/// Both halves are required, which is what `Scheduler::reload_backup_schedule`
/// asks before it registers a job: an active cron with no destination has
/// nowhere to upload, and a destination with the cron off runs nothing.
///
/// The ONE place that rule is written. `api::backup::schedule_response` decides
/// which cron to REPORT with it. [`backups_are_configured`] is deliberately
/// looser: a manual-only workspace runs no cron but is still set up.
pub fn schedule_will_run(schedule: Option<&str>, provider: Option<&str>) -> bool {
    provider_is_set(provider) && schedule.is_some_and(is_schedule_active)
}

fn provider_is_set(provider: Option<&str>) -> bool {
    provider.is_some_and(|p| !p.is_empty())
}

/// INVARIANT: Are this workspace's backups set up: is a destination stored?
///
/// The schedule is not part of it. "Manual only" stores schedule `off`, and
/// that is a way of using backups, not a lack of them. The app-shell banner
/// reads the same rule (`backupIsConfigured`).
///
/// A read failure propagates rather than reading as `false`. "Not set up" is a
/// verdict the picker states out loud, so a transient `sqlx` failure must not be
/// dressed up as one.
pub async fn backups_are_configured(pool: &PgPool) -> Result<bool, BoxError> {
    let provider = prefs::BACKUP_PROVIDER.try_stored(pool).await?;
    Ok(provider_is_set(provider.as_deref()))
}

/// INVARIANT: Read the backup retention count from preferences. A missing or unparseable
/// row is the catalog default, and a stored count is held inside the catalog
/// bounds; a FAILED read is an `Err`.
///
/// The split matters because one caller acts destructively on the answer.
/// Collapsing a failed read into the default means a transient `sqlx` failure
/// reads as "keep 5". `scheduler::backup` then feeds that into
/// [`prune_old_backups`], deleting every archive beyond the fifth. A workspace
/// configured for the catalog maximum of 50 would lose 45 backups on one pool
/// timeout, immediately after a SUCCESSFUL upload. That is the case
/// `.claude/rules/rust.md` names outright: an unknown must never authorize
/// deleting. The two display callers still want the default on an unknown, and
/// say so at their own call sites; the prune caller skips the prune.
pub async fn get_retention_count(pool: &PgPool) -> Result<usize, BoxError> {
    Ok(prefs::BACKUP_RETENTION.try_read(pool).await?.round() as usize)
}

const PROVIDERS: &[BackupProviderEntry] = &[
    (
        "google_drive",
        "Google Drive",
        "google",
        google_drive::BACKUP_SCOPES,
        google_drive::GRANT_SCOPES,
        |pool, scopes| Box::new(google_drive::GoogleDriveBackupProvider::new(pool, scopes)),
    ),
    (
        "dropbox",
        "Dropbox",
        "dropbox",
        // INVARIANT: Never empty: an empty list makes every connected account read as
        // ready however narrow its grant. The failure then lands on
        // `files/create_folder_v2` returning a 400 mid-backup.
        dropbox::BACKUP_SCOPES,
        dropbox::GRANT_SCOPES,
        |pool, scopes| Box::new(dropbox::DropboxBackupProvider::new(pool, scopes)),
    ),
];

pub const PROVIDER_IDS: &[&str] = &["google_drive", "dropbox"];

pub struct ProviderMeta {
    pub id: &'static str,
    pub name: &'static str,
    pub oauth_provider: &'static str,
    /// INVARIANT: Every scope substring that must appear in the OAuth account's scopes for
    /// the provider to be ready. Empty slice means connecting is enough.
    ///
    /// This is the SAME list the provider's preflight checks, deliberately.
    /// Readiness gating on one scope while preflight demands three lets an
    /// account holding only the first read as ready. The page then hides *Grant
    /// access* and enables *Back up now*. Preflight fails and tells the user to
    /// reconnect, with no button left to do it.
    pub required_scopes: &'static [&'static str],
    pub grant_scopes: &'static [&'static str],
}

pub fn list_providers() -> Vec<ProviderMeta> {
    PROVIDERS
        .iter()
        .map(|(id, name, oauth, scopes, grant_scopes, _)| ProviderMeta {
            id,
            name,
            oauth_provider: oauth,
            required_scopes: scopes,
            grant_scopes,
        })
        .collect()
}

pub fn provider_meta(provider_id: &str) -> Option<ProviderMeta> {
    list_providers().into_iter().find(|m| m.id == provider_id)
}

/// INVARIANT: Does a granted-scope string carry `required`?
///
/// Substring rather than token match, deliberately: Google's registry entry is
/// the fragment `"drive"`, which has to match the full
/// `https://www.googleapis.com/auth/drive.file` URL. Dropbox's scopes are whole
/// names (`files.content.write`), where the two readings coincide. An empty
/// requirement always passes.
///
/// This is NOT the question
/// [`crate::core::oauth::missing_requested_scopes`] answers. That one reports
/// what an authorization asked for and did not get, so it compares whole
/// tokens: containment there would read a refused scope as granted whenever
/// another granted scope happened to contain its name. Keep the two apart.
pub fn scopes_include(granted: &str, required: &str) -> bool {
    required.is_empty() || granted.contains(required)
}

/// INVARIANT: Which of `required` a granted-scope string is missing, in the order the
/// provider declared them so a message reads the same every time.
///
/// The ONE place the scope question is answered. Both providers' preflights and
/// [`provider_readiness`] call it with the same registry list. That is what
/// stops the page and the backup disagreeing about whether an account works.
pub fn missing_scopes(granted: &str, required: &'static [&'static str]) -> Vec<&'static str> {
    required
        .iter()
        .copied()
        .filter(|s| !scopes_include(granted, s))
        .collect()
}

pub fn name_missing_scopes(
    missing: Vec<&'static str>,
    grant_scopes: &'static [&'static str],
) -> Vec<&'static str> {
    missing
        .into_iter()
        .map(|requirement| {
            grant_scopes
                .iter()
                .copied()
                .find(|granted| granted.contains(requirement))
                .unwrap_or(requirement)
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderReadiness {
    pub connected: bool,
    pub missing_scopes: Vec<&'static str>,
}

impl ProviderReadiness {
    /// INVARIANT: Connected AND holding every required scope.
    ///
    /// Derived rather than stored, deliberately. Keeping both a boolean and the
    /// list it is computed from allows a state where they disagree, and a method
    /// does not. See `docs/plans/2026-08-05-dropbox-backup-scopes-and-refresh.md`
    /// for the two reconciliations that made the point.
    pub fn ready(&self) -> bool {
        self.connected && self.missing_scopes.is_empty()
    }
}

pub async fn provider_readiness(
    pool: &PgPool,
    meta: &ProviderMeta,
) -> Result<ProviderReadiness, BoxError> {
    let account = oauth::OAuthStore::get_by_provider(pool, meta.oauth_provider)
        .await
        .map_err(|e| -> BoxError {
            format!(
                "Failed to query OAuth account for {}: {e}",
                meta.oauth_provider
            )
            .into()
        })?;
    let missing_scopes = account
        .as_ref()
        .map(|a| {
            name_missing_scopes(
                missing_scopes(&a.scopes, meta.required_scopes),
                meta.grant_scopes,
            )
        })
        .unwrap_or_default();
    Ok(ProviderReadiness {
        connected: account.is_some(),
        missing_scopes,
    })
}

pub fn get_provider(provider_id: &str, pool: &PgPool) -> Result<Box<dyn BackupProvider>, String> {
    PROVIDERS
        .iter()
        .find(|(id, _, _, _, _, _)| *id == provider_id)
        .map(|(_, _, _, scopes, _, ctor)| ctor(pool.clone(), scopes))
        .ok_or_else(|| format!("Unknown backup provider: {}", provider_id))
}

pub async fn get_oauth_account(
    pool: &PgPool,
    provider: &str,
) -> Result<oauth::OAuthAccount, BoxError> {
    use crate::core::oauth::AccountLookupError;
    oauth::get_account_with_fresh_token(pool, provider)
        .await
        .map_err(|e| -> BoxError {
            match e {
                AccountLookupError::NotConnected => format!(
                    "No {} account connected. Connect it in Settings first.",
                    provider
                )
                .into(),
                AccountLookupError::DbError(err) => err,
                AccountLookupError::RefreshFailed(msg) => msg.into(),
            }
        })
}

pub async fn get_oauth_token(pool: &PgPool, provider: &str) -> Result<String, BoxError> {
    Ok(get_oauth_account(pool, provider).await?.access_token)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    pub id: String,
    pub filename: String,
    pub size_bytes: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackupRunStatus {
    Success,
    Failure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupLastRun {
    pub status: BackupRunStatus,
    pub at: DateTime<Utc>,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    pub filename: Option<String>,
    pub size_bytes: Option<u64>,
    pub error: Option<String>,
}

impl BackupLastRun {
    pub fn success(entry: &BackupEntry, started_at: DateTime<Utc>) -> Self {
        Self {
            status: BackupRunStatus::Success,
            at: Utc::now(),
            started_at: Some(started_at),
            filename: Some(entry.filename.clone()),
            size_bytes: Some(entry.size_bytes),
            error: None,
        }
    }

    pub fn failure(error: &str, started_at: DateTime<Utc>) -> Self {
        Self {
            status: BackupRunStatus::Failure,
            at: Utc::now(),
            started_at: Some(started_at),
            filename: None,
            size_bytes: None,
            error: Some(error.to_string()),
        }
    }
}

pub async fn persist_last_run(pool: &PgPool, run: &BackupLastRun) -> Result<(), BoxError> {
    use crate::core::PreferenceStore;
    let value = serde_json::to_string(run)?;
    PreferenceStore::set_silent(pool, prefs::BACKUP_LAST_RUN.key(), &value).await?;
    Ok(())
}

pub async fn load_last_run(pool: &PgPool) -> Option<BackupLastRun> {
    let key = prefs::BACKUP_LAST_RUN.key();
    let raw = match prefs::BACKUP_LAST_RUN.try_stored(pool).await {
        Ok(raw) => raw?,
        Err(e) => {
            crate::log_legacy!("[Backup] Could not read {key}: {e}");
            return None;
        }
    };
    match serde_json::from_str(&raw) {
        Ok(run) => Some(run),
        Err(e) => {
            crate::log_legacy!("[Backup] Ignoring malformed {key}: {e}");
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupRunSummary {
    pub status: BackupRunStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: DateTime<Utc>,
    pub filename: Option<String>,
    pub size_bytes: Option<u64>,
    pub error: Option<String>,
}

pub async fn load_recent_runs(pool: &PgPool, limit: i64) -> Vec<BackupRunSummary> {
    use sqlx::Row;

    let rows = match sqlx::query(
        "SELECT event_type, payload, created FROM events \
         WHERE event_type IN ('BackupCompleted', 'BackupFailed') \
         ORDER BY created DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            crate::log_legacy!("[Backup] Failed to load recent runs: {e}");
            return Vec::new();
        }
    };

    rows.iter()
        .filter_map(|row| {
            let event_type: String = row.get("event_type");
            let payload: serde_json::Value = row.get("payload");
            let created: DateTime<Utc> = row.get("created");
            backup_run_from_event(&event_type, &payload, created)
        })
        .collect()
}

/// INVARIANT: When the newest SUCCESSFUL backup run finished, or `None` when this workspace
/// has never produced one.
///
/// The `BackupCompleted` row is the evidence, so a failure since the last good
/// run cannot hide it. `prefs::BACKUP_LAST_RUN` would: it holds the last run
/// whatever its outcome. `idx_events_type_created` covers the lookup, so this is
/// one indexed read, cheap enough for the gateway to repeat on every backup event.
///
/// A query failure is an `Err`, NOT an empty answer. `None` means "never backed
/// up", which the picker states out loud in its warning colour. A transient
/// `sqlx` failure reported as `None` would therefore be a wrong answer rather
/// than a missing one. [`load_recent_runs`] degrades instead, because an empty
/// history list says nothing false.
pub async fn load_last_successful_backup(pool: &PgPool) -> Result<Option<DateTime<Utc>>, BoxError> {
    Ok(load_last_successful_backup_run(pool)
        .await?
        .map(|run| run.finished_at))
}

pub async fn load_last_successful_backup_run(
    pool: &PgPool,
) -> Result<Option<BackupRunSummary>, BoxError> {
    use sqlx::Row;

    let Some(row) = sqlx::query(
        "SELECT payload, created FROM events \
         WHERE event_type = 'BackupCompleted' \
         ORDER BY created DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let payload: serde_json::Value = row.get("payload");
    let created: DateTime<Utc> = row.get("created");
    Ok(backup_run_from_event("BackupCompleted", &payload, created))
}

fn backup_run_from_event(
    event_type: &str,
    payload: &serde_json::Value,
    created: DateTime<Utc>,
) -> Option<BackupRunSummary> {
    let data = payload.get("data").unwrap_or(payload);
    let parse_ts = |key: &str| {
        data.get(key)
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
    };
    let started_at = parse_ts("started_at");
    let finished_at = parse_ts("finished_at").unwrap_or(created);
    match event_type {
        "BackupCompleted" => Some(BackupRunSummary {
            status: BackupRunStatus::Success,
            started_at,
            finished_at,
            filename: data
                .get("filename")
                .and_then(|v| v.as_str())
                .map(String::from),
            size_bytes: data.get("size_bytes").and_then(|v| v.as_u64()),
            error: None,
        }),
        "BackupFailed" => Some(BackupRunSummary {
            status: BackupRunStatus::Failure,
            started_at,
            finished_at,
            filename: None,
            size_bytes: None,
            error: data.get("error").and_then(|v| v.as_str()).map(String::from),
        }),
        _ => None,
    }
}

#[async_trait]
pub trait BackupProvider: Send + Sync {
    fn name(&self) -> &str;
    fn id(&self) -> &str;
    fn oauth_provider(&self) -> &str;
    async fn folder_url(&self) -> Option<String>;
    async fn preflight(&self, estimated_upload_bytes: u64) -> Result<(), BoxError>;
    async fn upload(
        &self,
        file_path: &Path,
        filename: &str,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<String, BoxError>;
    async fn list_backups(&self) -> Result<Vec<BackupEntry>, BoxError>;
    async fn download(
        &self,
        backup_id: &str,
        dest: &Path,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), BoxError>;
    async fn delete(&self, backup_id: &str) -> Result<(), BoxError>;
}

/// INVARIANT: The archive entry holding the secret store key the dump's secrets are
/// sealed with. Restore files it into the key store and never into the
/// workspace directory.
pub const SECRET_STORE_KEY_ENTRY: &str = "secret-store.key";

#[derive(Debug, PartialEq, Eq)]
pub struct BackupOrigin(String);

impl std::str::FromStr for BackupOrigin {
    type Err = &'static str;

    fn from_str(token: &str) -> Result<Self, Self::Err> {
        if !backup_archive::is_origin(token) {
            return Err("Invalid backup origin");
        }
        Ok(Self(token.to_string()))
    }
}

async fn canonical_backup_path(workspace: &Path) -> Result<String, BoxError> {
    tokio::fs::canonicalize(workspace)
        .await?
        .into_os_string()
        .into_string()
        .map_err(|_| "The canonical workspace path is not valid UTF-8".into())
}

pub async fn read_backup_origin(
    pool: &PgPool,
    workspace: &Path,
) -> Result<Option<BackupOrigin>, BoxError> {
    let path = canonical_backup_path(workspace).await?;
    let token: Option<String> = sqlx::query_scalar(
        "SELECT origin FROM backup_origin WHERE singleton AND workspace_path = $1",
    )
    .bind(path)
    .fetch_optional(pool)
    .await?;
    Ok(token.map(|token| token.parse()).transpose()?)
}

pub async fn resolve_backup_origin(
    pool: &PgPool,
    workspace: &Path,
) -> Result<BackupOrigin, BoxError> {
    let path = canonical_backup_path(workspace).await?;
    let random: u64 = rand::random();
    let candidate = format!("{:012x}", random & 0xffff_ffff_ffff);
    let token: String = sqlx::query_scalar(
        "INSERT INTO backup_origin (workspace_path, origin) VALUES ($1, $2)
         ON CONFLICT (singleton) DO UPDATE SET
           workspace_path = EXCLUDED.workspace_path,
           origin = CASE WHEN backup_origin.workspace_path = EXCLUDED.workspace_path
                         THEN backup_origin.origin ELSE EXCLUDED.origin END
         RETURNING origin",
    )
    .bind(path)
    .bind(candidate)
    .fetch_one(pool)
    .await?;
    Ok(token.parse()?)
}

pub fn workspace_archive_name(workspace: &Path) -> &str {
    workspace
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace")
}

pub async fn restore_archive_into(
    workspace_dir: &Path,
    database_url: &str,
    key: crypto::OpeningKey,
    encrypted_path: &Path,
    progress: impl Fn(&str, usize, usize) + Send + Sync + 'static,
) -> Result<Vec<String>, BoxError> {
    pg_tools_preflight(database_url)?;

    let temp_dir = tempfile::tempdir()?;
    let progress = Arc::new(progress);

    let enc_bytes = std::fs::metadata(encrypted_path)?.len();
    let (decrypt_end, decompress_end) = estimate_restore_weights(enc_bytes, 0);

    let (staging_dir, secret_key, bundles) = {
        let progress = progress.clone();
        let encrypted_path = encrypted_path.to_path_buf();
        let temp_path = temp_dir.path().to_path_buf();

        type Staged = (
            PathBuf,
            Option<crate::core::secret_store::SecretKey>,
            Option<PathBuf>,
        );
        tokio::task::spawn_blocking(move || -> Result<Staged, BoxError> {
            progress("decrypting", 0, 100);
            let compressed_path = temp_path.join("backup.tar.zst");
            {
                let total_bytes = std::fs::metadata(&encrypted_path)?.len();
                let input = std::fs::File::open(&encrypted_path)?;
                let mut last_pct = 0usize;
                let reader = ProgressReader {
                    inner: input,
                    bytes_read: 0,
                    callback: |done| {
                        let pct =
                            (decrypt_end as f64 * done as f64 / total_bytes.max(1) as f64) as usize;
                        let pct = pct.min(decrypt_end);
                        if pct > last_pct {
                            last_pct = pct;
                            progress("decrypting", pct, 100);
                        }
                    },
                };
                let mut output = std::fs::File::create(&compressed_path)?;
                crypto::open(&key, reader, &mut output)?;
            }

            progress("decompressing", decrypt_end, 100);
            let staging_dir = temp_path.join("staging");
            std::fs::create_dir_all(&staging_dir)?;
            {
                let file = std::fs::File::open(&compressed_path)?;
                let decoder = zstd::Decoder::new(file)?;
                let mut archive = tar::Archive::new(decoder);
                archive.unpack(&staging_dir)?;
            }

            // INVARIANT: Drop any `user_dir/`, the source's ~/.lucidos, so it is neither
            // applied over the target's ~/.lucidos nor littered into the restored
            // workspace. Current backups no longer include it (see
            // `tar_and_compress`); this stays as defense for LEGACY archives.
            let user_dir_staging = staging_dir.join("user_dir");
            if user_dir_staging.exists() {
                std::fs::remove_dir_all(&user_dir_staging)?;
            }

            let key_entry = staging_dir.join(SECRET_STORE_KEY_ENTRY);
            let secret_key = match std::fs::read_to_string(&key_entry) {
                Ok(text) => {
                    std::fs::remove_file(&key_entry)?;
                    Some(crate::core::secret_store::SecretKey::from_base64(&text)?)
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            };

            let staged_bundles = staging_dir.join(pending_bundles::ARCHIVE_DIR);
            let bundles = if staged_bundles.exists() {
                let kept = temp_path.join(pending_bundles::ARCHIVE_DIR);
                std::fs::rename(&staged_bundles, &kept)?;
                Some(kept)
            } else {
                None
            };

            Ok((staging_dir, secret_key, bundles))
        })
        .await??
    };

    progress("initializing", decompress_end, 100);
    {
        let workspace_dir = workspace_dir.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<(), BoxError> {
            std::fs::create_dir_all(&workspace_dir)?;
            move_contents(&staging_dir, &workspace_dir)
        })
        .await??;
    }

    let mut warnings = Vec::new();
    if let Some(bundles) = bundles {
        match pending_bundles::restore_bundles(&bundles).await {
            Ok(problems) => warnings.extend(
                problems
                    .into_iter()
                    .map(|problem| format!("A pending change branch was not restored: {problem}")),
            ),
            Err(e) => warnings.push(format!("No pending change branch was restored: {e}")),
        }
    }
    for warning in &warnings {
        log_legacy!("[Restore] {warning}");
    }

    progress("restoring_db", decompress_end + 1, 100);
    {
        let ws_dir = workspace_dir.to_path_buf();
        let database_url = database_url.to_string();
        tokio::task::spawn_blocking(move || -> Result<(), BoxError> {
            let dump_path = ws_dir.join("lucidos_backup.dump");
            if dump_path.exists() {
                terminate_other_connections(&database_url)?;
                pg_restore(&database_url, &dump_path)?;
                // INVARIANT: The restore succeeded, so failing it now would be wrong. The
                // leftover is a full database dump in the workspace root.
                if let Err(e) = std::fs::remove_file(&dump_path) {
                    log_legacy!(
                        "[Backup] Restore done, but {} was not removed: {}",
                        dump_path.display(),
                        e
                    );
                }
            }
            Ok(())
        })
        .await??;
    }

    // INVARIANT: The restored secrets open only with the key they were sealed with.
    // Failing here leaves a database that would refuse to boot, so say so now.
    if let Some(key) = secret_key {
        use crate::core::secret_store::key_store;
        let database = crate::core::secret_store::boot::database_name(database_url)?;
        let store = key_store::resolve()?;
        let name = key_store::secrets_key_name(&database);
        if let Some(parked) = key_store::replace_keeping(store.as_ref(), &name, &key)? {
            log_legacy!(
                "[Restore] Kept the secret store key the restore replaced as {}",
                store.describe(&parked)
            );
        }
    }

    progress("done", 100, 100);
    Ok(warnings)
}

pub async fn create_backup(
    pool: &PgPool,
    workspace: &Path,
    database_url: &str,
    key: crypto::SealingKey,
    provider: &dyn BackupProvider,
    progress: impl Fn(&str, usize, usize) + Send + Sync + 'static,
) -> Result<(BackupEntry, BackupOrigin), BoxError> {
    let origin = resolve_backup_origin(pool, workspace)
        .await
        .map_err(|e| format!("could not resolve the backup origin: {e}"))?;
    let pending_branches = crate::core::changes::unresolved_change_branches(pool)
        .await
        .map_err(|e| format!("could not list the pending changes to back up: {e}"))?;
    crate::log_legacy!(
        "[Backup] Starting backup to {} (provider: {})",
        provider.name(),
        provider.id()
    );

    pg_tools_preflight(database_url)?;

    let ws_bytes = {
        let workspace = workspace.to_path_buf();
        tokio::task::spawn_blocking(move || {
            workspace_backup_size(&workspace, &BackupIgnore::load(&workspace))
        })
        .await?
    };

    provider.preflight(estimate_archive_size(ws_bytes)).await?;

    let temp_dir = tempfile::tempdir()?;
    let progress = Arc::new(progress);

    let pending_dir = temp_dir.path().join(pending_bundles::ARCHIVE_DIR);
    let pending_dir = pending_bundles::write_bundles(workspace, &pending_branches, &pending_dir)
        .await?
        .then_some(pending_dir);

    let compressed_path = {
        let workspace = workspace.to_path_buf();
        let database_url = database_url.to_owned();
        let temp_path = temp_dir.path().to_path_buf();
        let progress = progress.clone();
        tokio::task::spawn_blocking(move || {
            capture::capture_workspace_archive(
                &workspace,
                &database_url,
                &temp_path,
                pending_dir.as_deref(),
                ws_bytes,
                progress.as_ref(),
            )
        })
        .await??
    };
    let workspace_name = workspace_archive_name(workspace);
    let suffix = key.archive_suffix();
    let filename = backup_archive::format_marked(
        workspace_name,
        Utc::now().format("%Y%m%d-%H%M%S"),
        &origin.0,
        suffix,
    );

    let entry = seal_and_upload(
        filename,
        temp_dir.path(),
        &compressed_path,
        ws_bytes,
        key,
        provider,
        progress,
    )
    .await?;
    Ok((entry, origin))
}

async fn seal_and_upload(
    filename: String,
    temp_path: &Path,
    compressed_path: &Path,
    ws_bytes: u64,
    key: crypto::SealingKey,
    provider: &dyn BackupProvider,
    progress: Arc<impl Fn(&str, usize, usize) + Send + Sync + 'static>,
) -> Result<BackupEntry, BoxError> {
    let suffix = key.archive_suffix();
    let (encrypted_path, encrypt_end) = {
        let progress = progress.clone();
        let temp_path = temp_path.to_path_buf();
        let compressed_path = compressed_path.to_path_buf();

        tokio::task::spawn_blocking(move || -> Result<(PathBuf, usize), BoxError> {
            let (_, compress_end, encrypt_end) = estimate_weights(ws_bytes);
            crate::log_legacy!("[Backup] Phase 3/4: encrypt");
            progress("encrypting", compress_end, 100);
            let encrypted_path = temp_path.join(format!("backup{suffix}"));
            {
                let total_bytes = std::fs::metadata(&compressed_path)?.len();
                let encrypt_range = encrypt_end - compress_end;
                let input = std::fs::File::open(&compressed_path)?;
                let mut last_pct = compress_end;
                let reader = ProgressReader {
                    inner: input,
                    bytes_read: 0,
                    callback: |done| {
                        let pct = compress_end
                            + (encrypt_range as f64 * done as f64 / total_bytes.max(1) as f64)
                                as usize;
                        let pct = pct.min(encrypt_end);
                        if pct > last_pct {
                            last_pct = pct;
                            progress("encrypting", pct, 100);
                        }
                    },
                };
                let mut output = std::fs::File::create(&encrypted_path)?;
                crypto::seal(&key, reader, &mut output)?;
            }

            Ok((encrypted_path, encrypt_end))
        })
        .await??
    };

    crate::log_legacy!("[Backup] Phase 4/4: upload");
    progress("uploading", encrypt_end, 100);
    let upload_progress = |done: u64, total: u64| {
        let pct = if total > 0 {
            encrypt_end + ((100 - encrypt_end) as f64 * done as f64 / total as f64) as usize
        } else {
            encrypt_end
        };
        progress("uploading", pct, 100);
    };
    let backup_id = provider
        .upload(&encrypted_path, &filename, &upload_progress)
        .await?;
    let size_bytes = std::fs::metadata(&encrypted_path)?.len();
    crate::log_legacy!("[Backup] Complete: {} ({} bytes)", filename, size_bytes);

    Ok(BackupEntry {
        id: backup_id,
        filename,
        size_bytes,
        created_at: Utc::now(),
    })
}

pub fn is_own_backup_archive(filename: &str, workspace_name: &str, origin: &BackupOrigin) -> bool {
    backup_archive::parse(filename).is_some_and(|archive| {
        archive.workspace == workspace_name && archive.origin == Some(origin.0.as_str())
    })
}

fn select_prunable(
    entries: Vec<BackupEntry>,
    workspace_name: &str,
    origin: &BackupOrigin,
    keep: usize,
) -> Vec<BackupEntry> {
    let mut own: Vec<BackupEntry> = entries
        .into_iter()
        .filter(|e| is_own_backup_archive(&e.filename, workspace_name, origin))
        .collect();
    if own.len() <= keep {
        return Vec::new();
    }
    own.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let mut to_delete = own.split_off(keep);
    to_delete.reverse();
    to_delete
}

pub async fn prune_old_backups(
    provider: &dyn BackupProvider,
    workspace_name: &str,
    origin: &BackupOrigin,
    keep: usize,
) -> Result<usize, BoxError> {
    if keep == 0 {
        return Ok(0);
    }
    let entries = provider.list_backups().await?;
    let to_delete = select_prunable(entries, workspace_name, origin, keep);
    let mut deleted = 0;
    for entry in to_delete {
        match provider.delete(&entry.id).await {
            Ok(()) => {
                crate::log_legacy!("[Backup] Pruned old backup: {}", entry.filename);
                deleted += 1;
            }
            Err(e) => {
                crate::log_legacy!("[Backup] Failed to prune {}: {}", entry.filename, e);
            }
        }
    }
    Ok(deleted)
}

/// INVARIANT: The snapshot capture's free-space estimate. Unlike the backup preflight it
/// refuses an unreadable tree, so a disk floor is never checked against zero.
pub(crate) fn snapshot_file_estimate(workspace: &Path) -> Result<u64, BoxError> {
    included_file_bytes(
        workspace,
        &BackupIgnore::load(workspace),
        Unreadable::Refuse,
    )
}

fn workspace_backup_size(workspace: &Path, ignore: &BackupIgnore) -> u64 {
    included_file_bytes(workspace, ignore, Unreadable::Skip).unwrap_or_default()
}

fn included_file_bytes(
    workspace: &Path,
    ignore: &BackupIgnore,
    unreadable: Unreadable,
) -> Result<u64, BoxError> {
    sum_file_bytes(workspace, walkdir(workspace)?, ignore, unreadable)
}

#[derive(Debug, Clone, Copy)]
enum Unreadable {
    Skip,
    Refuse,
}

fn sum_file_bytes(
    workspace: &Path,
    paths: Vec<PathBuf>,
    ignore: &BackupIgnore,
    unreadable: Unreadable,
) -> Result<u64, BoxError> {
    let mut bytes = 0u64;
    for path in paths {
        if is_excluded_workspace_path(path.strip_prefix(workspace)?, ignore) {
            continue;
        }
        match (std::fs::metadata(&path), unreadable) {
            (Ok(metadata), _) => bytes = bytes.saturating_add(metadata.len()),
            (Err(error), _) if error.kind() == std::io::ErrorKind::NotFound => {}
            (Err(_), Unreadable::Skip) => {}
            (Err(error), Unreadable::Refuse) => return Err(error.into()),
        }
    }
    Ok(bytes)
}

fn estimate_archive_size(ws_bytes: u64) -> u64 {
    ws_bytes / 3
}

fn estimate_weights(ws_bytes: u64) -> (usize, usize, usize) {
    let ws_mb = ws_bytes as f64 / 1_048_576.0;
    let compressed_mb = ws_mb * 0.33;

    let dump_time = ws_mb / 50.0 + 2.0;
    let compress_time = ws_mb / 25.0;
    let encrypt_time = compressed_mb / 5.0;
    let upload_time = compressed_mb / 10.0;
    let total = (dump_time + compress_time + encrypt_time + upload_time).max(1.0);

    let dump_pct = ((dump_time / total) * 100.0) as usize;
    let compress_pct = ((compress_time / total) * 100.0) as usize;
    let encrypt_pct = ((encrypt_time / total) * 100.0) as usize;

    let dump_end = dump_pct.clamp(2, 90);
    let compress_end = (dump_end + compress_pct.max(2)).min(93);
    let encrypt_end = (compress_end + encrypt_pct.max(2)).min(97);

    (dump_end, compress_end, encrypt_end)
}

fn estimate_restore_weights(enc_bytes: u64, download_end: usize) -> (usize, usize) {
    let enc_mb = enc_bytes as f64 / 1_048_576.0;
    let compressed_mb = enc_mb;
    let uncompressed_mb = compressed_mb * 3.0;

    let decrypt_time = enc_mb / 5.0;
    let decompress_time = compressed_mb / 50.0;
    let restore_time = uncompressed_mb / 20.0 + 2.0;
    let total = (decrypt_time + decompress_time + restore_time).max(1.0);

    let remaining = 100 - download_end;
    let decrypt_pct = ((decrypt_time / total) * remaining as f64) as usize;
    let decompress_pct = ((decompress_time / total) * remaining as f64) as usize;

    let decrypt_end = (download_end + decrypt_pct.max(2)).min(95);
    let decompress_end = (decrypt_end + decompress_pct.max(2)).min(97);

    (decrypt_end, decompress_end)
}

struct ProgressReader<R, F> {
    inner: R,
    bytes_read: u64,
    callback: F,
}

impl<R: std::io::Read, F: FnMut(u64)> std::io::Read for ProgressReader<R, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes_read += n as u64;
        (self.callback)(self.bytes_read);
        Ok(n)
    }
}

fn move_contents(src: &Path, dest: &Path) -> Result<(), BoxError> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if let Err(e) = std::fs::rename(&src_path, &dest_path) {
            crate::log_legacy!(
                "[Backup] rename failed for {}, falling back to copy: {}",
                src_path.display(),
                e
            );
            if src_path.is_dir() {
                copy_dir_recursive(&src_path, &dest_path)?;
                std::fs::remove_dir_all(&src_path)?;
            } else {
                std::fs::copy(&src_path, &dest_path)?;
                std::fs::remove_file(&src_path)?;
            }
        }
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), BoxError> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else {
            std::fs::copy(&src_path, &dest_path)?;
        }
    }
    Ok(())
}

/// INVARIANT: Apply the libpq env for `database_url` onto `cmd`. Returns `Err` when the
/// URL does not parse, so the caller fails loudly. Otherwise it would spawn a
/// subprocess that inherits zero `PG*` vars and gets a confusing connection
/// error from libpq.
///
/// A URL signing in with the boot password names the boot password file
/// instead, so the tool's environment holds no password.
fn apply_pg_env(cmd: &mut std::process::Command, database_url: &str) -> Result<(), BoxError> {
    let mut url = lucidos_pg_url::PgUrl::parse(database_url)?;
    crate::core::use_boot_passfile(&mut url);
    cmd.env_remove("PGPASSWORD");
    for (key, value) in url.env() {
        cmd.env(key, value);
    }
    Ok(())
}

/// INVARIANT: Build a `Command` for a PostgreSQL client tool (`pg_dump` / `pg_restore` /
/// `psql`), resolved + ready to run.
///
/// In a packaged build the relocatable PG client binaries live at
/// `<resources>/postgres/bin` (`LUCIDOS_PG_BIN_DIR`), which is NOT on the
/// launchd minimal PATH. A bare `Command::new("pg_dump")` ENOENTs mid-backup.
/// So resolve against `LUCIDOS_PG_BIN_DIR` when set, failing fast with the path
/// if the binary is missing. Make the bundled libpq loadable via
/// `DYLD_LIBRARY_PATH` or `LD_LIBRARY_PATH` from `LUCIDOS_PG_LIB_DIR`. In dev
/// neither var is set and we fall back to a bare PATH lookup.
pub fn pg_tool_command(name: &str, database_url: &str) -> Result<std::process::Command, BoxError> {
    let bin_dir = std::env::var_os("LUCIDOS_PG_BIN_DIR").map(PathBuf::from);
    let bin = resolve_pg_tool_path(name, bin_dir.as_deref())?;
    let mut cmd = std::process::Command::new(bin);
    if let Some(lib) = std::env::var_os("LUCIDOS_PG_LIB_DIR") {
        cmd.env("DYLD_LIBRARY_PATH", &lib);
        cmd.env("LD_LIBRARY_PATH", &lib);
    }
    apply_pg_env(&mut cmd, database_url)?;
    Ok(cmd)
}

fn resolve_pg_tool_path(name: &str, bin_dir: Option<&Path>) -> Result<PathBuf, BoxError> {
    match bin_dir {
        Some(dir) => {
            let bin = dir.join(name);
            if !bin.is_file() {
                return Err(format!(
                    "bundled PostgreSQL client '{name}' not found at {} \
                     (LUCIDOS_PG_BIN_DIR) — backup/restore unavailable",
                    bin.display()
                )
                .into());
            }
            Ok(bin)
        }
        None => Ok(PathBuf::from(name)),
    }
}

fn pg_tools_preflight(database_url: &str) -> Result<(), BoxError> {
    if std::env::var_os("LUCIDOS_PG_BIN_DIR").is_none() {
        return Ok(());
    }
    for name in ["pg_dump", "pg_restore", "psql"] {
        pg_tool_command(name, database_url)?;
    }
    Ok(())
}

/// INVARIANT: Run pg_dump to export the database in custom archive format.
///
/// Custom format (`-Fc`) is a compressed binary format restored via `pg_restore`.
/// It avoids PostgreSQL 18's `\restrict` / `\unrestrict` psql meta-commands
/// (CVE-2025-8714) that break plain-text restores, and supports parallel restore.
///
/// Connection details flow through libpq `PG*` env vars so the password
/// stays out of argv.
fn pg_dump(database_url: &str, output_path: &Path) -> Result<(), BoxError> {
    pg_dump_with_snapshot(database_url, output_path, None)
}

fn pg_dump_with_snapshot(
    database_url: &str,
    output_path: &Path,
    snapshot: Option<&str>,
) -> Result<(), BoxError> {
    let mut cmd = pg_tool_command("pg_dump", database_url)?;
    if let Some(snapshot) = snapshot {
        cmd.arg(format!("--snapshot={snapshot}"));
    }
    let output = cmd
        .args([
            "--format=custom",
            "--no-owner",
            "--no-acl",
            "--clean",
            "--if-exists",
        ])
        .arg("-f")
        .arg(output_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "pg_dump failed with exit code: {}\n{}",
            output.status, stderr
        )
        .into());
    }
    Ok(())
}

fn pg_dbname(database_url: &str) -> Result<String, BoxError> {
    Ok(lucidos_pg_url::PgUrl::parse(database_url)?.database)
}

const CROSS_VERSION_SET_PARAMS: &[&str] = &["transaction_timeout"];

fn is_cross_version_set(line: &str) -> bool {
    let trimmed = line.trim();
    let upper = trimmed.to_uppercase();
    if !upper.starts_with("SET ") {
        return false;
    }
    for param in CROSS_VERSION_SET_PARAMS {
        if upper.starts_with(&format!("SET {} ", param.to_uppercase()))
            || upper.starts_with(&format!("SET {}=", param.to_uppercase()))
        {
            return true;
        }
    }
    false
}

fn pg_restore(database_url: &str, dump_path: &Path) -> Result<(), BoxError> {
    pg_restore_with_checks(
        database_url,
        dump_path,
        "",
        "
        DO $$
        BEGIN
            IF to_regclass('public.backup_origin') IS NOT NULL THEN
                DELETE FROM public.backup_origin;
            END IF;
        END
        $$;
        ",
    )
}

/// INVARIANT: Offline snapshot replay shares the archive SQL filtering and atomic psql
/// pipeline. The surrounding SQL is generated internally, never caller input.
pub(super) fn pg_restore_with_checks(
    database_url: &str,
    dump_path: &Path,
    before: &str,
    after: &str,
) -> Result<(), BoxError> {
    use std::io::{BufRead, BufReader, Write};

    let dbname = pg_dbname(database_url)?;

    let mut restore_cmd = pg_tool_command("pg_restore", database_url)?;
    let restore_output = restore_cmd
        .args([
            "--no-owner",
            "--no-acl",
            "--clean",
            "--if-exists",
            "--file=-",
        ])
        .arg(dump_path)
        .output()?;

    if !restore_output.status.success() {
        let stderr = String::from_utf8_lossy(&restore_output.stderr);
        return Err(format!(
            "pg_restore (to SQL) failed with exit code: {}\n{}",
            restore_output.status, stderr
        )
        .into());
    }

    let reader = BufReader::new(&restore_output.stdout[..]);
    let mut filtered = Vec::with_capacity(restore_output.stdout.len() + before.len() + after.len());
    filtered.extend_from_slice(before.as_bytes());
    for line in reader.split(b'\n') {
        let line = line?;
        let text = String::from_utf8_lossy(&line);
        if !is_cross_version_set(&text) {
            filtered.extend_from_slice(&line);
            filtered.push(b'\n');
        }
    }

    filtered.extend_from_slice(after.as_bytes());

    let mut psql_cmd = pg_tool_command("psql", database_url)?;
    let mut psql = psql_cmd
        .args(psql_restore_args(&dbname))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    // INVARIANT: Write stdin on a separate thread while `wait_with_output` drains
    // stdout/stderr. A synchronous `write_all` before the wait can deadlock on
    // a large restore: psql blocks once its stdout pipe fills, stops reading
    // stdin, and our write blocks forever. The write result is checked AFTER
    // the status so an EPIPE from an early psql death can't mask the real
    // stderr message.
    let mut stdin = psql.stdin.take().ok_or("failed to open psql stdin")?;
    let writer = std::thread::spawn(move || stdin.write_all(&filtered));
    let psql_output = psql.wait_with_output()?;
    let write_result = writer
        .join()
        .map_err(|_| "psql stdin writer thread panicked")?;

    if !psql_output.status.success() {
        let stderr = String::from_utf8_lossy(&psql_output.stderr);
        return Err(format!(
            "Restore failed: psql exited with code: {}\n{}",
            psql_output.status, stderr
        )
        .into());
    }
    write_result?;
    Ok(())
}

/// INVARIANT: The psql arguments that replay a restore's SQL.
///
/// `ON_ERROR_STOP` is what makes a failed restore exit non-zero. Without it,
/// one failing statement rolls the single transaction back, and psql still
/// exits 0. The user is then told an empty database was restored.
/// `--no-psqlrc` keeps a user's `~/.psqlrc` out of the replay.
fn psql_restore_args(dbname: &str) -> [&str; 6] {
    [
        "--single-transaction",
        "--no-psqlrc",
        "--set",
        "ON_ERROR_STOP=1",
        "--dbname",
        dbname,
    ]
}

fn terminate_other_connections(database_url: &str) -> Result<(), BoxError> {
    let mut cmd = pg_tool_command("psql", database_url)?;
    let output = cmd
        .arg("-c")
        .arg("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = current_database() AND pid <> pg_backend_pid()")
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        crate::log_legacy!(
            "[Backup] Warning: failed to terminate other connections: {}",
            stderr
        );
    }
    Ok(())
}

enum IgnorePattern {
    Prefix(PathBuf),
    /// INVARIANT: A pattern containing glob wildcards (`*`, `?`, `[`). Matched against the
    /// workspace-relative path *and each of its ancestors*, so a glob that
    /// names a directory also excludes that directory's entire subtree. With
    /// `glob`'s default options `*` matches across `/`, so wildcard patterns are
    /// greedy: `data/artifacts/*/klines` excludes `klines` at any depth under
    /// `data/artifacts`, not just one level down.
    Glob(glob::Pattern),
}

impl IgnorePattern {
    fn matches(&self, rel: &Path) -> bool {
        match self {
            IgnorePattern::Prefix(prefix) => rel.starts_with(prefix),
            IgnorePattern::Glob(pat) => rel.ancestors().any(|a| pat.matches_path(a)),
        }
    }
}

#[derive(Default)]
pub(crate) struct BackupIgnore(Vec<IgnorePattern>);

impl BackupIgnore {
    /// INVARIANT: Destructive restore cannot interpret an unreadable or malformed policy
    /// as permission to replace files. Keep capture's matching semantics.
    pub(crate) fn load_for_restore(workspace: &Path) -> Result<Self, BoxError> {
        let path = workspace.join("data/.backupignore");
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // INVARIANT: A dangling policy link is not an absent policy.
                if std::fs::symlink_metadata(&path).is_ok() {
                    return Err("snapshot restore ignore policy is unreadable".into());
                }
                return Ok(Self::default());
            }
            Err(_) => return Err("snapshot restore ignore policy is unreadable".into()),
        };
        for line in content.lines().map(str::trim) {
            if !line.is_empty() && !line.starts_with('#') && line.contains(['*', '?', '[']) {
                glob::Pattern::new(line.trim_end_matches('/'))
                    .map_err(|_| "snapshot restore ignore policy is invalid")?;
            }
        }
        Ok(Self::parse(&content))
    }

    pub(crate) fn normalized_patterns(&self) -> Result<Vec<String>, BoxError> {
        self.0
            .iter()
            .map(|pattern| match pattern {
                IgnorePattern::Prefix(path) => path
                    .to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "snapshot ignore pattern is not UTF-8".into()),
                IgnorePattern::Glob(pattern) => Ok(pattern.as_str().to_owned()),
            })
            .collect()
    }

    pub(crate) fn from_normalized_patterns(patterns: &[String]) -> Result<Self, BoxError> {
        for pattern in patterns {
            if pattern.is_empty()
                || pattern.trim() != pattern
                || pattern.contains(['\n', '\r'])
                || pattern.starts_with('#')
                || pattern.ends_with('/')
            {
                return Err("invalid frozen snapshot ignore pattern".into());
            }
            if pattern.contains(['*', '?', '[']) {
                glob::Pattern::new(pattern)
                    .map_err(|_| "invalid frozen snapshot ignore pattern")?;
            }
        }
        Ok(Self::parse(&patterns.join("\n")))
    }

    fn load(workspace: &Path) -> Self {
        let path = workspace.join("data").join(".backupignore");
        match std::fs::read_to_string(&path) {
            Ok(content) => Self::parse(&content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                crate::log_legacy!(
                    "[Backup] Could not read {} ({}); ignoring it",
                    path.display(),
                    e
                );
                Self::default()
            }
        }
    }

    fn parse(content: &str) -> Self {
        let mut patterns = Vec::new();
        for raw in content.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.trim_end_matches('/');
            if line.is_empty() {
                continue;
            }
            if line.contains(['*', '?', '[']) {
                match glob::Pattern::new(line) {
                    Ok(pat) => patterns.push(IgnorePattern::Glob(pat)),
                    Err(e) => crate::log_legacy!(
                        "[Backup] Skipping malformed .backupignore pattern {:?}: {}",
                        line,
                        e
                    ),
                }
            } else {
                patterns.push(IgnorePattern::Prefix(PathBuf::from(line)));
            }
        }
        Self(patterns)
    }

    fn matches(&self, rel: &Path) -> bool {
        self.0.iter().any(|p| p.matches(rel))
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub(crate) fn is_excluded_workspace_path(rel: &Path, ignore: &BackupIgnore) -> bool {
    if rel.starts_with(".lucidos") {
        return true;
    }
    let mut comps = rel.components();
    if let (Some(std::path::Component::Normal(d)), Some(std::path::Component::Normal(p))) =
        (comps.next(), comps.next())
    {
        let p = p.to_string_lossy();
        if d == "data" && (p == "postgres" || p.starts_with("postgres.")) {
            return true;
        }
    }
    ignore.matches(rel)
}

fn header_for_file(metadata: &std::fs::Metadata) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_size(metadata.len());
    header.set_mode(0o644);
    header.set_mtime(
        metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0),
    );
    header.set_cksum();
    header
}

fn skip_if_vanished<T>(path: &Path, result: std::io::Result<T>) -> Result<Option<T>, BoxError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            crate::log_legacy!("[Backup] Skipping vanished path {}: {}", path.display(), e);
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

/// INVARIANT: Yields exactly `len` bytes of `inner`: cut short if the source grew, and
/// zero-filled if it shrank.
///
/// A tar header states its entry's size before the data. The tar crate copies
/// whatever the reader yields and never checks it against that size. So a file
/// written while it is archived misaligns every entry after it, and the backup
/// reads as complete until a restore fails on it.
struct ExactLen<R> {
    inner: R,
    remaining: u64,
    padded: u64,
}

impl<R> ExactLen<R> {
    fn new(inner: R, len: u64) -> Self {
        Self {
            inner,
            remaining: len,
            padded: 0,
        }
    }
}

impl<R: std::io::Read> std::io::Read for ExactLen<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let want = buf
            .len()
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        if want == 0 {
            return Ok(0);
        }
        let n = match self.inner.read(&mut buf[..want])? {
            0 => {
                buf[..want].fill(0);
                self.padded += want as u64;
                want
            }
            n => n,
        };
        self.remaining -= n as u64;
        Ok(n)
    }
}

fn append_exact<W: std::io::Write, R: std::io::Read>(
    builder: &mut tar::Builder<W>,
    header: &mut tar::Header,
    archive_path: &Path,
    content: R,
) -> Result<u64, BoxError> {
    let mut content = ExactLen::new(content, header.size()?);
    builder.append_data(header, archive_path, &mut content)?;
    Ok(content.padded)
}

#[derive(Debug, PartialEq)]
enum CopyDrift {
    Grew { kept: u64 },
    Shrank { padded: u64 },
    Rewritten,
}

impl std::fmt::Display for CopyDrift {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Grew { kept } => write!(f, "grew; the archive holds its first {kept} bytes"),
            Self::Shrank { padded } => write!(f, "shrank; zero-filled its last {padded} bytes"),
            Self::Rewritten => write!(f, "was rewritten; the archive may hold a torn copy"),
        }
    }
}

fn copy_drift(
    before: &std::fs::Metadata,
    after: &std::fs::Metadata,
    padded: u64,
) -> Option<CopyDrift> {
    if padded > 0 {
        Some(CopyDrift::Shrank { padded })
    } else if after.len() > before.len() {
        Some(CopyDrift::Grew { kept: before.len() })
    } else if after.len() != before.len() || after.modified().ok() != before.modified().ok() {
        Some(CopyDrift::Rewritten)
    } else {
        None
    }
}

/// INVARIANT: Append a single file at `path` to the tar `builder` under archive path `archive_path`.
///
/// The size comes from the open handle, not from an earlier stat of the path. A
/// file replaced by a rename in between would otherwise bring another length. A
/// file written during the copy is logged, as a vanished one is.
fn append_file<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    path: &Path,
    archive_path: &Path,
) -> Result<(), BoxError> {
    let Some(file) = skip_if_vanished(path, std::fs::File::open(path))? else {
        return Ok(());
    };
    let before = file.metadata()?;
    if !before.is_file() {
        return Ok(());
    }
    let padded = append_exact(builder, &mut header_for_file(&before), archive_path, &file)?;
    if let Some(drift) = copy_drift(&before, &file.metadata()?, padded) {
        crate::log_legacy!(
            "[Backup] {} changed while it was archived: it {}",
            path.display(),
            drift
        );
    }
    Ok(())
}

fn tar_and_compress(
    workspace: &Path,
    sql_dump: &Path,
    secret_key: &Path,
    pending_dir: Option<&Path>,
    output: &Path,
    ignore: &BackupIgnore,
) -> Result<(), BoxError> {
    let file = std::fs::File::create(output)?;
    let encoder = zstd::Encoder::new(file, 3)?;
    let mut builder = tar::Builder::new(encoder);

    for path in walkdir(workspace)? {
        let rel = path.strip_prefix(workspace)?;

        if is_excluded_workspace_path(rel, ignore) {
            continue;
        }

        let Some(metadata) = skip_if_vanished(&path, std::fs::metadata(&path))? else {
            continue;
        };
        if metadata.is_file() {
            append_file(&mut builder, &path, rel)?;
        }
    }

    // INVARIANT: Add the SQL dump at the archive root. Unlike a workspace file, a missing
    // dump is an error: the backup is worthless without it.
    std::fs::metadata(sql_dump)?;
    append_file(&mut builder, sql_dump, Path::new("lucidos_backup.dump"))?;
    append_file(&mut builder, secret_key, Path::new(SECRET_STORE_KEY_ENTRY))?;
    if let Some(dir) = pending_dir {
        builder.append_dir_all(pending_bundles::ARCHIVE_DIR, dir)?;
    }

    let encoder = builder.into_inner()?;
    encoder.finish()?;

    Ok(())
}

/// INVARIANT: Recursively walk the workspace, returning all regular file paths.
///
/// One symlink is followed: a top-level `data` relocating the tree to another
/// disk. That layout is supported, and both coding-agent back ends grant it
/// through [`grants_more_than_the_data_tree`]. A backup skipping it would omit
/// every artifact, app, knowhow file and trigger the user has.
///
/// Every other symlink is skipped. Following one pulls its target into the
/// archive we upload to the user's cloud provider. `.backupignore` cannot
/// refuse that, because it matches the path INSIDE the workspace.
///
/// Skipping also makes a symlink loop unreachable. A `data` link onto a parent
/// of the workspace is refused, and no other link is entered, so the walk only
/// descends real directories.
fn walkdir(root: &Path) -> Result<Vec<PathBuf>, BoxError> {
    let mut results = Vec::new();
    let mut skipped = 0usize;
    walkdir_inner(root, root, &mut results, &mut skipped)?;
    if skipped > 0 {
        crate::log_legacy!(
            "[Backup] Skipped {} symlink(s); only a relocated {}/ is followed",
            skipped,
            crate::core::DATA_DIR
        );
    }
    Ok(results)
}

pub(crate) enum SymlinkAction {
    /// INVARIANT: The one relocation a backup follows. Recursion enters the LINK path
    /// rather than its target, so every child stays under the workspace and
    /// `is_excluded_workspace_path` keeps working on the relative path.
    Follow,
    Skip,
}

/// INVARIANT: Decide a symlink's fate, or FAIL the backup.
///
/// A top-level `data` link that does not resolve is an error, never a skip.
/// Skipping it archives a workspace holding no artifacts, apps, knowhow or
/// triggers. `workspace_backup_size` walks this same function, so the
/// preflight agrees and the run reports success. The user finds out at
/// restore. An unmounted external disk is the ordinary way to get here.
pub(crate) fn classify_symlink(path: &Path, root: &Path) -> Result<SymlinkAction, BoxError> {
    if path != root.join(crate::core::DATA_DIR) {
        return Ok(SymlinkAction::Skip);
    }
    let resolved = std::fs::canonicalize(path).map_err(|e| {
        format!(
            "{} is a symlink that does not resolve ({e}). Refusing to write a \
             backup with no data tree in it.",
            path.display()
        )
    })?;
    if !resolved.is_dir() {
        return Err(format!(
            "{} resolves to {}, which is not a directory. Refusing to write a \
             backup with no data tree in it.",
            path.display(),
            resolved.display()
        )
        .into());
    }
    if crate::core::grants::grants_more_than_the_data_tree(&resolved, root) {
        return Err(format!(
            "{} resolves to {}, which contains the workspace. Refusing to back \
             up the workspace's own runtime state and sibling worktrees.",
            path.display(),
            resolved.display()
        )
        .into());
    }
    Ok(SymlinkAction::Follow)
}

fn walkdir_inner(
    dir: &Path,
    root: &Path,
    results: &mut Vec<PathBuf>,
    skipped: &mut usize,
) -> Result<(), BoxError> {
    if !dir.is_dir() {
        return Ok(());
    }
    let Some(entries) = skip_if_vanished(dir, std::fs::read_dir(dir))? else {
        return Ok(());
    };
    for entry in entries {
        let Some(entry) = skip_if_vanished(dir, entry)? else {
            continue;
        };
        let path = entry.path();
        let Some(file_type) = skip_if_vanished(&path, entry.file_type())? else {
            continue;
        };
        if file_type.is_symlink() {
            match classify_symlink(&path, root)? {
                SymlinkAction::Follow => walkdir_inner(&path, root, results, skipped)?,
                SymlinkAction::Skip => {
                    *skipped += 1;
                    if dir == root {
                        crate::log_legacy!("[Backup] Not following symlink {}", path.display());
                    }
                }
            }
        } else if file_type.is_dir() {
            walkdir_inner(&path, root, results, skipped)?;
        } else if file_type.is_file() {
            results.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
