use super::{BackupEntry, BackupProvider, BoxError};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde::Deserialize;
use sqlx::PgPool;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio::sync::Mutex;

const BACKUP_FOLDER: &str = "/Lucidos Backups";

const UPLOAD_URL: &str = "https://content.dropboxapi.com/2/files/upload";
const SESSION_START_URL: &str = "https://content.dropboxapi.com/2/files/upload_session/start";
const SESSION_APPEND_URL: &str = "https://content.dropboxapi.com/2/files/upload_session/append_v2";
const SESSION_FINISH_URL: &str = "https://content.dropboxapi.com/2/files/upload_session/finish";
const SPACE_USAGE_URL: &str = "https://api.dropboxapi.com/2/users/get_space_usage";
const GET_METADATA_URL: &str = "https://api.dropboxapi.com/2/files/get_metadata";

/// INVARIANT: Dropbox's own cap on a single `POST /2/files/upload`. Over this the server
/// closes the connection instead of answering with a JSON error, so the caller
/// sees a bare reqwest transport error. The failure lands at `.send()`, before
/// [`check_dropbox_status`] ever sees a response. Anything above this goes
/// through the upload session API instead.
const SINGLE_SHOT_MAX: u64 = 150 * 1024 * 1024;

const UPLOAD_CHUNK_SIZE: u64 = 8 * 1024 * 1024;

const MAX_RETRIES_PER_CHUNK: u32 = 6;

const MAX_RETRY_AFTER_SECS: u64 = 300;

const CHUNK_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// INVARIANT: Per-request timeout for the two calls that commit a whole archive: the
/// single-shot POST, and `upload_session/finish`. Far more generous than the
/// chunk timeout, because both are all-or-nothing. A chunk that times out costs
/// 8 MiB and resumes, where a commit that times out throws away the entire
/// transfer. Dropbox also assembles a multi-GB session server-side before it
/// answers. The single-shot body is capped at [`SINGLE_SHOT_MAX`], so ten
/// minutes still leaves it a floor of ~250 KB/s.
const COMMIT_REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

/// INVARIANT: How long to wait for a TCP/TLS connection before giving up. Safe as a
/// client-wide default in a way a whole-request timeout is not: `download`
/// streams a multi-GB archive over one response, so a request-wide default
/// would kill every large restore.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

const BYTES_PER_GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// INVARIANT: Every Dropbox scope a working backup needs, and why:
///
/// - `files.content.write`: `create_folder_v2`, `files/upload`, `files/delete_v2`
/// - `files.content.read`: `files/download`, i.e. restoring an archive
/// - `files.metadata.read`: `files/list_folder`, which drives retention pruning
///   and the "last cloud backup" health card
///
/// This IS the provider registry's `required_scopes` entry, so the readiness
/// verdict the Settings page renders and the preflight below check one list.
/// Two lists would let a partly-scoped account read as ready, hiding *Grant
/// access* behind a *Back up now* that fails at preflight.
///
/// `account_info.read` is deliberately NOT here. It only names the connected
/// account in Settings, so its absence costs an email address, never a backup.
/// Requiring it would make an otherwise working account read as broken. The
/// request set in `backupProviderScopes.ts` asks for it anyway.
pub const BACKUP_SCOPES: &[&str] = &[
    "files.content.write",
    "files.content.read",
    "files.metadata.read",
];

pub const GRANT_SCOPES: &[&str] = &[
    "files.content.write",
    "files.content.read",
    "files.metadata.read",
    "account_info.read",
];

pub fn missing_scope_from_body(body: &str) -> Option<String> {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(scope) = json["error"]["required_scope"].as_str() {
            return Some(scope.to_string());
        }
    }
    let after = body.split_once("required scope")?.1;
    let inner = after.split_once('\'')?.1;
    let (scope, _) = inner.split_once('\'')?;
    (!scope.is_empty()).then(|| scope.to_string())
}

pub fn missing_scope_message(missing: &[&str]) -> String {
    format!(
        "Dropbox is missing the permission{} {}. Enable {} on the Permissions tab of your app in the Dropbox App Console, then reconnect the account in Settings → Accounts (ticking the box does not change a token that already exists).",
        if missing.len() == 1 { "" } else { "s" },
        missing.join(", "),
        if missing.len() == 1 { "it" } else { "them" },
    )
}

pub struct DropboxBackupProvider {
    pool: PgPool,
    client: reqwest::Client,
    folder_ensured: Arc<Mutex<bool>>,
    required_scopes: &'static [&'static str],
}

#[derive(Deserialize)]
struct DropboxFileMetadata {
    id: String,
}

#[derive(Deserialize)]
struct DropboxListFolderResult {
    entries: Vec<DropboxListEntry>,
}

#[derive(Deserialize)]
struct DropboxListEntry {
    #[serde(rename = ".tag")]
    tag: String,
    id: Option<String>,
    name: Option<String>,
    size: Option<u64>,
    server_modified: Option<String>,
}

enum ChunkError {
    Transient {
        message: String,
        /// INVARIANT: Dropbox's own `retry_after` hint in seconds, 0 when it sent none. It
        /// always sends one on a 429, and a retry that ignores it just spends
        /// itself against the same limit.
        retry_after: u64,
    },
    Fatal(BoxError),
}

