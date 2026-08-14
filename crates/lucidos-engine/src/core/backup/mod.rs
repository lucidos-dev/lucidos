pub mod crypto;
pub mod dropbox;
pub mod google_drive;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::core::oauth;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Registry entry: (id, name, oauth_provider, required_scopes, grant_scopes,
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

/// Preference key for the backup cron schedule expression.
pub const PREF_BACKUP_SCHEDULE: &str = "backup_schedule";
/// Preference key for the backup provider ID.
pub const PREF_BACKUP_PROVIDER: &str = "backup_provider";
/// Preference key for how many backups to keep (oldest are deleted after a new backup).
pub const PREF_BACKUP_RETENTION: &str = "backup_retention";
/// Default number of backups to keep when no preference is set.
pub const DEFAULT_BACKUP_RETENTION: usize = 5;
/// Preference key for the persisted outcome of the last backup run (success or
/// failure). Stored as a JSON `BackupLastRun` so the Settings → Backup page can
/// show "did the last run succeed or fail, and when?" even after an engine
/// restart — terminal outcome otherwise lives only in ephemeral SSE events.
pub const PREF_BACKUP_LAST_RUN: &str = "backup_last_run";

/// Check whether a backup schedule value represents an active (enabled) schedule.
pub fn is_schedule_active(value: &str) -> bool {
    !value.is_empty() && value != "off"
}

/// Read the backup retention count from preferences. A missing or unparseable
/// row is the default; a FAILED read is an `Err`.
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
    use crate::core::PreferenceStore;
    Ok(PreferenceStore::get(pool, PREF_BACKUP_RETENTION)
        .await?
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_BACKUP_RETENTION))
}

/// Static registry of all backup providers: (id, name, oauth_provider, required_scopes, constructor).
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
        // Never empty: an empty list makes every connected account read as
        // ready however narrow its grant. The failure then lands on
        // `files/create_folder_v2` returning a 400 mid-backup.
        dropbox::BACKUP_SCOPES,
        dropbox::GRANT_SCOPES,
        |pool, scopes| Box::new(dropbox::DropboxBackupProvider::new(pool, scopes)),
    ),
];

/// The valid backup provider ids — the agent-settable `backup_provider`
/// preference enum (see `core::preference_catalog`). Kept in sync with
/// [`PROVIDERS`] above by `provider_ids_match_registry` in the tests.
pub const PROVIDER_IDS: &[&str] = &["google_drive", "dropbox"];

/// Provider metadata including the OAuth provider name for readiness checks.
pub struct ProviderMeta {
    pub id: &'static str,
    pub name: &'static str,
    pub oauth_provider: &'static str,
    /// Every scope substring that must appear in the OAuth account's scopes for
    /// the provider to be ready. Empty slice means connecting is enough.
    ///
    /// This is the SAME list the provider's preflight checks, deliberately.
    /// Readiness gating on one scope while preflight demands three lets an
    /// account holding only the first read as ready. The page then hides *Grant
    /// access* and enables *Back up now*. Preflight fails and tells the user to
    /// reconnect, with no button left to do it.
    pub required_scopes: &'static [&'static str],
    /// The scopes an authorization actually asks for. Not a second readiness
    /// list: it exists so an unmet `required_scopes` matcher can be reported
    /// under a name the user can find in the provider's own console. See
    /// [`name_missing_scopes`].
    pub grant_scopes: &'static [&'static str],
}

/// List all registered backup providers with their metadata.
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

/// Look up one provider's metadata by id. `None` for an id not in [`PROVIDERS`].
pub fn provider_meta(provider_id: &str) -> Option<ProviderMeta> {
    list_providers().into_iter().find(|m| m.id == provider_id)
}

/// Does a granted-scope string carry `required`?
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

/// Which of `required` a granted-scope string is missing, in the order the
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

/// Rewrite a list of unmet requirements into the scopes a user can act on.
///
/// `required_scopes` are MATCHERS, not names. Readiness asks whether a granted
/// scope CONTAINS the entry ([`scopes_include`]). Google Drive's whole
/// requirement is therefore the fragment `drive`, matching the full
/// `https://www.googleapis.com/auth/drive.file` URL. Reporting that fragment to
/// a human names a "drive permission" that appears in no Google console, which
/// defeats the point of naming it at all.
///
/// So each unmet matcher is mapped to the first `grant_scopes` entry containing
/// it, applying the same containment rule from the other side. Dropbox's
/// requirements are already whole scope names and map to themselves, which is
/// also exactly what its App Console lists. A matcher no grant scope contains
/// falls back to itself: a provider whose two lists have drifted should still
/// say WHICH permission is short rather than drop it.
///
/// Engine-side on purpose. The page and `get_backup_status` both render what
/// this returns, so they cannot name a requirement differently.
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

/// Whether a backup provider can actually upload right now.
///
/// Both halves of "is this working?" in one value: is the OAuth account
/// connected at all, and does that account carry the scope the provider needs.
/// Shared by the Settings page (`api::backup::list_providers`) and the agent's
/// `get_backup_status`. The human and the agent can never be told different
/// things about the same provider. With only the page knowing, an agent reports
/// a backup as fully configured while its upload leg has no account behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderReadiness {
    /// An OAuth account row exists for this provider.
    pub connected: bool,
    /// Which of the provider's `required_scopes` the connected account does NOT
    /// carry, in the order the provider declared them. Empty when the provider
    /// is ready, and also empty when nothing is connected: there is no grant to
    /// measure, and listing every scope as "missing" would describe a
    /// not-connected provider as a half-granted one.
    ///
    /// Reported all the way to the Backup page. "Access not granted" with no
    /// permission named cannot distinguish a grant that never happened from one
    /// that came back a scope short. A user presses *Grant access*, completes
    /// the authorization, and meets the same red line.
    pub missing_scopes: Vec<&'static str>,
}

