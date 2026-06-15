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

/// Registry entry: (id, name, oauth_provider, required_scope, constructor).
/// The constructor receives the pool AND the entry's `required_scope`, so a
/// provider carries the scope it must verify in preflight without a second
/// source of truth.
type BackupProviderEntry = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    fn(PgPool, &'static str) -> Box<dyn BackupProvider>,
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

/// Read the backup retention count from preferences, falling back to the default.
pub async fn get_retention_count(pool: &PgPool) -> usize {
    use crate::core::PreferenceStore;
    PreferenceStore::get(pool, PREF_BACKUP_RETENTION)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_BACKUP_RETENTION)
}

/// Static registry of all backup providers: (id, name, oauth_provider, required_scope, constructor).
const PROVIDERS: &[BackupProviderEntry] = &[
    (
        "google_drive",
        "Google Drive",
        "google",
        "drive",
        |pool, scope| Box::new(google_drive::GoogleDriveBackupProvider::new(pool, scope)),
    ),
    ("dropbox", "Dropbox", "dropbox", "", |pool, _scope| {
        Box::new(dropbox::DropboxBackupProvider::new(pool))
    }),
];

/// Provider metadata including the OAuth provider name for readiness checks.
pub struct ProviderMeta {
    pub id: &'static str,
    pub name: &'static str,
    pub oauth_provider: &'static str,
    /// Substring that must appear in the OAuth account's scopes for the provider to be ready.
    /// Empty string means no specific scope is required beyond being connected.
    pub required_scope: &'static str,
}

/// List all registered backup providers with their metadata.
pub fn list_providers() -> Vec<ProviderMeta> {
    PROVIDERS
        .iter()
        .map(|(id, name, oauth, scope, _)| ProviderMeta {
            id,
            name,
            oauth_provider: oauth,
            required_scope: scope,
        })
        .collect()
}

/// Create a backup provider by ID.
pub fn get_provider(provider_id: &str, pool: &PgPool) -> Result<Box<dyn BackupProvider>, String> {
    PROVIDERS
        .iter()
        .find(|(id, _, _, _, _)| *id == provider_id)
        .map(|(_, _, _, scope, ctor)| ctor(pool.clone(), scope))
        .ok_or_else(|| format!("Unknown backup provider: {}", provider_id))
}

/// Get an OAuth token for a backup provider, refreshing if needed.
pub async fn get_oauth_token(pool: &PgPool, provider: &str) -> Result<String, BoxError> {
    use crate::core::oauth::AccountLookupError;
    let account = oauth::get_account_with_fresh_token(pool, provider)
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
        })?;
    Ok(account.access_token)
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
/// by `run_backup` on both the success and failure paths and stored under
/// `PREF_BACKUP_LAST_RUN`, so the page survives engine restarts (the
/// `BackupCompleted` / `BackupFailed` SSE events are ephemeral).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupLastRun {
    pub status: BackupRunStatus,
    /// When the run reached its terminal state (RFC 3339).
    pub at: DateTime<Utc>,
    /// Filename of the produced backup (success only).
    pub filename: Option<String>,
    /// Size of the produced backup in bytes (success only).
    pub size_bytes: Option<u64>,
    /// Error message (failure only).
    pub error: Option<String>,
}

impl BackupLastRun {
    /// Build a success record from the produced backup entry.
    pub fn success(entry: &BackupEntry) -> Self {
        Self {
            status: BackupRunStatus::Success,
            at: Utc::now(),
            filename: Some(entry.filename.clone()),
            size_bytes: Some(entry.size_bytes),
            error: None,
        }
    }