impl DropboxBackupProvider {
    pub fn new(pool: PgPool, required_scopes: &'static [&'static str]) -> Self {
        Self {
            pool,
            // INVARIANT: A connect timeout, NOT a whole-request one: `download` streams a
            // multi-GB archive over a single response, so a request-wide
            // default would kill every large restore. The upload calls set
            // their own per-request timeouts instead.
            client: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .build()
                .expect("a reqwest client with a connect timeout is constructible"),
            folder_ensured: Arc::new(Mutex::new(false)),
            required_scopes,
        }
    }

    fn verify_scopes(&self, granted: &str) -> Result<(), BoxError> {
        let missing = super::missing_scopes(granted, self.required_scopes);
        if missing.is_empty() {
            Ok(())
        } else {
            Err(missing_scope_message(&missing).into())
        }
    }

    async fn ensure_folder(&self, token: &str) -> Result<(), BoxError> {
        {
            let ensured = self.folder_ensured.lock().await;
            if *ensured {
                return Ok(());
            }
        }

        let resp = self
            .client
            .post("https://api.dropboxapi.com/2/files/create_folder_v2")
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .body(serde_json::json!({ "path": BACKUP_FOLDER, "autorename": false }).to_string())
            .send()
            .await?;

        if resp.status() == 409 || resp.status().is_success() {
            let mut ensured = self.folder_ensured.lock().await;
            *ensured = true;
            return Ok(());
        }

        check_dropbox_status(resp, "create the backups folder").await?;
        Ok(())
    }

    /// INVARIANT: Best-effort free-space check against `users/get_space_usage`.
    ///
    /// Best-effort where Drive's equivalent is mandatory, because this endpoint
    /// needs `account_info.read` and that scope is deliberately NOT required for
    /// a backup (see [`BACKUP_SCOPES`]): an account without it must not have a
    /// working backup turned into a failure, so anything short of a parsed
    /// over-quota verdict passes. With the scope present, it buys the fail-fast
    /// Drive already gets: "Dropbox is full" in seconds, rather than an
    /// `insufficient_space` after the whole archive pipeline has run.
    async fn check_free_space(
        &self,
        token: &str,
        estimated_upload_bytes: u64,
    ) -> Result<(), BoxError> {
        #[derive(Deserialize)]
        struct SpaceAllocation {
            allocated: Option<u64>,
        }
        #[derive(Deserialize)]
        struct SpaceUsage {
            used: Option<u64>,
            allocation: Option<SpaceAllocation>,
        }

        // INVARIANT: The route takes no arguments, so it is posted with no body and no
        // Content-Type at all.
        let usage = match self
            .client
            .post(SPACE_USAGE_URL)
            .bearer_auth(token)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => resp.json::<SpaceUsage>().await,
            Ok(resp) => {
                crate::log_legacy!(
                    "[Backup] Skipping the Dropbox free-space check (HTTP {}); \
                     it needs the optional account_info.read scope",
                    resp.status()
                );
                return Ok(());
            }
            Err(e) => {
                crate::log_legacy!("[Backup] Skipping the Dropbox free-space check: {}", e);
                return Ok(());
            }
        };
        let usage = match usage {
            Ok(usage) => usage,
            Err(e) => {
                crate::log_legacy!(
                    "[Backup] Unreadable Dropbox space usage, not blocking: {}",
                    e
                );
                return Ok(());
            }
        };

        space_check(
            usage.allocation.and_then(|a| a.allocated),
            usage.used.unwrap_or(0),
            estimated_upload_bytes,
        )
        .map_err(BoxError::from)
    }

    async fn send_upload_post(
        &self,
        url: &str,
        token: &str,
        api_arg: &serde_json::Value,
        body: Bytes,
        timeout: Duration,
    ) -> Result<reqwest::Response, ChunkError> {
        self.client
            .post(url)
            .bearer_auth(token)
            .timeout(timeout)
            .header("Content-Type", "application/octet-stream")
            .header("Dropbox-API-Arg", api_arg.to_string())
            .body(body)
            .send()
            .await
            .map_err(|e| ChunkError::Transient {
                message: super::transport_failure("Dropbox", "uploading", e),
                retry_after: 0,
            })
    }

    /// INVARIANT: Replace `token` with a freshly-resolved one, keeping the current value if
    /// the lookup fails.
    ///
    /// Called before every chunk, not once per upload. Dropbox's short-lived
    /// tokens last four hours and `get_oauth_token` only refreshes inside the
    /// last minute of that. A backup starting on a nearly-expired token holds
    /// one good for as little as a minute. A session authenticates per call
    /// over minutes to hours, so it would 401 partway through. The lookup is
    /// one indexed SELECT, nothing beside the network each chunk costs.
    ///
    /// A failed lookup keeps the token already in hand rather than aborting: a
    /// database blip must not cost a multi-GB transfer that is otherwise fine.
    async fn refresh_token(&self, token: &mut String) {
        if let Some(fresh) = self.current_token().await {
            *token = fresh;
        }
    }

    async fn current_token(&self) -> Option<String> {
        match super::get_oauth_token(&self.pool, "dropbox").await {
            Ok(token) => Some(token),
            Err(e) => {
                crate::log_legacy!(
                    "[Backup] Keeping the current Dropbox token, refresh lookup failed: {}",
                    e
                );
                None
            }
        }
    }

    async fn committed_file_id(&self, token: &str, dropbox_path: &str) -> Option<String> {
        let resp = self
            .client
            .post(GET_METADATA_URL)
            .bearer_auth(token)
            .timeout(CHUNK_REQUEST_TIMEOUT)
            .header("Content-Type", "application/json")
            .body(serde_json::json!({ "path": dropbox_path }).to_string())
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.json::<DropboxFileMetadata>().await.ok().map(|m| m.id)
    }

    async fn start_upload_session(&self, token: &str) -> Result<String, BoxError> {
        #[derive(Deserialize)]
        struct SessionStart {
            session_id: String,
        }

        let arg = serde_json::json!({ "close": false });
        retry_transient("start the backup upload", || async {
            let resp = self
                .send_upload_post(
                    SESSION_START_URL,
                    token,
                    &arg,
                    Bytes::new(),
                    CHUNK_REQUEST_TIMEOUT,
                )
                .await?;
            let status = resp.status();
            if status.is_success() {
                let started: SessionStart = resp.json().await.map_err(|e| {
                    ChunkError::Fatal(
                        format!("Unreadable Dropbox upload_session/start response: {}", e).into(),
                    )
                })?;
                return Ok(started.session_id);
            }
            let retry_after = retry_after_header(resp.headers());
            let body = resp.text().await.unwrap_or_default();
            Err(classify_upload_failure(
                status,
                &body,
                retry_after,
                "start the backup upload",
            ))
        })
        .await
    }

    /// INVARIANT: Append one chunk at `offset`, and return the offset Dropbox expects next.
    ///
    /// A 200 and a `409 incorrect_offset` answer the same question, so both
    /// come back as an offset. The 409 is not a failure: Dropbox returns it when
    /// it already holds bytes we are about to re-send. That is what a retried
    /// append looks like after a success whose response never reached us.
    async fn append_chunk(
        &self,
        token: &str,
        session_id: &str,
        offset: u64,
        body: Bytes,
    ) -> Result<u64, ChunkError> {
        let next = offset + body.len() as u64;
        let arg = serde_json::json!({
            "cursor": { "session_id": session_id, "offset": offset },
            "close": false,
        });
        let resp = self
            .send_upload_post(SESSION_APPEND_URL, token, &arg, body, CHUNK_REQUEST_TIMEOUT)
            .await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(next);
        }
        let retry_after = retry_after_header(resp.headers());
        let body = resp.text().await.unwrap_or_default();
        if status == StatusCode::CONFLICT {
            if let Some(correct) = correct_offset_from_body(&body) {
                return Ok(correct);
            }
        }
        Err(classify_upload_failure(
            status,
            &body,
            retry_after,
            "upload a chunk of the backup",
        ))
    }

    async fn finish_upload_session(
        &self,
        token: &str,
        session_id: &str,
        total: u64,
        dropbox_path: &str,
    ) -> Result<String, BoxError> {
        let arg = serde_json::json!({
            "cursor": { "session_id": session_id, "offset": total },
            "commit": commit_args(dropbox_path),
        });
        retry_transient("finish the backup upload", || async {
            // INVARIANT: Resolved per attempt rather than closed over: a retry chain here
            // can outlive a token that had under a minute of life left when the
            // last chunk went up. This is the one call whose failure discards
            // the whole transfer.
            let token = self
                .current_token()
                .await
                .unwrap_or_else(|| token.to_string());
            let resp = self
                .send_upload_post(
                    SESSION_FINISH_URL,
                    &token,
                    &arg,
                    Bytes::new(),
                    COMMIT_REQUEST_TIMEOUT,
                )
                .await?;
            let status = resp.status();
            if status.is_success() {
                let meta: DropboxFileMetadata = resp.json().await.map_err(|e| {
                    ChunkError::Fatal(
                        format!("Unreadable Dropbox upload_session/finish response: {}", e).into(),
                    )
                })?;
                return Ok(meta.id);
            }
            let retry_after = retry_after_header(resp.headers());
            let body = resp.text().await.unwrap_or_default();
            Err(classify_upload_failure(
                status,
                &body,
                retry_after,
                "finish the backup upload",
            ))
        })
        .await
    }

    /// INVARIANT: Dropbox's upload session protocol: `start`, one `append_v2` per
    /// [`UPLOAD_CHUNK_SIZE`] chunk, then `finish` with the commit info.
    ///
    /// Peak memory is one chunk. The file is opened once and each chunk read
    /// into a fresh buffer at its own offset, never the whole archive into one
    /// `Vec<u8>`. Multiple GB resident is enough to hard-freeze a swap-less
    /// machine, during the very backup meant to protect it.
    ///
    /// The loop is flat where Drive's resumable one is nested, because Dropbox
    /// has no separate "where are you?" call. A session that has drifted reports
    /// its own offset in the `409 incorrect_offset` body, so recovery is the
    /// next iteration reading from there. `stalls` counts consecutive rounds
    /// that gained no ground, so a session that keeps failing gives up instead
    /// of spinning. An accepted chunk resets it.
    async fn session_upload(
        &self,
        file_path: &Path,
        total: u64,
        dropbox_path: &str,
        token: &str,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<String, BoxError> {
        let mut token = token.to_string();
        let session_id = self.start_upload_session(&token).await?;
        let mut file = tokio::fs::File::open(file_path).await?;
        let mut offset: u64 = 0;
        let mut stalls: u32 = 0;
        progress(0, total);

        while offset < total {
            self.refresh_token(&mut token).await;

            // INVARIANT: Read the chunk fresh every round rather than caching it for a
            // retry: a retry has already slept for seconds, and the offset it
            // resumes from may not be the one the old buffer held.
            let len = chunk_len(offset, total, UPLOAD_CHUNK_SIZE) as usize;
            file.seek(SeekFrom::Start(offset)).await?;
            let mut buf = vec![0u8; len];
            file.read_exact(&mut buf).await?;

            match self
                .append_chunk(&token, &session_id, offset, Bytes::from(buf))
                .await
            {
                Ok(next) if next > offset => {
                    offset = next.min(total);
                    stalls = 0;
                    progress(offset, total);
                    crate::log_legacy!("[Backup] Uploaded chunk: {} / {} bytes", offset, total);
                }
                Ok(next) => {
                    stalls += 1;
                    if let Some(e) = stalled_error(stalls, offset, "session kept resyncing") {
                        return Err(e);
                    }
                    crate::log_legacy!(
                        "[Backup] Dropbox resynced the upload session from byte {} to {}",
                        offset,
                        next
                    );
                    offset = next;
                }
                Err(ChunkError::Fatal(e)) => return Err(e),
                Err(ChunkError::Transient {
                    message,
                    retry_after,
                }) => {
                    stalls += 1;
                    if let Some(e) = stalled_error(stalls, offset, &message) {
                        return Err(e);
                    }
                    let backoff = backoff_secs(stalls - 1, retry_after);
                    crate::log_legacy!(
                        "[Backup] Chunk failed at byte {} (retry {}/{} after {}s): {}",
                        offset,
                        stalls,
                        MAX_RETRIES_PER_CHUNK,
                        backoff,
                        message
                    );
                    tokio::time::sleep(Duration::from_secs(backoff)).await;
                }
            }
        }

        self.refresh_token(&mut token).await;
        let id = match self
            .finish_upload_session(&token, &session_id, total, dropbox_path)
            .await
        {
            Ok(id) => id,
            Err(e) => match self.committed_file_id(&token, dropbox_path).await {
                Some(id) => {
                    crate::log_legacy!(
                        "[Backup] Dropbox reported '{}' but the archive is committed at {}",
                        e,
                        dropbox_path
                    );
                    id
                }
                None => return Err(e),
            },
        };
        progress(total, total);
        crate::log_legacy!("[Backup] Upload complete: {} / {} bytes", total, total);
        Ok(id)
    }

    async fn single_shot_upload(
        &self,
        file_path: &Path,
        total: u64,
        dropbox_path: &str,
        token: &str,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<String, BoxError> {
        let file_bytes = tokio::fs::read(file_path).await?;
        progress(0, total);

        let resp = check_dropbox_status(
            self.client
                .post(UPLOAD_URL)
                .bearer_auth(token)
                .timeout(COMMIT_REQUEST_TIMEOUT)
                .header("Content-Type", "application/octet-stream")
                .header("Dropbox-API-Arg", commit_args(dropbox_path).to_string())
                .body(file_bytes)
                .send()
                .await?,
            "upload the backup",
        )
        .await?;

        progress(total, total);
        let meta: DropboxFileMetadata = resp.json().await?;
        Ok(meta.id)
    }
}

fn fits_single_shot(total_bytes: u64) -> bool {
    total_bytes <= SINGLE_SHOT_MAX
}

fn chunk_len(offset: u64, total: u64, chunk_size: u64) -> u64 {
    chunk_size.min(total.saturating_sub(offset))
}

fn commit_args(dropbox_path: &str) -> serde_json::Value {
    serde_json::json!({
        "path": dropbox_path,
        "mode": "add",
        "autorename": true,
        "mute": true,
    })
}

fn backoff_secs(retry: u32, retry_after: u64) -> u64 {
    let ladder: u64 = 1u64 << retry.min(5);
    ladder.max(retry_after.min(MAX_RETRY_AFTER_SECS))
}

fn stalled_error(stalls: u32, offset: u64, last: &str) -> Option<BoxError> {
    (stalls > MAX_RETRIES_PER_CHUNK).then(|| -> BoxError {
        format!(
            "Dropbox upload made no progress at byte {} after {} retries: {}",
            offset, MAX_RETRIES_PER_CHUNK, last
        )
        .into()
    })
}

fn correct_offset_from_body(body: &str) -> Option<u64> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let error = &json["error"];
    error["correct_offset"]
        .as_u64()
        .or_else(|| error["lookup_failed"]["correct_offset"].as_u64())
}