impl ProviderReadiness {
    /// Connected AND holding every required scope.
    ///
    /// Derived rather than stored, deliberately. Keeping both a boolean and the
    /// list it is computed from allows a state where they disagree, and a method
    /// does not. See `docs/plans/2026-08-05-dropbox-backup-scopes-and-refresh.md`
    /// for the two reconciliations that made the point.
    pub fn ready(&self) -> bool {
        self.connected && self.missing_scopes.is_empty()
    }
}

/// Resolve [`ProviderReadiness`] for one provider.
///
/// A DB error propagates rather than degrading to "not connected": a transient
/// failure must not be reported to a user (or an agent) as a missing account.
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
    // Named on the way out, so every consumer reports one string for one gap.
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

/// Create a backup provider by ID.
pub fn get_provider(provider_id: &str, pool: &PgPool) -> Result<Box<dyn BackupProvider>, String> {
    PROVIDERS
        .iter()
        .find(|(id, _, _, _, _, _)| *id == provider_id)
        .map(|(_, _, _, scopes, _, ctor)| ctor(pool.clone(), scopes))
        .ok_or_else(|| format!("Unknown backup provider: {}", provider_id))
}

/// Get the connected OAuth account for a backup provider, refreshing its token
/// if needed. Callers that only want the token use [`get_oauth_token`]; the
/// whole account is what a scope preflight needs, since the granted `scopes`
/// live on the row.
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
                AccountLookupError::DbError(err) | AccountLookupError::RefreshFailed(err) => err,
            }
        })
}

/// Get an OAuth token for a backup provider, refreshing if needed.
pub async fn get_oauth_token(pool: &PgPool, provider: &str) -> Result<String, BoxError> {
    Ok(get_oauth_account(pool, provider).await?.access_token)
}

/// Metadata for a single backup stored by a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    pub id: String,
    pub filename: String,
    pub size_bytes: u64,
    pub created_at: DateTime<Utc>,
}

/// Terminal outcome of a backup run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackupRunStatus {
    Success,
    Failure,
}

/// Persisted record of the most recent backup run's terminal outcome. Written
/// by `run_backup` on both paths and stored under `PREF_BACKUP_LAST_RUN`, so the
/// page survives engine restarts. The `BackupCompleted` and `BackupFailed` SSE
/// events are ephemeral.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupLastRun {
    pub status: BackupRunStatus,
    /// When the run reached its terminal state (RFC 3339).
    pub at: DateTime<Utc>,
    /// When the run STARTED (RFC 3339). `None` for records written before
    /// start-time tracking existed (`#[serde(default)]` keeps them readable); the
    /// duration is `at - started_at` when present.
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    /// Filename of the produced backup (success only).
    pub filename: Option<String>,
    /// Size of the produced backup in bytes (success only).
    pub size_bytes: Option<u64>,
    /// Error message (failure only).
    pub error: Option<String>,
}

impl BackupLastRun {
    /// Build a success record from the produced backup entry and the run's
    /// start time (the terminal time is stamped now).
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

    /// Build a failure record from the error message and the run's start time.
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

/// Persist the last-run outcome under `PREF_BACKUP_LAST_RUN`. Failure to
/// persist is returned to the caller so it can log it — the health card would
/// otherwise silently lag behind the real outcome.
pub async fn persist_last_run(pool: &PgPool, run: &BackupLastRun) -> Result<(), BoxError> {
    use crate::core::PreferenceStore;
    let value = serde_json::to_string(run)?;
    PreferenceStore::set_silent(pool, PREF_BACKUP_LAST_RUN, &value).await?;
    Ok(())
}

/// Read the persisted last-run outcome. Returns `None` when never recorded; a
/// malformed stored value is logged and treated as absent rather than failing
/// the whole status response.
pub async fn load_last_run(pool: &PgPool) -> Option<BackupLastRun> {
    use crate::core::PreferenceStore;
    let raw = PreferenceStore::get(pool, PREF_BACKUP_LAST_RUN)
        .await
        .ok()
        .flatten()?;
    match serde_json::from_str(&raw) {
        Ok(run) => Some(run),
        Err(e) => {
            crate::log!("[Backup] Ignoring malformed {PREF_BACKUP_LAST_RUN}: {e}");
            None
        }
    }
}

/// One historical backup run, reconstructed from a persisted `BackupCompleted` /
/// `BackupFailed` event. The `events` table IS the durable run history (start /
/// finish / size); this is its read model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupRunSummary {
    pub status: BackupRunStatus,
    /// When the run started (RFC 3339); `None` only for malformed/legacy rows.
    pub started_at: Option<DateTime<Utc>>,
    /// When the run reached its terminal state.
    pub finished_at: DateTime<Utc>,
    /// Produced archive filename (success only).
    pub filename: Option<String>,
    /// Encrypted archive size in bytes (success only).
    pub size_bytes: Option<u64>,
    /// Error message (failure only).
    pub error: Option<String>,
}

/// Load the most recent backup runs (newest first, capped at `limit`) from the
/// persisted `BackupCompleted` / `BackupFailed` events — the durable run history.
/// A query error logs and returns an empty list rather than failing the caller
/// (the status surface degrades to "no history", never an error).
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
            crate::log!("[Backup] Failed to load recent runs: {e}");
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

