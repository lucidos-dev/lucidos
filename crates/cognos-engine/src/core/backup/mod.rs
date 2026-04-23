pub mod crypto;
pub mod dropbox;
pub mod google_drive;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::core::oauth::{self, OAuthStore};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Registry entry: (id, name, oauth_provider, required_scope, constructor).
type BackupProviderEntry = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    fn(PgPool) -> Box<dyn BackupProvider>,
);

/// Preference key for the backup cron schedule expression.
pub const PREF_BACKUP_SCHEDULE: &str = "backup_schedule";
/// Preference key for the backup provider ID.
pub const PREF_BACKUP_PROVIDER: &str = "backup_provider";
/// Preference key for how many backups to keep (oldest are deleted after a new backup).
pub const PREF_BACKUP_RETENTION: &str = "backup_retention";
/// Default number of backups to keep when no preference is set.
pub const DEFAULT_BACKUP_RETENTION: usize = 5;

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
    ("google_drive", "Google Drive", "google", "drive", |pool| {
        Box::new(google_drive::GoogleDriveBackupProvider::new(pool))
    }),
    ("dropbox", "Dropbox", "dropbox", "", |pool| {
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
        .map(|(_, _, _, _, ctor)| ctor(pool.clone()))
        .ok_or_else(|| format!("Unknown backup provider: {}", provider_id))
}

/// Get an OAuth token for a backup provider, refreshing if needed.
pub async fn get_oauth_token(pool: &PgPool, provider: &str) -> Result<String, BoxError> {
    let mut account = OAuthStore::get_by_provider(pool, provider)
        .await?
        .ok_or_else(|| {
            format!(
                "No {} account connected. Connect it in Settings first.",
                provider
            )
        })?;
    oauth::refresh_oauth_if_needed(pool, &mut account).await?;
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

/// Trait for cloud storage backends that can store/retrieve encrypted backups.
#[async_trait]
pub trait BackupProvider: Send + Sync {
    fn name(&self) -> &str;
    fn id(&self) -> &str;
    /// The OAuth provider name used for token lookup (e.g. "google", "dropbox").
    fn oauth_provider(&self) -> &str;
    /// Verify the provider is accessible (valid token, correct permissions).
    /// Called before expensive work like pg_dump/compress/encrypt.
    async fn verify_access(&self) -> Result<(), BoxError>;
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
    workspace.join(".cognos").join("backup.key")
}

/// Result of a successful backup restore — the new workspace's location.
#[derive(Debug, Clone, Serialize)]
pub struct RestoredWorkspace {
    pub workspace_path: String,
    pub workspace_name: String,
}

/// Parse the workspace name from a backup filename.
///
/// Expected format: `cognos-backup-{name}-{YYYYMMDD}-{HHMMSS}.enc`
/// The name may contain hyphens, so we match the timestamp suffix pattern.
pub fn parse_workspace_name(filename: &str) -> Option<String> {
    let stem = filename.strip_suffix(".enc")?;
    let prefix = "cognos-backup-";
    let rest = stem.strip_prefix(prefix)?;
    // Find the timestamp suffix: -{YYYYMMDD}-{HHMMSS} (16 chars: -XXXXXXXX-XXXXXX)
    if rest.len() < 16 {
        return None;
    }
    let (name, suffix) = rest.split_at(rest.len() - 16);
    // Validate suffix is -{digits8}-{digits6}
    let parts: Vec<&str> = suffix.split('-').collect();
    if parts.len() != 3
        || !parts[0].is_empty()
        || parts[1].len() != 8
        || !parts[1].chars().all(|c| c.is_ascii_digit())
        || parts[2].len() != 6
        || !parts[2].chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
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
/// Pipeline: pg_dump -> tar (exclude .cognos/) -> zstd compress -> AES-256-GCM encrypt -> upload
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

    // Verify provider access before doing any expensive work
    provider.verify_access().await?;

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
            // Estimate phase weights from workspace size
            progress("estimating", 0, 100);
            let (dump_end, compress_end, encrypt_end) = estimate_weights(&workspace);

            // Phase 1: pg_dump (0% → dump_end%)
            crate::log!("[Backup] Phase 1/4: pg_dump");
            progress("dumping_db", 1, 100);
            let dump_path = temp_path.join("cognos_backup.dump");
            pg_dump(&database_url, &dump_path)?;

            // Phase 2: tar + zstd compress (dump_end% → compress_end%)
            crate::log!("[Backup] Phase 2/4: tar + zstd compress");
            progress("compressing", dump_end, 100);
            let compressed_path = temp_path.join("backup.tar.zst");
            let user_dir = std::env::var("HOME")
                .ok()
                .map(|h| std::path::PathBuf::from(h).join(".cognos"));
            tar_and_compress(
                &workspace,
                &dump_path,
                &compressed_path,
                user_dir.as_deref(),
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
    let workspace_name = workspace
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");
    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("cognos-backup-{workspace_name}-{timestamp}.enc");

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

/// Delete old backups beyond the retention limit, keeping the newest `keep` entries.
///
/// Lists all backups from the provider, sorts by creation time (newest first),
/// and deletes any beyond `keep`. Returns the number of backups deleted.
pub async fn prune_old_backups(
    provider: &dyn BackupProvider,
    keep: usize,
) -> Result<usize, BoxError> {
    if keep == 0 {
        return Ok(0);
    }
    let mut entries = provider.list_backups().await?;
    if entries.len() <= keep {
        return Ok(0);
    }
    // Sort newest first
    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let to_delete = &entries[keep..];
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

            // Extract user_dir/ entries to ~/.cognos/
            if let Ok(home) = std::env::var("HOME") {
                let user_dir_staging = staging_dir.join("user_dir");
                if user_dir_staging.exists() {
                    let user_dir = std::path::PathBuf::from(home).join(".cognos");
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
                let _ = std::fs::remove_dir_all(&workspace_path);
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
            let dump_path = ws_path.join("cognos_backup.dump");
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
    let scripts_dir = find_scripts_dir()?;
    let init_script = scripts_dir.join("init-workspace.sh");
    if !init_script.exists() {
        return Err(format!("init-workspace.sh not found at {}", init_script.display()).into());
    }

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

/// Find the scripts/ directory using CARGO_MANIFEST_DIR (baked at compile time).
pub fn find_scripts_dir() -> Result<PathBuf, BoxError> {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .ok_or("Cannot resolve repo root from CARGO_MANIFEST_DIR")?
        .to_path_buf();
    let scripts = repo_root.join("scripts");
    if scripts.join("init-workspace.sh").exists() {
        Ok(scripts)
    } else {
        Err(format!(
            "scripts/init-workspace.sh not found at {}",
            scripts.display()
        )
        .into())
    }
}

/// Estimate progress weights for each backup phase based on workspace size.
///
/// Returns `(dump_end, compress_end, encrypt_end)` as percentages (0-100).
/// Upload fills the remainder to 100%.
///
/// Rough speed estimates from real workspaces:
///   pg_dump  ~50 MB/s + 2s overhead
///   compress ~25 MB/s (tar + zstd level 3)
///   encrypt  ~5 MB/s  (AES-256-GCM, 1 MB chunks)
///   upload   ~10 MB/s (network dependent)
fn estimate_weights(workspace: &Path) -> (usize, usize, usize) {
    let cognos_dir = workspace.join(".cognos");
    let postgres_dir = workspace.join(crate::core::POSTGRES_DIR);
    let ws_bytes = walkdir(workspace)
        .unwrap_or_default()
        .iter()
        .filter(|p| !p.starts_with(&cognos_dir) && !p.starts_with(&postgres_dir))
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum::<u64>();

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

/// Run pg_dump to export the database in custom archive format.
///
/// Custom format (`-Fc`) is a compressed binary format restored via `pg_restore`.
/// It avoids PostgreSQL 18's `\restrict` / `\unrestrict` psql meta-commands
/// (CVE-2025-8714) that break plain-text restores, and supports parallel restore.
fn pg_dump(database_url: &str, output_path: &Path) -> Result<(), BoxError> {
    let output = std::process::Command::new("pg_dump")
        .arg(database_url)
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

/// Restore a database from a custom-format dump file using pg_restore.
fn pg_restore(database_url: &str, dump_path: &Path) -> Result<(), BoxError> {
    let output = std::process::Command::new("pg_restore")
        .arg("--dbname")
        .arg(database_url)
        .args([
            "--no-owner",
            "--no-acl",
            "--clean",
            "--if-exists",
            "--single-transaction",
        ])
        .arg(dump_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "pg_restore failed with exit code: {}\n{}",
            output.status, stderr
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
    let output = std::process::Command::new("psql")
        .arg(database_url)
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

/// Tar workspace files (excluding .cognos/) and SQL dump, then zstd compress.
///
/// Streams tar entries directly through zstd to a file — never holds the full
/// archive in memory.
fn tar_and_compress(
    workspace: &Path,
    sql_dump: &Path,
    output: &Path,
    user_dir: Option<&Path>,
) -> Result<(), BoxError> {
    let file = std::fs::File::create(output)?;
    let encoder = zstd::Encoder::new(file, 3)?;
    let mut builder = tar::Builder::new(encoder);

    // Add workspace files, excluding .cognos/ and data/postgres/ (live DB — backed up via pg_dump)
    let postgres_rel = std::path::Path::new(crate::core::POSTGRES_DIR);
    for path in walkdir(workspace)? {
        let rel = path.strip_prefix(workspace)?;

        if rel.starts_with(".cognos") || rel.starts_with(postgres_rel) {
            continue;
        }

        let metadata = std::fs::metadata(&path)?;
        if metadata.is_file() {
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
            let mut file = std::fs::File::open(&path)?;
            builder.append_data(&mut header, rel, &mut file)?;
        }
    }

    // Add the SQL dump at the archive root
    let sql_metadata = std::fs::metadata(sql_dump)?;
    let mut header = tar::Header::new_gnu();
    header.set_size(sql_metadata.len());
    header.set_mode(0o644);
    header.set_mtime(
        sql_metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0),
    );
    header.set_cksum();
    let mut sql_file = std::fs::File::open(sql_dump)?;
    builder.append_data(&mut header, "cognos_backup.dump", &mut sql_file)?;

    // Add user-level shared data (~/.cognos/) under "user_dir/" prefix in the archive
    if let Some(user_dir) = user_dir {
        if user_dir.exists() {
            for path in walkdir(user_dir)? {
                let rel = path.strip_prefix(user_dir)?;
                // Skip .git/ in user dir (not useful in backup, takes space)
                if rel.starts_with(".git") {
                    continue;
                }
                let metadata = std::fs::metadata(&path)?;
                if metadata.is_file() {
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
                    let archive_path = std::path::Path::new("user_dir").join(rel);
                    let mut file = std::fs::File::open(&path)?;
                    builder.append_data(&mut header, archive_path, &mut file)?;
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
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
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
mod tests {
    use super::*;

    #[test]
    fn test_parse_workspace_name_from_filename() {
        assert_eq!(
            parse_workspace_name("cognos-backup-personal-20260415-191605.enc"),
            Some("personal".to_string())
        );
        assert_eq!(
            parse_workspace_name("cognos-backup-my-workspace-20260415-191605.enc"),
            Some("my-workspace".to_string())
        );
        assert_eq!(
            parse_workspace_name("cognos-backup-dev-20260101-000000.enc"),
            Some("dev".to_string())
        );
        // Unexpected format
        assert_eq!(parse_workspace_name("random-file.enc"), None);
        assert_eq!(parse_workspace_name("cognos-backup-.enc"), None);
        assert_eq!(parse_workspace_name("not-a-backup.txt"), None);
    }

    #[test]
    fn test_validate_workspace_name() {
        assert!(validate_workspace_name("personal").is_ok());
        assert!(validate_workspace_name("my-workspace").is_ok());
        assert!(validate_workspace_name("test_123").is_ok());

        // Invalid
        assert!(validate_workspace_name("").is_err());
        assert!(validate_workspace_name("..").is_err());
        assert!(validate_workspace_name("/etc/passwd").is_err());
        assert!(validate_workspace_name("\\windows").is_err());
        assert!(validate_workspace_name("has space").is_err());
        assert!(validate_workspace_name(".hidden").is_err());
    }

    #[test]
    fn test_key_file_path() {
        let workspace = Path::new("/home/user/my-workspace");
        assert_eq!(
            key_file_path(workspace),
            PathBuf::from("/home/user/my-workspace/.cognos/backup.key")
        );
    }

    #[test]
    fn test_tar_and_compress_excludes_cognos_and_postgres() {
        let workspace = tempfile::tempdir().unwrap();
        let ws = workspace.path();

        // Create .cognos/ — should be excluded
        std::fs::create_dir_all(ws.join(".cognos/cache")).unwrap();
        std::fs::write(ws.join(".cognos/cache/index.dat"), "cache data").unwrap();

        // Create data/postgres/ — should be excluded (live DB, backed up via pg_dump)
        std::fs::create_dir_all(ws.join("data/postgres/global")).unwrap();
        std::fs::write(ws.join("data/postgres/global/pg_filenode.map"), "pgdata").unwrap();

        // Create .git/ — should be included
        std::fs::create_dir_all(ws.join(".git")).unwrap();
        std::fs::write(ws.join(".git/HEAD"), "ref: refs/heads/main").unwrap();

        // Create data/artifacts/ — should be included
        std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
        std::fs::write(ws.join("data/artifacts/user_profile.md"), "# Profile").unwrap();

        // Create a SQL dump
        let sql_dump = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(sql_dump.path(), "CREATE TABLE test;").unwrap();

        // Tar and compress
        let output = tempfile::NamedTempFile::new().unwrap();
        tar_and_compress(ws, sql_dump.path(), output.path(), None).unwrap();

        // Decompress and verify contents (streaming from file)
        let file = std::fs::File::open(output.path()).unwrap();
        let decoder = zstd::Decoder::new(file).unwrap();
        let mut archive = tar::Archive::new(decoder);

        let mut found_paths: Vec<String> = Vec::new();
        for entry in archive.entries().unwrap() {
            let entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().to_string();
            found_paths.push(path);
        }

        found_paths.sort();

        // .cognos/ should be excluded
        assert!(
            !found_paths.iter().any(|p| p.starts_with(".cognos")),
            "archive should not contain .cognos/: {found_paths:?}"
        );

        // data/postgres/ should be excluded (live DB)
        assert!(
            !found_paths.iter().any(|p| p.starts_with("data/postgres")),
            "archive should not contain data/postgres/: {found_paths:?}"
        );

        // .git/ and data/artifacts/ should be included
        assert!(
            found_paths.iter().any(|p| p.starts_with(".git")),
            "archive should contain .git/: {found_paths:?}"
        );
        assert!(
            found_paths.iter().any(|p| p.starts_with("data/")),
            "archive should contain data/: {found_paths:?}"
        );

        // SQL dump should be at root
        assert!(
            found_paths.contains(&"cognos_backup.dump".to_string()),
            "archive should contain cognos_backup.dump: {found_paths:?}"
        );
    }

    #[test]
    fn test_full_pipeline_encrypt_decrypt() {
        let workspace = tempfile::tempdir().unwrap();
        let ws = workspace.path();

        // Create a file in the workspace
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::write(ws.join("data/notes.txt"), "important notes").unwrap();

        // Also create .cognos/ which should be excluded
        std::fs::create_dir_all(ws.join(".cognos")).unwrap();
        std::fs::write(ws.join(".cognos/runtime.pid"), "12345").unwrap();

        // Create a mock SQL dump
        let sql_dump = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(sql_dump.path(), "INSERT INTO events VALUES (1);").unwrap();

        // Tar + compress (streaming to file)
        let compressed_path = tempfile::NamedTempFile::new().unwrap();
        tar_and_compress(ws, sql_dump.path(), compressed_path.path(), None).unwrap();

        // Encrypt (streaming file-to-file)
        let key = crypto::generate_key();
        let encrypted_path = tempfile::NamedTempFile::new().unwrap();
        {
            let input = std::fs::File::open(compressed_path.path()).unwrap();
            let mut output = std::fs::File::create(encrypted_path.path()).unwrap();
            crypto::encrypt(&key, input, &mut output).unwrap();
        }

        // Decrypt (streaming file-to-file)
        let decrypted_path = tempfile::NamedTempFile::new().unwrap();
        {
            let input = std::fs::File::open(encrypted_path.path()).unwrap();
            let mut output = std::fs::File::create(decrypted_path.path()).unwrap();
            crypto::decrypt(&key, input, &mut output).unwrap();
        }

        // Decompress + untar (streaming from file)
        let restore_dir = tempfile::tempdir().unwrap();
        {
            let file = std::fs::File::open(decrypted_path.path()).unwrap();
            let decoder = zstd::Decoder::new(file).unwrap();
            let mut archive = tar::Archive::new(decoder);
            archive.unpack(restore_dir.path()).unwrap();
        }

        // Verify restored content matches original
        let restored_notes =
            std::fs::read_to_string(restore_dir.path().join("data/notes.txt")).unwrap();
        assert_eq!(restored_notes, "important notes");

        // Verify SQL dump is present
        let restored_sql =
            std::fs::read_to_string(restore_dir.path().join("cognos_backup.dump")).unwrap();
        assert_eq!(restored_sql, "INSERT INTO events VALUES (1);");

        // Verify .cognos/ was NOT included
        assert!(
            !restore_dir.path().join(".cognos").exists(),
            ".cognos/ should not be in the restored backup"
        );
    }

    #[test]
    fn test_estimate_weights_small_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        // 1 MB workspace — dump dominates (fixed 2s overhead)
        std::fs::write(ws.join("data.bin"), vec![0u8; 1_048_576]).unwrap();

        let (dump_end, compress_end, encrypt_end) = estimate_weights(ws);

        assert!(dump_end >= 2, "dump should get at least 2%: {dump_end}");
        assert!(
            compress_end > dump_end,
            "compress_end should exceed dump_end"
        );
        assert!(
            encrypt_end > compress_end,
            "encrypt_end should exceed compress_end"
        );
        assert!(
            encrypt_end <= 97,
            "encrypt_end should leave room for upload: {encrypt_end}"
        );
    }

    #[test]
    fn test_estimate_weights_empty_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let (dump_end, compress_end, encrypt_end) = estimate_weights(dir.path());

        // Each phase gets at least its minimum, and they're strictly increasing
        assert!(dump_end >= 2);
        assert!(compress_end > dump_end);
        assert!(encrypt_end > compress_end);
        assert!(encrypt_end <= 97);
    }

    #[test]
    fn test_estimate_restore_weights() {
        // 10 MB encrypted file
        let (decrypt_end, decompress_end) = estimate_restore_weights(10 * 1_048_576, 15);

        assert!(decrypt_end > 15, "decrypt should start after download");
        assert!(
            decompress_end > decrypt_end,
            "decompress should follow decrypt"
        );
        assert!(
            decompress_end <= 97,
            "should leave room for restore: {decompress_end}"
        );
    }

    #[test]
    fn test_estimate_restore_weights_tiny_file() {
        // 100 KB file — restore dominates
        let (decrypt_end, decompress_end) = estimate_restore_weights(100_000, 15);

        assert!(decrypt_end >= 17, "each phase gets at least 2%");
        assert!(decompress_end >= decrypt_end + 2);
    }

    #[test]
    fn test_move_contents() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();

        // Create structure in src
        std::fs::create_dir_all(src.path().join("data/artifacts")).unwrap();
        std::fs::write(src.path().join("data/artifacts/profile.md"), "# Profile").unwrap();
        std::fs::write(src.path().join("top.txt"), "hello").unwrap();

        // Put existing content in dest that shouldn't be affected
        std::fs::create_dir_all(dest.path().join(".cognos")).unwrap();
        std::fs::write(dest.path().join(".cognos/key"), "secret").unwrap();

        move_contents(src.path(), dest.path()).unwrap();

        // Source content should now be in dest
        assert_eq!(
            std::fs::read_to_string(dest.path().join("data/artifacts/profile.md")).unwrap(),
            "# Profile"
        );
        assert_eq!(
            std::fs::read_to_string(dest.path().join("top.txt")).unwrap(),
            "hello"
        );
        // Pre-existing dest content should still be there
        assert_eq!(
            std::fs::read_to_string(dest.path().join(".cognos/key")).unwrap(),
            "secret"
        );
    }

    #[test]
    fn test_tar_includes_user_dir() {
        let workspace = tempfile::tempdir().unwrap();
        let ws = workspace.path();
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::write(ws.join("data/notes.txt"), "workspace content").unwrap();

        // Create a mock user dir
        let user_tmp = tempfile::tempdir().unwrap();
        let user_dir = user_tmp.path();
        std::fs::create_dir_all(user_dir.join("knowhow")).unwrap();
        std::fs::write(
            user_dir.join("knowhow/cognos.md"),
            "---\nname: CognOS\n---\nCognOS knowhow.",
        )
        .unwrap();
        // Create .git/ which should be excluded
        std::fs::create_dir_all(user_dir.join(".git/objects")).unwrap();
        std::fs::write(user_dir.join(".git/HEAD"), "ref: refs/heads/main").unwrap();

        let sql_dump = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(sql_dump.path(), "CREATE TABLE test;").unwrap();

        let output = tempfile::NamedTempFile::new().unwrap();
        tar_and_compress(ws, sql_dump.path(), output.path(), Some(user_dir)).unwrap();

        // Verify archive contents
        let file = std::fs::File::open(output.path()).unwrap();
        let decoder = zstd::Decoder::new(file).unwrap();
        let mut archive = tar::Archive::new(decoder);
        let mut found_paths: Vec<String> = Vec::new();
        for entry in archive.entries().unwrap() {
            let entry = entry.unwrap();
            found_paths.push(entry.path().unwrap().to_string_lossy().to_string());
        }

        // Should include user_dir/knowhow/cognos.md
        assert!(
            found_paths
                .iter()
                .any(|p| p == "user_dir/knowhow/cognos.md"),
            "archive should contain user_dir/knowhow/cognos.md: {found_paths:?}"
        );
        // Should NOT include user_dir/.git/
        assert!(
            !found_paths.iter().any(|p| p.starts_with("user_dir/.git")),
            "archive should not contain user_dir/.git/: {found_paths:?}"
        );
        // Should still include workspace content
        assert!(
            found_paths.iter().any(|p| p == "data/notes.txt"),
            "archive should contain data/notes.txt: {found_paths:?}"
        );
    }

    #[test]
    fn test_walkdir_recursive() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("top.txt"), "top").unwrap();
        std::fs::write(root.join("a/mid.txt"), "mid").unwrap();
        std::fs::write(root.join("a/b/deep.txt"), "deep").unwrap();

        let files = walkdir(root).unwrap();
        assert_eq!(files.len(), 3);

        let rel_paths: Vec<String> = files
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().to_string())
            .collect();

        assert!(rel_paths.contains(&"top.txt".to_string()));
        assert!(rel_paths.contains(&"a/mid.txt".to_string()));
        assert!(rel_paths.contains(&"a/b/deep.txt".to_string()));
    }
}