fn retry_after_from_body(body: &str) -> Option<u64> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    json["error"]["retry_after"]
        .as_u64()
        .or_else(|| json["retry_after"].as_u64())
}

fn retry_after_header(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// INVARIANT: True when a 401 body says the *token* went stale, rather than the grant
/// behind it being wrong. Dropbox tags these `expired_access_token` and
/// `invalid_access_token`. Substring rather than a parse, because the tag
/// appears in both `error_summary` and the error union, and a non-JSON body
/// should still match.
fn is_stale_token_body(body: &str) -> bool {
    body.contains("expired_access_token") || body.contains("invalid_access_token")
}

fn classify_upload_failure(
    status: StatusCode,
    body: &str,
    retry_after_header: Option<u64>,
    context: &str,
) -> ChunkError {
    // INVARIANT: Checked before anything else, so a permission problem can never be read
    // as something that will fix itself. Dropbox reports a missing scope on a
    // 401, the same status a merely stale token arrives on.
    if let Some(scope) = missing_scope_from_body(body) {
        return ChunkError::Fatal(missing_scope_message(&[scope.as_str()]).into());
    }
    let stale_token = status == StatusCode::UNAUTHORIZED && is_stale_token_body(body);
    if stale_token
        || status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
    {
        return ChunkError::Transient {
            message: format!("HTTP {}: {}", status.as_u16(), body),
            retry_after: retry_after_header
                .or_else(|| retry_after_from_body(body))
                .unwrap_or(0),
        };
    }
    ChunkError::Fatal(format!("Dropbox failed to {} ({}): {}", context, status, body).into())
}

/// INVARIANT: Run one replayable upload call under the shared retry policy: up to
/// [`MAX_RETRIES_PER_CHUNK`] attempts, transient failures only, [`backoff_secs`]
/// between them.
///
/// Only `start` and `finish` use it. Both carry no bytes, so replaying one
/// re-sends a small JSON header. Retrying `finish` is what stops a single 503
/// discarding a multi-GB upload that already landed.
/// The per-chunk append cannot use it: its retry has to re-read the file at
/// whatever offset the session resynced to, which is the loop in
/// [`DropboxBackupProvider::session_upload`].
async fn retry_transient<T, F, Fut>(context: &str, mut call: F) -> Result<T, BoxError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ChunkError>>,
{
    let mut retry: u32 = 0;
    loop {
        match call().await {
            Ok(value) => return Ok(value),
            Err(ChunkError::Fatal(e)) => return Err(e),
            Err(ChunkError::Transient {
                message,
                retry_after,
            }) => {
                if retry >= MAX_RETRIES_PER_CHUNK {
                    return Err(format!(
                        "Dropbox could not {} after {} retries: {}",
                        context, MAX_RETRIES_PER_CHUNK, message
                    )
                    .into());
                }
                let backoff = backoff_secs(retry, retry_after);
                crate::log_legacy!(
                    "[Backup] Could not {} (retry {}/{} after {}s): {}",
                    context,
                    retry + 1,
                    MAX_RETRIES_PER_CHUNK,
                    backoff,
                    message
                );
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                retry += 1;
            }
        }
    }
}

fn space_check(
    allocated: Option<u64>,
    used: u64,
    estimated_upload_bytes: u64,
) -> Result<(), String> {
    let Some(allocated) = allocated.filter(|a| *a > 0) else {
        return Ok(());
    };
    let free = allocated.saturating_sub(used);
    let needed = estimated_upload_bytes.saturating_add(estimated_upload_bytes / 10);
    if free < needed {
        Err(format!(
            "Dropbox is full: {:.1} GB free, need ~{:.1} GB. Delete old backups or free space.",
            free as f64 / BYTES_PER_GB,
            needed as f64 / BYTES_PER_GB,
        ))
    } else {
        Ok(())
    }
}

async fn check_dropbox_status(
    resp: reqwest::Response,
    context: &str,
) -> Result<reqwest::Response, BoxError> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if let Some(scope) = missing_scope_from_body(&body) {
        return Err(missing_scope_message(&[scope.as_str()]).into());
    }
    Err(format!("Dropbox failed to {} ({}): {}", context, status, body).into())
}