/// Reconstruct a [`BackupRunSummary`] from one persisted backup event row. Pure
/// (no DB) so the payload-shape parsing is unit-testable. System events persist
/// as `{ "type": .., "data": { .. } }` (the `#[serde(tag = "type", content =
/// "data")]` shape); a bare data object is tolerated too. `created` is the
/// fallback for `finished_at` if the payload lacks it.
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

/// Trait for cloud storage backends that can store/retrieve encrypted backups.
#[async_trait]
pub trait BackupProvider: Send + Sync {
    fn name(&self) -> &str;
    fn id(&self) -> &str;
    /// The OAuth provider name used for token lookup (e.g. "google", "dropbox").
    fn oauth_provider(&self) -> &str;
    /// Web URL to this provider's backups folder, for the Settings → Backup
    /// "View backups folder" link. `None` when the provider can't form one.
    async fn folder_url(&self) -> Option<String>;
    /// Fail-fast checks run BEFORE any expensive backup work (pg_dump, the
    /// multi-GB tar, the multi-minute encrypt). `estimated_upload_bytes` is the
    /// orchestrator's estimate of the encrypted archive size. The provider uses
    /// it to reject the run up front when the upload cannot succeed, rather than
    /// failing at the final upload. Also verifies the token is valid and the
    /// required scope exists.
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

/// Returns the path to the backup encryption key file for a workspace.
pub fn key_file_path(workspace: &Path) -> PathBuf {
    workspace.join(".lucidos").join("backup.key")
}

/// The workspace directory name used in backup archive filenames
/// (`lucidos-backup-{name}-{timestamp}.enc`). Falls back to `"workspace"` when
/// the path has no final component. The upload path and the prune matcher both
/// derive the name from here, so they can never disagree on which archives are
/// "this workspace's".
pub fn workspace_archive_name(workspace: &Path) -> &str {
    workspace
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace")
}

/// Parse the workspace name a backup archive was produced for, from its
/// filename. `create_backup` stamps `lucidos-backup-{name}-{YYYYMMDD-HHMMSS}.enc`
/// (see [`workspace_archive_name`]); this is the inverse, used by the gateway
/// picker to derive a default workspace name for a restore. The `{name}` may
/// itself contain hyphens (`e2e-test`), so we strip the fixed prefix/suffix and
/// then the trailing `-YYYYMMDD-HHMMSS` timestamp (shape-validated via
/// [`is_backup_timestamp`]). Returns `None` for anything not in that exact shape
/// — the caller then asks the user for a name.
pub fn parse_workspace_name_from_archive(filename: &str) -> Option<String> {
    let rest = filename
        .strip_prefix("lucidos-backup-")?
        .strip_suffix(".enc")?;
    // Split off the trailing `-HHMMSS`, then the `-YYYYMMDD` before it; whatever
    // remains is the workspace name. Both halves must be timestamp-shaped.
    let (before_time, time) = rest.rsplit_once('-')?;
    let (name, date) = before_time.rsplit_once('-')?;
    if name.is_empty() || !is_backup_timestamp(&format!("{date}-{time}")) {
        return None;
    }
    Some(name.to_string())
}

/// Restore a local encrypted backup archive INTO an already-provisioned
/// workspace directory and database. This is the gateway picker's restore path.
/// The gateway has already reserved the registry entry, provisioned Postgres,
/// and created `workspace_dir`. Here we only decrypt, decompress, move the
/// workspace files in, and `pg_restore` the dump into `database_url`.
///
/// Deliberately NARROWER than the old cloud `restore_backup`:
///   * **No provider / download** — the archive is already on disk (`encrypted_path`).
///   * **No `init-workspace.sh` / `~/workspaces/{name}`** — the gateway owns
///     provisioning and the dir (which may live under the gateway app-data dir).
///   * **No `user_dir/` extraction over `~/.lucidos`.** Current backups no longer
///     include `~/.lucidos` at all (see `tar_and_compress`). LEGACY archives
///     carry it under `user_dir/`, and applying that on restore would CLOBBER
///     the target's gateway registry and other machine-global state. So we drop
///     any `user_dir/` from the staging tree before moving files in.
///   * **No migrations** — the engine server the gateway spawns afterward runs
///     `sqlx::migrate!()` at construction, upgrading an older-schema restore.
pub async fn restore_archive_into(
    workspace_dir: &Path,
    database_url: &str,
    key: &[u8],
    encrypted_path: &Path,
    progress: impl Fn(&str, usize, usize) + Send + Sync + 'static,
) -> Result<(), BoxError> {
    // Fail fast (before decrypt/decompress) if the bundled PG client tools can't
    // be resolved — restore drives pg_restore + psql. Path-bearing message.
    pg_tools_preflight(database_url)?;

    let temp_dir = tempfile::tempdir()?;
    let progress = Arc::new(progress);

    // Phase weights — there is no download phase (the file is already local), so
    // decrypt starts at 0 and restore fills the remainder to 100.
    let enc_bytes = std::fs::metadata(encrypted_path)?.len();
    let (decrypt_end, decompress_end) = estimate_restore_weights(enc_bytes, 0);

    // Phases 1-2: decrypt + decompress/untar into staging, then drop user_dir/
    // (blocking I/O on the blocking pool).
    let staging_dir = {
        let progress = progress.clone();
        let encrypted_path = encrypted_path.to_path_buf();
        let key = key.to_vec();
        let temp_path = temp_dir.path().to_path_buf();

        tokio::task::spawn_blocking(move || -> Result<PathBuf, BoxError> {
            // Phase 1: decrypt
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
                crypto::decrypt(&key, reader, &mut output)?;
            }

            // Phase 2: decompress + untar into staging
            progress("decompressing", decrypt_end, 100);
            let staging_dir = temp_path.join("staging");
            std::fs::create_dir_all(&staging_dir)?;
            {
                let file = std::fs::File::open(&compressed_path)?;
                let decoder = zstd::Decoder::new(file)?;
                let mut archive = tar::Archive::new(decoder);
                archive.unpack(&staging_dir)?;
            }

            // Drop any `user_dir/`, the source's ~/.lucidos, so it is neither
            // applied over the target's ~/.lucidos nor littered into the restored
            // workspace. Current backups no longer include it (see
            // `tar_and_compress`); this stays as defense for LEGACY archives.
            let user_dir_staging = staging_dir.join("user_dir");
            if user_dir_staging.exists() {
                std::fs::remove_dir_all(&user_dir_staging)?;
            }

            Ok(staging_dir)
        })
        .await??
    };