    /// Build a failure record from the error message.
    pub fn failure(error: &str) -> Self {
        Self {
            status: BackupRunStatus::Failure,
            at: Utc::now(),
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
    PreferenceStore::set(pool, PREF_BACKUP_LAST_RUN, &value).await?;
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
    /// orchestrator's estimate of the encrypted archive size; the provider uses
    /// it to reject the run up front when the upload can't succeed (e.g. over
    /// quota), instead of wasting the whole pipeline only to fail at the final
    /// upload. Also verifies the token is valid and the required scope exists.
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

/// Result of a successful backup restore — the new workspace's location.
#[derive(Debug, Clone, Serialize)]
pub struct RestoredWorkspace {
    pub workspace_path: String,
    pub workspace_name: String,
}

/// Authoritative restore-progress state, held by the engine for the duration of
/// a restore and beyond (the terminal `Completed`/`Failed` is kept until the
/// next restore starts). This is the SINGLE source of truth that BOTH the SSE
/// `Restore*` events and the `GET /api/v1/backup/restore-status` endpoint
/// serialize from — so a live stream and a page-reload refetch always render
/// the identical state. A restore can run for many minutes; binding its state
/// to the HTTP request lifetime (the old design) meant a tab reload or network
/// blip cancelled the handler future, dropped the staging `TempDir`, and lost
/// the whole download with nothing to reconnect to. See `restore_backup` in
/// `api/backup.rs`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RestoreState {
    /// No restore has run since startup (or the field was never touched).
    #[default]
    Idle,
    /// A restore is in flight. `phase`/`progress`/`total` mirror the
    /// `restore_progress_sender` ticks (phase strings: downloading, decrypting,
    /// decompressing, initializing, starting_db, restoring_db, done).
    Running {
        workspace_name: String,
        phase: String,
        progress: usize,
        total: usize,
    },
    /// The restore finished; the new workspace is ready to start.
    Completed {
        workspace_name: String,
        workspace_path: String,
    },
    /// The restore failed; `error` is the user-facing message.
    Failed {
        workspace_name: String,
        error: String,
    },
}

impl RestoreState {
    /// True while a restore is actively running — used to reject a concurrent
    /// restore (the engine runs one at a time).
    pub fn is_running(&self) -> bool {
        matches!(self, RestoreState::Running { .. })
    }

    /// Atomically claim the restore slot: if no restore is running, transition to
    /// `Running { phase: "starting" }` and return `true`; if one is already
    /// running, leave the state untouched and return `false`. The caller holds
    /// the engine's `restore_state` write lock across this call, which makes the
    /// check-and-set indivisible — two concurrent requests can't both win.
    pub fn try_start(&mut self, workspace_name: &str) -> bool {
        if self.is_running() {
            return false;
        }
        *self = RestoreState::Running {
            workspace_name: workspace_name.to_string(),
            phase: "starting".to_string(),
            progress: 0,
            total: 100,
        };
        true
    }
}

/// Validate a workspace name for use as a directory name.
///
/// Rules: non-empty, no path traversal, filesystem-safe characters only
/// (alphanumeric, hyphens, underscores), no leading dot.
pub fn validate_workspace_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Workspace name cannot be empty".into());
    }
    if name.starts_with('.') {
        return Err("Workspace name cannot start with a dot".into());
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err("Workspace name contains invalid characters".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(
            "Workspace name may only contain letters, digits, hyphens, and underscores".into(),
        );
    }
    Ok(())
}

/// Resolve the target workspace path from a name.
/// Returns the full path under ~/workspaces/{name}.
pub fn resolve_restore_workspace_path(name: &str) -> Result<PathBuf, BoxError> {
    validate_workspace_name(name).map_err(|e| -> BoxError { e.into() })?;
    let home = std::env::var("HOME").map_err(|_| "HOME not set")?;
    let path = PathBuf::from(home).join("workspaces").join(name);
    if path.exists() {
        return Err(format!("Workspace '{}' already exists at {}", name, path.display()).into());
    }
    Ok(path)
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

    // Estimate the upload size up front — one workspace walk on the blocking
    // pool — so the provider's preflight can fail fast BEFORE pg_dump, the
    // multi-GB tar, and the multi-minute encrypt if it can't accept the upload
    // (over quota, missing scope, bad token). This walk replaces the one
    // estimate_weights used to do; the tar phase still walks once to stream files.
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
            let user_dir = std::env::var("HOME")
                .ok()
                .map(|h| std::path::PathBuf::from(h).join(".lucidos"));
            tar_and_compress(
                &workspace,
                &dump_path,
                &compressed_path,
                user_dir.as_deref(),
                &ignore,
            )?;

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
/// `personal` vs `personal-2`, where `lucidos-backup-personal-2-…` belongs to
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
/// workspace — it must match `lucidos-backup-{workspace_name}-{timestamp}.enc`
/// exactly (timestamp shape validated). This is the guard that keeps pruning
/// from ever touching another workspace's archives or any unrelated file the
/// user placed in the shared cloud backup folder.
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
/// workspace's `lucidos-backup-{workspace_name}-*.enc` archives (so another
/// workspace's backups and any unrelated file in the folder are never touched —
/// see [`select_prunable`]), then deletes oldest-first. Returns the number of
/// backups deleted. A single delete failure is logged and skipped rather than
/// propagated — one stuck archive must not abort cleanup of the rest.
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

/// Restore a backup into a brand-new workspace.
///
/// Pipeline: download → decrypt → decompress → create workspace → init postgres → pg_restore
///
/// The calling workspace is never touched. A new workspace directory is created at
/// `~/workspaces/{workspace_name}`, provisioned via `init-workspace.sh`, and the
/// backup's database is restored into the new workspace's postgres.
pub async fn restore_backup(
    workspace_name: &str,
    key: &[u8],
    backup_id: &str,
    provider: &dyn BackupProvider,
    progress: impl Fn(&str, usize, usize) + Send + Sync + 'static,
) -> Result<RestoredWorkspace, BoxError> {
    let workspace_path = resolve_restore_workspace_path(workspace_name)?;
    let ws_name = workspace_name.to_string();

    let temp_dir = tempfile::tempdir()?;
    let progress = Arc::new(progress);

    // Phase 1: download with progress (0% → download_end%)
    let download_end = 15usize;
    progress("downloading", 0, 100);
    let encrypted_path = temp_dir.path().join("backup.enc");
    let download_progress = |done: u64, total: u64| {
        let pct = if total > 0 {
            (download_end as f64 * done as f64 / total as f64) as usize
        } else {
            0
        };
        progress("downloading", pct, 100);
    };
    provider
        .download(backup_id, &encrypted_path, &download_progress)
        .await?;

    // Estimate remaining phase weights from the downloaded file size
    let enc_bytes = std::fs::metadata(&encrypted_path)?.len();
    let (decrypt_end, decompress_end) = estimate_restore_weights(enc_bytes, download_end);

    // Phases 2-3: decrypt + decompress into staging (blocking)
    let staging_dir = {
        let progress = progress.clone();
        let encrypted_path = encrypted_path.clone();
        let key = key.to_vec();
        let temp_path = temp_dir.path().to_path_buf();

        tokio::task::spawn_blocking(move || -> Result<PathBuf, BoxError> {
            // Phase 2: decrypt
            progress("decrypting", download_end, 100);
            let compressed_path = temp_path.join("backup.tar.zst");
            {
                let total_bytes = std::fs::metadata(&encrypted_path)?.len();
                let decrypt_range = decrypt_end - download_end;
                let input = std::fs::File::open(&encrypted_path)?;
                let mut last_pct = download_end;
                let reader = ProgressReader {
                    inner: input,
                    bytes_read: 0,
                    callback: |done| {
                        let pct = download_end
                            + (decrypt_range as f64 * done as f64 / total_bytes.max(1) as f64)
                                as usize;
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

            // Phase 3: decompress + untar into staging
            progress("decompressing", decrypt_end, 100);
            let staging_dir = temp_path.join("staging");
            std::fs::create_dir_all(&staging_dir)?;
            {
                let file = std::fs::File::open(&compressed_path)?;
                let decoder = zstd::Decoder::new(file)?;
                let mut archive = tar::Archive::new(decoder);
                archive.unpack(&staging_dir)?;
            }

            // Extract user_dir/ entries to ~/.lucidos/
            if let Ok(home) = std::env::var("HOME") {
                let user_dir_staging = staging_dir.join("user_dir");
                if user_dir_staging.exists() {
                    let user_dir = std::path::PathBuf::from(home).join(".lucidos");
                    std::fs::create_dir_all(&user_dir)?;
                    move_contents(&user_dir_staging, &user_dir)?;
                    let _ = std::fs::remove_dir_all(&user_dir_staging);
                }
            }

            Ok(staging_dir)
        })
        .await??
    };

    // Phase 4: create workspace directory and move files in
    progress("initializing", decompress_end, 100);
    {
        let workspace_path = workspace_path.clone();
        tokio::task::spawn_blocking(move || -> Result<(), BoxError> {
            std::fs::create_dir_all(&workspace_path)?;
            if let Err(e) = move_contents(&staging_dir, &workspace_path) {
                // `workspace_path` was just created in this function and
                // `resolve_restore_workspace_path` guaranteed it did not exist
                // beforehand, so this only ever removes a freshly-created,
                // half-populated directory — never pre-existing user data. The
                // cleanup is what lets the user retry the restore without hitting
                // "workspace already exists". Log the failure (don't swallow it)
                // so a botched cleanup that would block the retry is visible.
                if let Err(rm) = std::fs::remove_dir_all(&workspace_path) {
                    crate::log!(
                        "[Backup] Restore failed and cleanup of partial workspace {} also failed: {} (original error: {})",
                        workspace_path.display(),
                        rm,
                        e
                    );
                }
                return Err(e);
            }
            Ok(())
        })
        .await??;
    }

    // Phase 5: provision postgres via init-workspace.sh
    progress("starting_db", decompress_end + 1, 100);
    let database_url = {
        let ws_name = ws_name.clone();
        tokio::task::spawn_blocking(move || init_workspace(&ws_name)).await??
    };

    // Phase 6: restore database
    progress("restoring_db", decompress_end + 2, 100);
    {
        let ws_path = workspace_path.clone();
        tokio::task::spawn_blocking(move || -> Result<(), BoxError> {
            terminate_other_connections(&database_url)?;
            let dump_path = ws_path.join("lucidos_backup.dump");
            if dump_path.exists() {
                pg_restore(&database_url, &dump_path)?;
                let _ = std::fs::remove_file(&dump_path);
            }
            Ok(())
        })
        .await??;
    }

    progress("done", 100, 100);

    Ok(RestoredWorkspace {
        workspace_path: workspace_path.to_string_lossy().to_string(),
        workspace_name: ws_name,
    })
}

/// Provision a new workspace by calling init-workspace.sh.
///
/// Returns the DATABASE_URL for the new workspace's postgres.
fn init_workspace(workspace_name: &str) -> Result<String, BoxError> {
    let init_script = crate::paths::script("init-workspace.sh")?;

    let output = std::process::Command::new("bash")
        .arg(&init_script)
        .arg("-w")
        .arg(workspace_name)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("init-workspace.sh failed: {}", stderr).into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(url) = line.strip_prefix("DATABASE_URL=") {
            return Ok(url.to_string());
        }
    }
    Err("init-workspace.sh did not output DATABASE_URL".into())
}

/// Sum the on-disk size of every file that WILL be included in the backup tar
/// (i.e. after the hardcoded + `.backupignore` exclusions). This is the one
/// workspace walk shared by preflight's free-space estimate and the phase-weight
/// estimate; the tar phase walks once more to actually stream the files.
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
///   pg_dump  ~50 MB/s + 2s overhead
///   compress ~25 MB/s (tar + zstd level 3)
///   encrypt  ~5 MB/s  (AES-256-GCM, 1 MB chunks)
///   upload   ~10 MB/s (network dependent)
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
/// doesn't match the expected shape so the caller fails loudly instead of
/// spawning a subprocess that inherits zero `PG*` vars and gets a confusing
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

/// Run pg_dump to export the database in custom archive format.
///
/// Custom format (`-Fc`) is a compressed binary format restored via `pg_restore`.
/// It avoids PostgreSQL 18's `\restrict` / `\unrestrict` psql meta-commands
/// (CVE-2025-8714) that break plain-text restores, and supports parallel restore.
///
/// Connection details flow through libpq `PG*` env vars so the password
/// stays out of argv.
fn pg_dump(database_url: &str, output_path: &Path) -> Result<(), BoxError> {
    let mut cmd = std::process::Command::new("pg_dump");
    apply_pg_env(&mut cmd, database_url)?;
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
/// We pull the name from the same `pg_env_vars` parse that seeds the `PG*` env,
/// so host/port/user/password still flow via env (kept out of argv) and only
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
const CROSS_VERSION_SET_PARAMS: &[&str] = &[
    "transaction_timeout",
];

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
    let mut restore_cmd = std::process::Command::new("pg_restore");
    apply_pg_env(&mut restore_cmd, database_url)?;
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
    let mut psql_cmd = std::process::Command::new("psql");
    apply_pg_env(&mut psql_cmd, database_url)?;
    let mut psql = psql_cmd
        .args(["--single-transaction", "--dbname", &dbname])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let write_result = psql.stdin.take().unwrap().write_all(&filtered);
    let psql_output = psql.wait_with_output()?;
    write_result?;

    if !psql_output.status.success() {
        let stderr = String::from_utf8_lossy(&psql_output.stderr);
        return Err(format!(
            "Restore failed: psql exited with code: {}\n{}",
            psql_output.status, stderr
        )
        .into());
    }
    Ok(())
}

/// Terminate all other database connections to allow pg_restore --clean to drop objects.
///
/// The engine's connection pool holds connections that may have cached prepared
/// statements. pg_restore --clean needs to DROP and recreate tables, which requires
/// no other sessions holding locks. After termination, the pool auto-reconnects.
fn terminate_other_connections(database_url: &str) -> Result<(), BoxError> {
    let mut cmd = std::process::Command::new("psql");
    apply_pg_env(&mut cmd, database_url)?;
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
    /// names a directory also excludes that directory's entire subtree. Note:
    /// with `glob`'s default options `*` matches across `/`, so wildcard
    /// patterns are greedy — `data/artifacts/*/klines` excludes `klines` at any
    /// depth under `data/artifacts`, not just one level down.
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
/// the hardcoded exclusions. Loaded and parsed once per backup run and shared
/// across the size estimate and the tar walk; an absent or empty file yields an
/// empty set, preserving today's behavior.
#[derive(Default)]
struct BackupIgnore(Vec<IgnorePattern>);

impl BackupIgnore {
    /// Load `<workspace>/data/.backupignore`. A missing file is the common case
    /// and yields no patterns silently; any other read error is logged and also
    /// treated as no patterns — a broken ignore file never aborts the backup.
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
/// The workspace's autoresearch loop constantly creates and tears down git
/// worktrees under `.git/`, so a file (or directory) enumerated by the walk can
/// disappear before tar reads it. A single vanished path must not abort the
/// whole backup — but other error kinds (permissions, I/O) still must.
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
fn tar_and_compress(
    workspace: &Path,
    sql_dump: &Path,
    output: &Path,
    user_dir: Option<&Path>,
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

    // Add user-level shared data (~/.lucidos/) under "user_dir/" prefix in the archive
    if let Some(user_dir) = user_dir {
        if user_dir.exists() {
            for path in walkdir(user_dir)? {
                let rel = path.strip_prefix(user_dir)?;
                // Skip .git/ in user dir (not useful in backup, takes space)
                if rel.starts_with(".git") {
                    continue;
                }
                let Some(metadata) = skip_if_vanished(&path, std::fs::metadata(&path))? else {
                    continue;
                };
                if metadata.is_file() {
                    let archive_path = std::path::Path::new("user_dir").join(rel);
                    append_file(&mut builder, &path, &archive_path, &metadata)?;
                }
            }
        }
    }

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