fn dropbox_folder_url() -> String {
    let mut url =
        reqwest::Url::parse("https://www.dropbox.com/home").expect("static base URL is valid");
    url.path_segments_mut()
        .expect("base URL is a base")
        .extend(BACKUP_FOLDER.split('/').filter(|s| !s.is_empty()));
    url.to_string()
}

#[async_trait]
impl BackupProvider for DropboxBackupProvider {
    fn name(&self) -> &str {
        "Dropbox"
    }

    fn id(&self) -> &str {
        "dropbox"
    }

    fn oauth_provider(&self) -> &str {
        "dropbox"
    }

    async fn folder_url(&self) -> Option<String> {
        Some(dropbox_folder_url())
    }

    async fn preflight(&self, estimated_upload_bytes: u64) -> Result<(), BoxError> {
        let account = super::get_oauth_account(&self.pool, "dropbox").await?;
        self.verify_scopes(&account.scopes)?;
        // INVARIANT: 3. Free space: `users/get_space_usage` IS a quota endpoint, so the
        //    size hint has real work to do here. Best-effort, because that
        //    endpoint needs the one scope a backup does not require, so an
        //    account without it passes untested (see
        //    [`Self::check_free_space`]).
        self.check_free_space(&account.access_token, estimated_upload_bytes)
            .await?;
        self.ensure_folder(&account.access_token).await?;
        Ok(())
    }