    // Phase 3: move staged files into the (gateway-created) workspace dir.
    progress("initializing", decompress_end, 100);
    {
        let workspace_dir = workspace_dir.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<(), BoxError> {
            std::fs::create_dir_all(&workspace_dir)?;
            move_contents(&staging_dir, &workspace_dir)
        })
        .await??;
    }

    // Phase 4: restore the database dump into the provisioned Postgres. Skipped
    // entirely when the archive carries no dump (no Postgres work, no psql) — a
    // real backup always includes `lucidos_backup.dump`.
    progress("restoring_db", decompress_end + 1, 100);
    {
        let ws_dir = workspace_dir.to_path_buf();
        let database_url = database_url.to_string();
        tokio::task::spawn_blocking(move || -> Result<(), BoxError> {
            let dump_path = ws_dir.join("lucidos_backup.dump");
            if dump_path.exists() {
                terminate_other_connections(&database_url)?;
                pg_restore(&database_url, &dump_path)?;
                let _ = std::fs::remove_file(&dump_path);
            }
            Ok(())
        })
        .await??;
    }

    progress("done", 100, 100);
    Ok(())
}

/// Create an encrypted backup of a workspace and its database.
///
/// Pipeline: pg_dump -> tar (exclude .lucidos/) -> zstd compress -> AES-256-GCM encrypt -> upload
///
/// Phases 1-3 (pg_dump, compress, encrypt) run in `spawn_blocking` so they don't
/// starve the tokio runtime — this ensures SSE progress events reach the client.
pub async fn create_backup(
    workspace: &Path,
    database_url: &str,
    key: &[u8],
    provider: &dyn BackupProvider,
    progress: impl Fn(&str, usize, usize) + Send + Sync + 'static,
) -> Result<BackupEntry, BoxError> {
    crate::log!(
        "[Backup] Starting backup to {} (provider: {})",
        provider.name(),
        provider.id()
    );

    // Fail fast if the bundled PG client tools cannot be resolved, with a
    // path-bearing message, before any upload-size walk, preflight or encryption.
    pg_tools_preflight(database_url)?;

    // Estimate the upload size up front, in one workspace walk on the blocking
    // pool. The provider's preflight can then fail fast on a quota, scope or
    // token problem, BEFORE pg_dump, the multi-GB tar and the encrypt. The tar
    // phase still walks once more to stream the files.
    let ws_bytes = {
        let workspace = workspace.to_path_buf();
        tokio::task::spawn_blocking(move || {
            workspace_backup_size(&workspace, &BackupIgnore::load(&workspace))
        })
        .await?
    };

    // PREFLIGHT — token, required scope, and free space. The only gate before
    // expensive work; an actionable error here costs seconds, not ~25 minutes.
    provider.preflight(estimate_archive_size(ws_bytes)).await?;

    let temp_dir = tempfile::tempdir()?;
    let progress = Arc::new(progress);

    // Phases 1-3: blocking I/O — run on the blocking thread pool
    let (encrypted_path, encrypt_end) = {
        let progress = progress.clone();
        let workspace = workspace.to_path_buf();
        let database_url = database_url.to_string();
        let key = key.to_vec();
        let temp_path = temp_dir.path().to_path_buf();

        tokio::task::spawn_blocking(move || -> Result<(PathBuf, usize), BoxError> {
            // Load data/.backupignore once for this run — shared by the tar walk
            // (the workspace can have tens of thousands of entries; never
            // re-parse per file).
            let ignore = BackupIgnore::load(&workspace);

            // Phase weights from the size already computed for preflight.
            progress("estimating", 0, 100);
            let (dump_end, compress_end, encrypt_end) = estimate_weights(ws_bytes);

            // Phase 1: pg_dump (0% → dump_end%)
            crate::log!("[Backup] Phase 1/4: pg_dump");
            progress("dumping_db", 1, 100);
            let dump_path = temp_path.join("lucidos_backup.dump");
            pg_dump(&database_url, &dump_path)?;

            // Phase 2: tar + zstd compress (dump_end% → compress_end%)
            crate::log!("[Backup] Phase 2/4: tar + zstd compress");
            progress("compressing", dump_end, 100);
            let compressed_path = temp_path.join("backup.tar.zst");
            tar_and_compress(&workspace, &dump_path, &compressed_path, &ignore)?;

            // Phase 3: encrypt with per-chunk progress (compress_end% → encrypt_end%)
            crate::log!("[Backup] Phase 3/4: encrypt");
            progress("encrypting", compress_end, 100);
            let encrypted_path = temp_path.join("backup.enc");
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
                crypto::encrypt(&key, reader, &mut output)?;
            }

            Ok((encrypted_path, encrypt_end))
        })
        .await??
    };

    // Phase 4: upload with progress (encrypt_end% → 100%)
    crate::log!("[Backup] Phase 4/4: upload");
    progress("uploading", encrypt_end, 100);
    let workspace_name = workspace_archive_name(workspace);
    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("lucidos-backup-{workspace_name}-{timestamp}.enc");

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
    crate::log!("[Backup] Complete: {} ({} bytes)", filename, size_bytes);

    Ok(BackupEntry {
        id: backup_id,
        filename,
        size_bytes,
        created_at: Utc::now(),
    })
}