    async fn upload(
        &self,
        file_path: &Path,
        filename: &str,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<String, BoxError> {
        let token = super::get_oauth_token(&self.pool, "dropbox").await?;
        self.ensure_folder(&token).await?;

        let total = tokio::fs::metadata(file_path).await?.len();
        if total == 0 {
            return Err("Refusing to upload empty backup file".into());
        }
        let dropbox_path = format!("{}/{}", BACKUP_FOLDER, filename);

        if fits_single_shot(total) {
            crate::log_legacy!(
                "[Backup] Starting single-shot upload: {} ({} bytes)",
                filename,
                total
            );
            return self
                .single_shot_upload(file_path, total, &dropbox_path, &token, progress)
                .await;
        }

        crate::log_legacy!(
            "[Backup] Starting upload session: {} ({} bytes, chunk {} bytes)",
            filename,
            total,
            UPLOAD_CHUNK_SIZE
        );
        self.session_upload(file_path, total, &dropbox_path, &token, progress)
            .await
    }

    async fn list_backups(&self) -> Result<Vec<BackupEntry>, BoxError> {
        let token = super::get_oauth_token(&self.pool, "dropbox").await?;
        self.ensure_folder(&token).await?;

        let resp = check_dropbox_status(
            self.client
                .post("https://api.dropboxapi.com/2/files/list_folder")
                .bearer_auth(&token)
                .header("Content-Type", "application/json")
                .body(serde_json::json!({ "path": BACKUP_FOLDER, "limit": 100 }).to_string())
                .send()
                .await?,
            "list backups",
        )
        .await?;

        let result: DropboxListFolderResult = resp.json().await?;

        let mut entries = Vec::new();
        for entry in result.entries {
            if entry.tag != "file" {
                continue;
            }
            let created_at = entry
                .server_modified
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);

            entries.push(BackupEntry {
                id: entry.id.unwrap_or_default(),
                filename: entry.name.unwrap_or_default(),
                size_bytes: entry.size.unwrap_or(0),
                created_at,
            });
        }

        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(entries)
    }