/// True when `s` is a backup-filename timestamp of the form `YYYYMMDD-HHMMSS`
/// (8 digits, a hyphen, 6 digits) — the shape `create_backup` stamps via
/// `Utc::now().format("%Y%m%d-%H%M%S")`. Validating it lets the archive matcher
/// reject files whose name merely shares a workspace's prefix (e.g. workspace
/// `myws` vs `myws-2`, where `lucidos-backup-myws-2-…` belongs to
/// the latter).
fn is_backup_timestamp(s: &str) -> bool {
    match s.split_once('-') {
        Some((date, time)) => {
            date.len() == 8
                && time.len() == 6
                && date.bytes().all(|b| b.is_ascii_digit())
                && time.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    }
}

/// True when `filename` is an encrypted backup archive produced by THIS
/// workspace. It must match `lucidos-backup-{workspace_name}-{timestamp}.enc`
/// exactly, timestamp shape included. This is the guard that keeps pruning off
/// another workspace's archives, and off any unrelated file in the shared
/// folder.
fn is_own_backup_archive(filename: &str, workspace_name: &str) -> bool {
    let prefix = format!("lucidos-backup-{workspace_name}-");
    filename
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix(".enc"))
        .is_some_and(is_backup_timestamp)
}

/// From everything the provider lists in the shared backup folder, choose which
/// archives to delete: narrow to THIS workspace's archives, keep the newest
/// `keep`, and return the remainder OLDEST-FIRST for deletion. Pure so the
/// selection rule is unit-testable without a live provider.
fn select_prunable(
    entries: Vec<BackupEntry>,
    workspace_name: &str,
    keep: usize,
) -> Vec<BackupEntry> {
    let mut own: Vec<BackupEntry> = entries
        .into_iter()
        .filter(|e| is_own_backup_archive(&e.filename, workspace_name))
        .collect();
    if own.len() <= keep {
        return Vec::new();
    }
    // Newest first, then split off the newest `keep` to retain; the tail is
    // what we delete. Reverse it so deletion proceeds oldest-first.
    own.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let mut to_delete = own.split_off(keep);
    to_delete.reverse();
    to_delete
}

/// Delete this workspace's old backups beyond the retention limit, keeping the
/// newest `keep`.
///
/// Lists everything in the provider's shared backup folder, narrows to THIS
/// workspace's `lucidos-backup-{workspace_name}-*.enc` archives, then deletes
/// oldest-first. Another workspace's backups and any unrelated file in the
/// folder are never touched (see [`select_prunable`]). Returns the number of
/// backups deleted. A single delete failure is logged and skipped rather than
/// propagated: one stuck archive must not abort cleanup of the rest.
pub async fn prune_old_backups(
    provider: &dyn BackupProvider,
    workspace_name: &str,
    keep: usize,
) -> Result<usize, BoxError> {
    if keep == 0 {
        return Ok(0);
    }
    let entries = provider.list_backups().await?;
    let to_delete = select_prunable(entries, workspace_name, keep);
    let mut deleted = 0;
    for entry in to_delete {
        match provider.delete(&entry.id).await {
            Ok(()) => {
                crate::log!("[Backup] Pruned old backup: {}", entry.filename);
                deleted += 1;
            }
            Err(e) => {
                crate::log!("[Backup] Failed to prune {}: {}", entry.filename, e);
            }
        }
    }
    Ok(deleted)
}

/// Sum the on-disk size of every file that WILL be included in the backup tar
/// (i.e. after the hardcoded + `.backupignore` exclusions). One walk, shared by
/// preflight's free-space estimate and the phase-weight estimate. The tar phase
/// walks once more to actually stream the files.
fn workspace_backup_size(workspace: &Path, ignore: &BackupIgnore) -> u64 {
    walkdir(workspace)
        .unwrap_or_default()
        .iter()
        .filter(|p| {
            p.strip_prefix(workspace)
                .map(|rel| !is_excluded_workspace_path(rel, ignore))
                .unwrap_or(true)
        })
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum::<u64>()
}

/// Estimate the size of the *encrypted archive that will be uploaded* from the
/// raw workspace size. The pipeline tar+zstd-compresses (~3:1) then AES-GCM
/// encrypts (adds only ~16 bytes per chunk), so the upload is ≈ a third of the
/// workspace. Used by preflight's free-space check; matches the 0.33 ratio
/// `estimate_weights` uses for `compressed_mb` so the two stay consistent.
fn estimate_archive_size(ws_bytes: u64) -> u64 {
    ws_bytes / 3
}

/// Estimate progress weights for each backup phase from the (already-computed)
/// workspace size in bytes.
///
/// Returns `(dump_end, compress_end, encrypt_end)` as percentages (0-100).
/// Upload fills the remainder to 100%.
///
/// Rough speed estimates from real workspaces:
///
/// - pg_dump: ~50 MB/s plus 2s overhead
/// - compress: ~25 MB/s (tar + zstd level 3)
/// - encrypt: ~5 MB/s (AES-256-GCM, 1 MB chunks)
/// - upload: ~10 MB/s, network dependent
fn estimate_weights(ws_bytes: u64) -> (usize, usize, usize) {
    let ws_mb = ws_bytes as f64 / 1_048_576.0;
    let compressed_mb = ws_mb * 0.33; // zstd ~3:1 ratio estimate

    let dump_time = ws_mb / 50.0 + 2.0;
    let compress_time = ws_mb / 25.0;
    let encrypt_time = compressed_mb / 5.0;
    let upload_time = compressed_mb / 10.0;
    let total = (dump_time + compress_time + encrypt_time + upload_time).max(1.0);

    let dump_pct = ((dump_time / total) * 100.0) as usize;
    let compress_pct = ((compress_time / total) * 100.0) as usize;
    let encrypt_pct = ((encrypt_time / total) * 100.0) as usize;

    // Ensure each phase gets at least 2% so the bar visibly moves,
    // and cap so later phases always have room.
    let dump_end = dump_pct.clamp(2, 90);
    let compress_end = (dump_end + compress_pct.max(2)).min(93);
    let encrypt_end = (compress_end + encrypt_pct.max(2)).min(97);

    (dump_end, compress_end, encrypt_end)
}

/// Estimate progress weights for restore phases based on encrypted file size.
///
/// Returns `(decrypt_end, decompress_end)` as percentages (0-100).
/// `download_end` is the starting point (download phase already accounted for).
/// Restore DB fills the remainder to 100%.
///
/// Rough speed estimates:
///   decrypt    ~5 MB/s  (AES-256-GCM, 1 MB chunks)
///   decompress ~50 MB/s (zstd + untar to disk)
///   restore    ~20 MB/s (psql import) + 2s overhead
fn estimate_restore_weights(enc_bytes: u64, download_end: usize) -> (usize, usize) {
    let enc_mb = enc_bytes as f64 / 1_048_576.0;
    // Encrypted ≈ compressed (encryption adds ~16 bytes/MB overhead)
    let compressed_mb = enc_mb;
    // Estimate uncompressed size (zstd ~3:1)
    let uncompressed_mb = compressed_mb * 3.0;

    let decrypt_time = enc_mb / 5.0;
    let decompress_time = compressed_mb / 50.0;
    let restore_time = uncompressed_mb / 20.0 + 2.0;
    let total = (decrypt_time + decompress_time + restore_time).max(1.0);

    let remaining = 100 - download_end;
    let decrypt_pct = ((decrypt_time / total) * remaining as f64) as usize;
    let decompress_pct = ((decompress_time / total) * remaining as f64) as usize;

    // Ensure each phase gets at least 2% so the bar visibly moves
    let decrypt_end = (download_end + decrypt_pct.max(2)).min(95);
    let decompress_end = (decrypt_end + decompress_pct.max(2)).min(97);

    (decrypt_end, decompress_end)
}

/// Read wrapper that calls a callback with total bytes read so far.
/// Used to report fine-grained progress during encrypt/decrypt.
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