    async fn download(
        &self,
        backup_id: &str,
        dest: &Path,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), BoxError> {
        let token = super::get_oauth_token(&self.pool, "dropbox").await?;

        let api_arg = serde_json::json!({ "path": backup_id });

        let resp = check_dropbox_status(
            self.client
                .post("https://content.dropboxapi.com/2/files/download")
                .bearer_auth(&token)
                .header("Dropbox-API-Arg", api_arg.to_string())
                .send()
                .await?,
            "download the backup",
        )
        .await?;

        let total = resp.content_length().unwrap_or(0);
        let mut file = tokio::fs::File::create(dest).await?;
        let mut downloaded: u64 = 0;
        progress(0, total);

        let mut stream = resp.bytes_stream();
        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            progress(downloaded, total);
        }
        file.flush().await?;

        Ok(())
    }

    async fn delete(&self, backup_id: &str) -> Result<(), BoxError> {
        let token = super::get_oauth_token(&self.pool, "dropbox").await?;

        check_dropbox_status(
            self.client
                .post("https://api.dropboxapi.com/2/files/delete_v2")
                .bearer_auth(&token)
                .header("Content-Type", "application/json")
                .body(serde_json::json!({ "path": backup_id }).to_string())
                .send()
                .await?,
            "delete an old backup",
        )
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropbox_folder_url_deep_links_to_home_path() {
        let url = dropbox_folder_url();
        assert!(url.starts_with("https://www.dropbox.com/home/"));
        assert!(url.contains("Lucidos"));
        assert!(!url.contains(' '));
        assert_eq!(url, "https://www.dropbox.com/home/Lucidos%20Backups");
    }

    #[test]
    fn a_fully_scoped_grant_is_missing_nothing() {
        let granted =
            "account_info.read files.metadata.read files.content.read files.content.write";
        assert!(super::super::missing_scopes(granted, BACKUP_SCOPES).is_empty());
    }

    #[test]
    fn an_account_info_only_grant_is_missing_every_backup_scope() {
        assert_eq!(
            super::super::missing_scopes("account_info.read", BACKUP_SCOPES),
            vec![
                "files.content.write",
                "files.content.read",
                "files.metadata.read"
            ]
        );
    }

    #[test]
    fn a_write_only_grant_is_still_missing_the_read_scopes() {
        assert_eq!(
            super::super::missing_scopes("files.content.write", BACKUP_SCOPES),
            vec!["files.content.read", "files.metadata.read"]
        );
    }

    #[test]
    fn one_missing_scope_is_reported_alone() {
        let granted = "files.content.write files.content.read account_info.read";
        assert_eq!(
            super::super::missing_scopes(granted, BACKUP_SCOPES),
            vec!["files.metadata.read"]
        );
    }

    #[test]
    fn an_empty_grant_is_missing_every_backup_scope() {
        assert_eq!(
            super::super::missing_scopes("", BACKUP_SCOPES).len(),
            BACKUP_SCOPES.len()
        );
    }

    #[test]
    fn a_grant_without_account_info_is_still_a_working_backup() {
        let granted = "files.content.write files.content.read files.metadata.read";
        assert!(super::super::missing_scopes(granted, BACKUP_SCOPES).is_empty());
    }

    #[test]
    fn the_message_names_the_console_the_scopes_and_the_reconnect() {
        let msg = missing_scope_message(&["files.content.write"]);
        assert!(msg.contains("files.content.write"));
        assert!(msg.contains("Dropbox App Console"));
        assert!(msg.contains("Settings → Accounts"));
        assert!(msg.contains("the permission files.content.write"));
        let many = missing_scope_message(&["files.content.write", "files.metadata.read"]);
        assert!(many.contains("the permissions files.content.write, files.metadata.read"));
    }

    #[test]
    fn missing_scope_is_read_from_the_structured_body() {
        let body = r#"{"error_summary":"missing_scope/.","error":{".tag":"missing_scope","required_scope":"files.metadata.read"}}"#;
        assert_eq!(
            missing_scope_from_body(body).as_deref(),
            Some("files.metadata.read")
        );
    }

    #[test]
    fn missing_scope_is_read_from_the_prose_body() {
        let body = "Error in call to API function \"files/create_folder_v2\": Your app (ID: 1234567) is not permitted to access this endpoint because it does not have the required scope 'files.content.write'. The owner of the app can enable the scope for the app using the Permissions tab on the App Console.";
        assert_eq!(
            missing_scope_from_body(body).as_deref(),
            Some("files.content.write")
        );
    }

    #[test]
    fn an_unrelated_error_body_names_no_scope() {
        assert_eq!(missing_scope_from_body("insufficient_space"), None);
        assert_eq!(
            missing_scope_from_body(r#"{"error_summary":"path/not_found/."}"#),
            None
        );
        assert_eq!(
            missing_scope_from_body("could not find 'Lucidos Backups'"),
            None
        );
    }

    #[test]
    fn a_small_archive_still_goes_up_in_one_request() {
        assert!(fits_single_shot(1));
        assert!(fits_single_shot(64 * 1024 * 1024));
        assert!(fits_single_shot(SINGLE_SHOT_MAX - 1));
    }

    #[test]
    fn an_archive_of_exactly_the_limit_is_still_a_single_shot() {
        assert_eq!(SINGLE_SHOT_MAX, 150 * 1024 * 1024);
        assert!(fits_single_shot(SINGLE_SHOT_MAX));
    }

    #[test]
    fn one_byte_over_the_limit_needs_an_upload_session() {
        assert!(!fits_single_shot(SINGLE_SHOT_MAX + 1));
    }

    #[test]
    fn the_archive_that_failed_would_now_take_the_session_path() {
        let four_point_three_six_gb = 4_360_000_000u64;
        assert!(!fits_single_shot(four_point_three_six_gb));
    }

    #[test]
    fn a_full_chunk_is_read_until_the_tail() {
        let total = 100 * 1024 * 1024;
        assert_eq!(chunk_len(0, total, UPLOAD_CHUNK_SIZE), UPLOAD_CHUNK_SIZE);
        assert_eq!(
            chunk_len(UPLOAD_CHUNK_SIZE, total, UPLOAD_CHUNK_SIZE),
            UPLOAD_CHUNK_SIZE
        );
    }

    #[test]
    fn the_last_chunk_of_a_non_aligned_file_is_only_the_remainder() {
        let total: u64 = 442 * 1024 * 1024 + 1234;
        let mut offset = 0u64;
        let mut chunks = 0u32;
        while offset < total {
            let len = chunk_len(offset, total, UPLOAD_CHUNK_SIZE);
            assert!(len > 0, "a chunk before the end is never empty");
            assert!(len <= UPLOAD_CHUNK_SIZE);
            offset += len;
            chunks += 1;
        }
        assert_eq!(offset, total, "offsets must land exactly on the total");
        assert_eq!(chunks, 56);
        let last_start = 55 * UPLOAD_CHUNK_SIZE;
        assert_eq!(
            chunk_len(last_start, total, UPLOAD_CHUNK_SIZE),
            total - last_start
        );
        assert!(chunk_len(last_start, total, UPLOAD_CHUNK_SIZE) < UPLOAD_CHUNK_SIZE);
    }

    #[test]
    fn a_file_shorter_than_a_chunk_is_one_append() {
        let total: u64 = 1024 * 1024;
        assert_eq!(chunk_len(0, total, UPLOAD_CHUNK_SIZE), total);
        assert_eq!(chunk_len(total, total, UPLOAD_CHUNK_SIZE), 0);
        assert_eq!(chunk_len(total + 999, total, UPLOAD_CHUNK_SIZE), 0);
    }

    fn transient_of(e: &ChunkError) -> Option<(&str, u64)> {
        match e {
            ChunkError::Transient {
                message,
                retry_after,
            } => Some((message.as_str(), *retry_after)),
            ChunkError::Fatal(_) => None,
        }
    }

    fn fatal_message(e: &ChunkError) -> String {
        match e {
            ChunkError::Fatal(e) => e.to_string(),
            ChunkError::Transient { message, .. } => {
                panic!("expected a fatal error, got transient: {message}")
            }
        }
    }

    #[test]
    fn a_server_error_a_timeout_and_a_rate_limit_are_all_retried() {
        for status in [
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            let err = classify_upload_failure(status, "", None, "upload a chunk of the backup");
            let (message, _) = transient_of(&err)
                .unwrap_or_else(|| panic!("{status} should be transient, got a fatal error"));
            assert!(message.contains(status.as_str()), "got: {message}");
        }
    }

    #[test]
    fn a_client_error_gives_up_instead_of_burning_the_retry_budget() {
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::CONFLICT,
            StatusCode::PAYLOAD_TOO_LARGE,
        ] {
            let err = classify_upload_failure(
                status,
                "path/malformed_path/",
                None,
                "upload a chunk of the backup",
            );
            let message = fatal_message(&err);
            assert!(
                message.contains("Dropbox failed to upload a chunk of the backup"),
                "got: {message}"
            );
        }
    }

    #[test]
    fn a_scope_lost_mid_upload_still_names_the_console() {
        let body = r#"{"error_summary":"missing_scope/.","error":{".tag":"missing_scope","required_scope":"files.content.write"}}"#;
        let err = classify_upload_failure(
            StatusCode::UNAUTHORIZED,
            body,
            None,
            "upload a chunk of the backup",
        );
        let message = fatal_message(&err);
        assert!(message.contains("files.content.write"), "got: {message}");
        assert!(message.contains("Dropbox App Console"), "got: {message}");
    }

    #[test]
    fn a_token_that_expires_mid_session_is_refreshed_not_fatal() {
        for tag in ["expired_access_token", "invalid_access_token"] {
            let body = format!(r#"{{"error_summary":"{tag}/","error":{{".tag":"{tag}"}}}}"#);
            let err = classify_upload_failure(
                StatusCode::UNAUTHORIZED,
                &body,
                None,
                "upload a chunk of the backup",
            );
            assert!(
                transient_of(&err).is_some(),
                "{tag} should be retried, got a fatal error"
            );
        }
        let opaque = classify_upload_failure(
            StatusCode::UNAUTHORIZED,
            "",
            None,
            "upload a chunk of the backup",
        );
        assert!(transient_of(&opaque).is_none(), "an opaque 401 is fatal");
    }

    #[test]
    fn a_rate_limit_carries_dropboxs_own_wait() {
        let body = r#"{"error_summary":"too_many_requests/","error":{"reason":{".tag":"too_many_requests"},"retry_after":15}}"#;
        let from_body =
            classify_upload_failure(StatusCode::TOO_MANY_REQUESTS, body, None, "upload a chunk");
        assert_eq!(transient_of(&from_body).map(|(_, s)| s), Some(15));

        let from_header = classify_upload_failure(
            StatusCode::TOO_MANY_REQUESTS,
            body,
            Some(42),
            "upload a chunk",
        );
        assert_eq!(transient_of(&from_header).map(|(_, s)| s), Some(42));

        let no_hint =
            classify_upload_failure(StatusCode::SERVICE_UNAVAILABLE, "", None, "upload a chunk");
        assert_eq!(transient_of(&no_hint).map(|(_, s)| s), Some(0));
    }

    #[test]
    fn the_backoff_ladder_matches_the_drive_provider() {
        assert_eq!(backoff_secs(0, 0), 1);
        assert_eq!(backoff_secs(1, 0), 2);
        assert_eq!(backoff_secs(2, 0), 4);
        assert_eq!(backoff_secs(3, 0), 8);
        assert_eq!(backoff_secs(4, 0), 16);
        assert_eq!(backoff_secs(5, 0), 32);
        assert_eq!(backoff_secs(6, 0), 32);
        assert_eq!(backoff_secs(100, 0), 32);
    }

    #[test]
    fn a_server_hint_lengthens_the_wait_but_cannot_run_away() {
        assert_eq!(backoff_secs(0, 30), 30);
        assert_eq!(backoff_secs(5, 3), 32);
        assert_eq!(backoff_secs(0, 99_999), MAX_RETRY_AFTER_SECS);
    }

    #[test]
    fn a_session_that_stops_advancing_gives_up_at_the_budget() {
        assert!(stalled_error(1, 4096, "HTTP 503: ").is_none());
        assert!(stalled_error(MAX_RETRIES_PER_CHUNK, 4096, "HTTP 503: ").is_none());
        let err = stalled_error(MAX_RETRIES_PER_CHUNK + 1, 4096, "HTTP 503: ")
            .expect("one past the budget must fail")
            .to_string();
        assert!(err.contains("no progress at byte 4096"), "got: {err}");
    }

    #[test]
    fn an_incorrect_offset_reports_where_dropbox_actually_is() {
        let body = r#"{"error_summary":"incorrect_offset/","error":{".tag":"incorrect_offset","correct_offset":86736}}"#;
        assert_eq!(correct_offset_from_body(body), Some(86736));
    }

    #[test]
    fn a_lookup_failure_reports_the_same_offset_one_level_down() {
        let body = r#"{"error_summary":"lookup_failed/incorrect_offset/","error":{".tag":"lookup_failed","lookup_failed":{".tag":"incorrect_offset","correct_offset":8388608}}}"#;
        assert_eq!(correct_offset_from_body(body), Some(8_388_608));
    }

    #[test]
    fn an_unrelated_conflict_carries_no_offset_to_resume_from() {
        assert_eq!(
            correct_offset_from_body(r#"{"error":{".tag":"closed"}}"#),
            None
        );
        assert_eq!(
            correct_offset_from_body(r#"{"error":{".tag":"not_found"}}"#),
            None
        );
        assert_eq!(correct_offset_from_body("upload session not found"), None);
        assert_eq!(correct_offset_from_body(""), None);
    }

    #[test]
    fn a_dropbox_with_room_passes_preflight() {
        let gib = 1024 * 1024 * 1024;
        assert!(space_check(Some(2048 * gib), 500 * gib, 5 * gib).is_ok());
    }

    #[test]
    fn a_full_dropbox_is_rejected_before_the_pipeline_runs() {
        let gib = 1024 * 1024 * 1024;
        let err = space_check(Some(2 * gib), 2 * gib - 1024, 5 * gib).unwrap_err();
        assert!(err.contains("Dropbox is full"), "got: {err}");
        assert!(err.contains("free"), "should report free GB: {err}");
        assert!(
            !err.contains("App Console"),
            "not a permission problem: {err}"
        );
    }

    #[test]
    fn the_headroom_rejects_a_near_exact_fit() {
        assert!(space_check(Some(1_000_000), 0, 950_000).is_err());
    }

    #[test]
    fn an_unknown_allocation_never_blocks_a_backup() {
        assert!(space_check(None, u64::MAX, u64::MAX / 2).is_ok());
        assert!(space_check(Some(0), 0, u64::MAX / 2).is_ok());
    }
}