/// Move all entries from `src` into `dest`, preserving directory structure.
/// Used to move staged restore contents into the workspace after validation.
fn move_contents(src: &Path, dest: &Path) -> Result<(), BoxError> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        // Try rename first (fast, same filesystem), fall back to copy+delete
        if let Err(e) = std::fs::rename(&src_path, &dest_path) {
            crate::log!(
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

/// Recursively copy a directory tree.
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

/// Apply `pg_env_vars(database_url)` onto `cmd`. Returns `Err` when the URL
/// does not match the expected shape, so the caller fails loudly. Otherwise it
/// spawns a subprocess that inherits zero `PG*` vars and gets a confusing
/// connection error from libpq.
fn apply_pg_env(cmd: &mut std::process::Command, database_url: &str) -> Result<(), BoxError> {
    let vars = crate::core::pg_env_vars(database_url);
    if vars.is_empty() {
        return Err(
            "database URL does not match expected postgres(ql)://user:pass@host[:port]/db shape"
                .into(),
        );
    }
    for (k, v) in vars {
        cmd.env(k, v);
    }
    Ok(())
}

/// Build a `Command` for a PostgreSQL client tool (`pg_dump` / `pg_restore` /
/// `psql`), resolved + ready to run.
///
/// In a packaged build the relocatable PG client binaries live at
/// `<resources>/postgres/bin` (`LUCIDOS_PG_BIN_DIR`), which is NOT on the
/// launchd minimal PATH. A bare `Command::new("pg_dump")` ENOENTs mid-backup.
/// So resolve against `LUCIDOS_PG_BIN_DIR` when set, failing fast with the path
/// if the binary is missing. Make the bundled libpq loadable via
/// `DYLD_LIBRARY_PATH` or `LD_LIBRARY_PATH` from `LUCIDOS_PG_LIB_DIR`. In dev
/// neither var is set and we fall back to a bare PATH lookup.
fn pg_tool_command(name: &str, database_url: &str) -> Result<std::process::Command, BoxError> {
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

/// Pure resolution of a PG client tool path: an absolute `<bin_dir>/<name>`
/// (must exist — fail fast with the path) when `bin_dir` is given, else the bare
/// `name` for a PATH lookup. Split out from `pg_tool_command` so the
/// resolution + the missing-binary error are unit-testable without env/spawn.
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

/// Fail fast at the START of a backup or restore if the bundled PG client tools
/// cannot be resolved, with a path-bearing error. The alternative is a cryptic
/// ENOENT deep inside `pg_dump`, `pg_restore` or `psql`, after the run has
/// already started. A no-op in dev, where `LUCIDOS_PG_BIN_DIR` is unset and the
/// bare PATH lookup is validated when the tool actually runs.
fn pg_tools_preflight(database_url: &str) -> Result<(), BoxError> {
    if std::env::var_os("LUCIDOS_PG_BIN_DIR").is_none() {
        return Ok(());
    }
    for name in ["pg_dump", "pg_restore", "psql"] {
        pg_tool_command(name, database_url)?;
    }
    Ok(())
}

/// Run pg_dump to export the database in custom archive format.
///
/// Custom format (`-Fc`) is a compressed binary format restored via `pg_restore`.
/// It avoids PostgreSQL 18's `\restrict` / `\unrestrict` psql meta-commands
/// (CVE-2025-8714) that break plain-text restores, and supports parallel restore.
///
/// Connection details flow through libpq `PG*` env vars so the password
/// stays out of argv.
fn pg_dump(database_url: &str, output_path: &Path) -> Result<(), BoxError> {
    let mut cmd = pg_tool_command("pg_dump", database_url)?;
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

/// The target database name from a postgres URL, for `psql --dbname`.
///
/// We pull the name from the same `pg_env_vars` parse that seeds the `PG*` env.
/// Host, port, user and password still flow via env, kept out of argv, and only
/// the dbname lands on the command line.
fn pg_dbname(database_url: &str) -> Result<String, BoxError> {
    crate::core::pg_env_vars(database_url)
        .into_iter()
        .find_map(|(k, v)| (k == "PGDATABASE").then_some(v))
        .ok_or_else(|| -> BoxError {
            "database URL does not match expected postgres(ql)://user:pass@host[:port]/db shape"
                .into()
        })
}

/// Session-level SET parameters that exist in newer PostgreSQL versions but not
/// older ones. When restoring a dump created by a newer pg_dump into an older
/// server, these SET statements cause a fatal error. We strip them from the SQL
/// before piping to psql.
const CROSS_VERSION_SET_PARAMS: &[&str] = &["transaction_timeout"];

/// Returns true if `line` is a `SET <param> = ...;` for a parameter in
/// [`CROSS_VERSION_SET_PARAMS`]. Pure for testability.
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

/// Restore a database from a custom-format dump file using pg_restore.
///
/// Pipes `pg_restore --file=-` (SQL to stdout) through a filter that strips
/// session SET statements for parameters that don't exist in older PostgreSQL
/// versions (e.g. `transaction_timeout` added in PG 17), then feeds the
/// filtered SQL to `psql --single-transaction`. This makes cross-version
/// restores (e.g. PG 17 dump → PG 16 server) work without losing atomicity.
fn pg_restore(database_url: &str, dump_path: &Path) -> Result<(), BoxError> {
    use std::io::{BufRead, BufReader, Write};

    let dbname = pg_dbname(database_url)?;

    // Step 1: pg_restore outputs SQL to stdout
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

    // Step 2: filter out cross-version SET statements
    let reader = BufReader::new(&restore_output.stdout[..]);
    let mut filtered = Vec::with_capacity(restore_output.stdout.len());
    for line in reader.split(b'\n') {
        let line = line?;
        let text = String::from_utf8_lossy(&line);
        if !is_cross_version_set(&text) {
            filtered.extend_from_slice(&line);
            filtered.push(b'\n');
        }
    }

    // Step 3: pipe filtered SQL to psql inside a single transaction
    let mut psql_cmd = pg_tool_command("psql", database_url)?;
    let mut psql = psql_cmd
        .args(["--single-transaction", "--dbname", &dbname])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    // Write stdin on a separate thread while `wait_with_output` drains
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

/// Terminate all other database connections to allow pg_restore --clean to drop objects.
///
/// The engine's connection pool holds connections that may have cached prepared
/// statements. pg_restore --clean needs to DROP and recreate tables, which requires
/// no other sessions holding locks. After termination, the pool auto-reconnects.
fn terminate_other_connections(database_url: &str) -> Result<(), BoxError> {
    let mut cmd = pg_tool_command("psql", database_url)?;
    let output = cmd
        .arg("-c")
        .arg("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = current_database() AND pid <> pg_backend_pid()")
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        crate::log!(
            "[Backup] Warning: failed to terminate other connections: {}",
            stderr
        );
    }
    Ok(())
}

/// A single parsed `.backupignore` rule.
enum IgnorePattern {
    /// A plain path with no wildcards. Matches the path and everything under
    /// it via component-prefix comparison — identical semantics to the
    /// hardcoded `rel.starts_with(...)` checks.
    Prefix(PathBuf),
    /// A pattern containing glob wildcards (`*`, `?`, `[`). Matched against the
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

/// Parsed contents of a workspace's `data/.backupignore` — a gitignore-style
/// list of workspace-relative paths to omit from the backup, *in addition* to
/// the hardcoded exclusions. Loaded and parsed once per backup run, and shared
/// across the size estimate and the tar walk. An absent or empty file yields an
/// empty set.
#[derive(Default)]
struct BackupIgnore(Vec<IgnorePattern>);

impl BackupIgnore {
    /// Load `<workspace>/data/.backupignore`. A missing file is the common case
    /// and yields no patterns silently. Any other read error is logged and also
    /// treated as no patterns: a broken ignore file never aborts the backup.
    fn load(workspace: &Path) -> Self {
        let path = workspace.join("data").join(".backupignore");
        match std::fs::read_to_string(&path) {
            Ok(content) => Self::parse(&content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                crate::log!(
                    "[Backup] Could not read {} ({}); ignoring it",
                    path.display(),
                    e
                );
                Self::default()
            }
        }
    }

    /// Parse the file body: one pattern per line, blank lines and `#` comments
    /// skipped, a trailing `/` tolerated. Patterns with wildcard chars compile
    /// to `glob::Pattern`; the rest are plain component-prefixes. A malformed
    /// glob is logged and skipped rather than aborting the parse.
    fn parse(content: &str) -> Self {
        let mut patterns = Vec::new();
        for raw in content.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // Tolerate a trailing slash: `foo/` behaves like `foo`.
            let line = line.trim_end_matches('/');
            if line.is_empty() {
                continue;
            }
            if line.contains(['*', '?', '[']) {
                match glob::Pattern::new(line) {
                    Ok(pat) => patterns.push(IgnorePattern::Glob(pat)),
                    Err(e) => crate::log!(
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

    /// True if any parsed pattern matches the workspace-relative path.
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

/// True if a workspace-relative path must be omitted from the backup tar
/// and from the size estimate.
///
/// Excludes (hardcoded, always — cheap checks first):
/// - `.lucidos/` — ephemeral runtime/cache
/// - `data/postgres/` — live PGDATA, captured via pg_dump
/// - `data/postgres.*/` — archived PGDATA siblings (e.g.
///   `postgres.migrated-<ts>/` left by `scripts/lib/workspace.sh` after the
///   one-time bind-mount → Docker-volume migration). These can be many GB
///   and are redundant with pg_dump.
///
/// Then, in addition, any path matched by the workspace's `.backupignore`
/// (`ignore`) — see [`BackupIgnore`].
fn is_excluded_workspace_path(rel: &Path, ignore: &BackupIgnore) -> bool {
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

/// Build a tar header for a file, copying size + mtime from filesystem metadata.
/// Falls back to mtime=0 when the OS doesn't expose modified time (best-effort,
/// matches tar's "no mtime known" convention).
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

/// Classify an `io::Result` so the backup walk tolerates paths that vanish
/// mid-walk: `Ok(Some(v))` = use it, `Ok(None)` = the path was removed
/// (`NotFound`) so skip it, `Err(e)` = a real error to propagate.
///
/// A workspace's background jobs constantly create and tear down git worktrees
/// under `.git/`. A path the walk enumerated can disappear before tar reads it.
/// A single vanished path must not abort the whole backup. Other error kinds,
/// permissions and I/O, still must.
fn skip_if_vanished<T>(path: &Path, result: std::io::Result<T>) -> Result<Option<T>, BoxError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            crate::log!("[Backup] Skipping vanished path {}: {}", path.display(), e);
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

/// Append a single file at `path` to the tar `builder` under archive path `archive_path`.
fn append_file<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    path: &Path,
    archive_path: &Path,
    metadata: &std::fs::Metadata,
) -> Result<(), BoxError> {
    let mut header = header_for_file(metadata);
    // The file may have vanished between the metadata check and now; skip it.
    let Some(mut file) = skip_if_vanished(path, std::fs::File::open(path))? else {
        return Ok(());
    };
    builder.append_data(&mut header, archive_path, &mut file)?;
    Ok(())
}

/// Tar workspace files (excluding .lucidos/) and SQL dump, then zstd compress.
///
/// Streams tar entries directly through zstd to a file — never holds the full
/// archive in memory.
///
/// The workspace's own `.git/` IS included (artifact version history); only
/// `.lucidos/`, `data/postgres*`, and `data/.backupignore` entries are excluded
/// (see [`is_excluded_workspace_path`]). The user-level `~/.lucidos/` is
/// deliberately NOT included. `restore_archive_into` unconditionally discards
/// any `user_dir/`, because it would clobber the target's machine-global gateway
/// registry, so backing it up is multi-GB of dead weight. The gateway's
/// `deleted/` stashes alone can be several GB.
fn tar_and_compress(
    workspace: &Path,
    sql_dump: &Path,
    output: &Path,
    ignore: &BackupIgnore,
) -> Result<(), BoxError> {
    let file = std::fs::File::create(output)?;
    let encoder = zstd::Encoder::new(file, 3)?;
    let mut builder = tar::Builder::new(encoder);

    // Add workspace files, excluding .lucidos/, data/postgres/, any
    // data/postgres.*/ siblings, and anything matched by data/.backupignore
    // (see is_excluded_workspace_path).
    for path in walkdir(workspace)? {
        let rel = path.strip_prefix(workspace)?;

        if is_excluded_workspace_path(rel, ignore) {
            continue;
        }

        let Some(metadata) = skip_if_vanished(&path, std::fs::metadata(&path))? else {
            continue;
        };
        if metadata.is_file() {
            append_file(&mut builder, &path, rel, &metadata)?;
        }
    }

    // Add the SQL dump at the archive root
    let sql_metadata = std::fs::metadata(sql_dump)?;
    append_file(
        &mut builder,
        sql_dump,
        Path::new("lucidos_backup.dump"),
        &sql_metadata,
    )?;

    let encoder = builder.into_inner()?;
    encoder.finish()?;

    Ok(())
}

/// Recursively walk a directory, returning all file paths.
fn walkdir(dir: &Path) -> Result<Vec<PathBuf>, BoxError> {
    let mut results = Vec::new();
    walkdir_inner(dir, &mut results)?;
    Ok(results)
}

fn walkdir_inner(dir: &Path, results: &mut Vec<PathBuf>) -> Result<(), BoxError> {
    if !dir.is_dir() {
        return Ok(());
    }
    // The directory may be removed mid-walk; treat a vanished dir as empty.
    let Some(entries) = skip_if_vanished(dir, std::fs::read_dir(dir))? else {
        return Ok(());
    };
    for entry in entries {
        let Some(entry) = skip_if_vanished(dir, entry)? else {
            continue;
        };
        let path = entry.path();
        if path.is_dir() {
            walkdir_inner(&path, results)?;
        } else {
            results.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
