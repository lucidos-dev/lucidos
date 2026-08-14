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

/// The four legs of an archive upload. `/2/files/upload` puts a whole small
/// file in one request; the three `upload_session` endpoints stream a large one
/// past the [`SINGLE_SHOT_MAX`] cap.
const UPLOAD_URL: &str = "https://content.dropboxapi.com/2/files/upload";
const SESSION_START_URL: &str = "https://content.dropboxapi.com/2/files/upload_session/start";
const SESSION_APPEND_URL: &str = "https://content.dropboxapi.com/2/files/upload_session/append_v2";
const SESSION_FINISH_URL: &str = "https://content.dropboxapi.com/2/files/upload_session/finish";
/// Account storage totals, for the preflight free-space check.
const SPACE_USAGE_URL: &str = "https://api.dropboxapi.com/2/users/get_space_usage";
/// Single-file lookup, for deciding whether an ambiguous commit actually landed.
const GET_METADATA_URL: &str = "https://api.dropboxapi.com/2/files/get_metadata";

/// Dropbox's own cap on a single `POST /2/files/upload`. Over this the server
/// closes the connection instead of answering with a JSON error, so the caller
/// sees a bare reqwest transport error. The failure lands at `.send()`, before
/// [`check_dropbox_status`] ever sees a response. Anything above this goes
/// through the upload session API instead.
const SINGLE_SHOT_MAX: u64 = 150 * 1024 * 1024;

/// Bytes per `upload_session/append_v2` call. Matches the Drive provider's
/// `RESUMABLE_CHUNK_SIZE`, so the two providers retry at one granularity. It is
/// also a multiple of the 4 MiB Dropbox's performance guide recommends.
const UPLOAD_CHUNK_SIZE: u64 = 8 * 1024 * 1024;

/// Per-chunk retry budget for transient failures (network, 5xx, 408, 429).
const MAX_RETRIES_PER_CHUNK: u32 = 6;

/// Longest `retry_after` hint we will honor. Dropbox's own hints are seconds to
/// low minutes; the cap stops a malformed one parking a backup for an hour.
const MAX_RETRY_AFTER_SECS: u64 = 300;

/// Per-request timeout for `upload_session/start` and each `append_v2`. Keeps a
/// hung TCP connection from blocking the whole backup: with 8 MiB chunks this
/// allows down to ~70 KB/s before triggering a retry.
const CHUNK_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Per-request timeout for the two calls that commit a whole archive: the
/// single-shot POST, and `upload_session/finish`. Far more generous than the
/// chunk timeout, because both are all-or-nothing. A chunk that times out costs
/// 8 MiB and resumes, where a commit that times out throws away the entire
/// transfer. Dropbox also assembles a multi-GB session server-side before it
/// answers. The single-shot body is capped at [`SINGLE_SHOT_MAX`], so ten
/// minutes still leaves it a floor of ~250 KB/s.
const COMMIT_REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

/// How long to wait for a TCP/TLS connection before giving up. Safe as a
/// client-wide default in a way a whole-request timeout is not: `download`
/// streams a multi-GB archive over one response, so a request-wide default
/// would kill every large restore.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Bytes per GB for the free-space message. Dropbox allocates in binary GB and
/// labels them in decimal units. Divide by 1024³ to match the number the user
/// sees in Dropbox's own UI.
const BYTES_PER_GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Every Dropbox scope a working backup needs, and why:
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

/// The scopes a user actually grants. Dropbox's requirements ARE whole scope
/// names, so this is the same list plus `account_info.read`, which is requested
/// but not required (see above). Exists so every provider answers
/// [`super::name_missing_scopes`] the same way, including the one whose
/// requirements are substrings.
///
/// Mirrors the request string in `backupProviderScopes.ts`, which is what the
/// *Grant access* button actually sends. Change one and change the other: a
/// scope requested but absent here would be named by its raw matcher instead.
pub const GRANT_SCOPES: &[&str] = &[
    "files.content.write",
    "files.content.read",
    "files.metadata.read",
    "account_info.read",
];

/// Pull the scope name out of a Dropbox permission error, whichever shape it
/// arrives in. Dropbox reports the same condition two ways: a structured
/// `{"error": {".tag": "missing_scope", "required_scope": "..."}}` body, and a
/// prose 400 naming the scope in quotes, as in *does not have the required
/// scope 'files.content.write'*. `None` when the body is about something else.
pub fn missing_scope_from_body(body: &str) -> Option<String> {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(scope) = json["error"]["required_scope"].as_str() {
            return Some(scope.to_string());
        }
    }
    // Prose form. Take what sits between the first pair of single quotes after
    // the phrase, not from the whole body. An unrelated quoted string elsewhere
    // in the message then cannot be reported as a scope name.
    let after = body.split_once("required scope")?.1;
    let inner = after.split_once('\'')?.1;
    let (scope, _) = inner.split_once('\'')?;
    (!scope.is_empty()).then(|| scope.to_string())
}

/// What to tell the user when the connected Dropbox account is short a scope.
///
/// Names all three parts of the fix, because each one alone leaves them stuck:
/// the App Console is where the permission is enabled, and re-connecting is what
/// actually issues a token carrying it. Dropbox never upgrades a token or an
/// existing grant when a permission is ticked in the console.
pub fn missing_scope_message(missing: &[&str]) -> String {
    format!(
        "Dropbox is missing the permission{} {}. Enable {} on the Permissions tab of your app in the Dropbox App Console, then reconnect the account in Settings > Accounts (ticking the box does not change a token that already exists).",
        if missing.len() == 1 { "" } else { "s" },
        missing.join(", "),
        if missing.len() == 1 { "it" } else { "them" },
    )
}

pub struct DropboxBackupProvider {
    pool: PgPool,
    client: reqwest::Client,
    folder_ensured: Arc<Mutex<bool>>,
    /// Scopes the token must carry, threaded from the provider registry's
    /// `required_scopes` so there's a single source of truth for them.
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

/// Classification of an upload-call failure for the retry loop.
enum ChunkError {
    /// Network-layer error, or 5xx / 408 / 429: safe to retry after a backoff.
    Transient {
        message: String,
        /// Dropbox's own `retry_after` hint in seconds, 0 when it sent none. It
        /// always sends one on a 429, and a retry that ignores it just spends
        /// itself against the same limit.
        retry_after: u64,
    },
    /// 4xx other than 408 / 429, and parse failures: give up.
    Fatal(BoxError),
}

impl DropboxBackupProvider {
    pub fn new(pool: PgPool, required_scopes: &'static [&'static str]) -> Self {
        Self {
            pool,
            // A connect timeout, NOT a whole-request one: `download` streams a
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

    /// Fail before any real work when the connected account's granted scopes
    /// can't support a backup.
    ///
    /// Dropbox has no `tokeninfo` endpoint to introspect a token with, the way
    /// `google_drive::verify_scope` does. It does return the granted `scope` in
    /// the token response, and `prepare_oauth_flow` stores exactly that. So the
    /// account row is the authoritative record of what this token can do, which
    /// is why this takes the stored scopes: no network, and no second lookup.
    fn verify_scopes(&self, granted: &str) -> Result<(), BoxError> {
        let missing = super::missing_scopes(granted, self.required_scopes);
        if missing.is_empty() {
            Ok(())
        } else {
            Err(missing_scope_message(&missing).into())
        }
    }

    /// Ensure the backup folder exists (cached — only makes the API call once per session).
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

        // 409 = folder already exists — that's expected
        if resp.status() == 409 || resp.status().is_success() {
            let mut ensured = self.folder_ensured.lock().await;
            *ensured = true;
            return Ok(());
        }

        check_dropbox_status(resp, "create the backups folder").await?;
        Ok(())
    }

    /// Best-effort free-space check against `users/get_space_usage`.
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
            /// Carried by both the `individual` and the `team` variant.
            allocated: Option<u64>,
        }
        #[derive(Deserialize)]
        struct SpaceUsage {
            used: Option<u64>,
            allocation: Option<SpaceAllocation>,
        }

        // The route takes no arguments, so it is posted with no body and no
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
                crate::log!(
                    "[Backup] Skipping the Dropbox free-space check (HTTP {}); \
                     it needs the optional account_info.read scope",
                    resp.status()
                );
                return Ok(());
            }
            Err(e) => {
                crate::log!("[Backup] Skipping the Dropbox free-space check: {}", e);
                return Ok(());
            }
        };
        let usage = match usage {
            Ok(usage) => usage,
            Err(e) => {
                crate::log!(
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

    /// One POST to a content-API upload endpoint: JSON args in the
    /// `Dropbox-API-Arg` header, bytes in the body. A network-layer failure,
    /// which is what a connection Dropbox closed looks like, becomes
    /// [`ChunkError::Transient`], so the retry loop can recover from it.
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
                message: format!("network: {}", e),
                retry_after: 0,
            })
    }

    /// Replace `token` with a freshly-resolved one, keeping the current value if
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

    /// A freshly-resolved bearer token, or `None` when the lookup failed. One
    /// indexed SELECT; the refresh behind it only reaches Dropbox when the
    /// token is inside the last minute of its four-hour life.
    async fn current_token(&self) -> Option<String> {
        match super::get_oauth_token(&self.pool, "dropbox").await {
            Ok(token) => Some(token),
            Err(e) => {
                crate::log!(
                    "[Backup] Keeping the current Dropbox token, refresh lookup failed: {}",
                    e
                );
                None
            }
        }
    }

    /// The id of an archive already sitting at `dropbox_path`, if there is one.
    ///
    /// Tells a genuinely failed `finish` apart from one that committed and lost
    /// its response. The replay of an ambiguous `finish` hits a session Dropbox
    /// has already closed, which comes back as a fatal 409. Without this check,
    /// a backup sitting safely in Dropbox is reported as a failure, and
    /// retention pruning is skipped with it. `None` on any doubt, including a
    /// lookup that itself fails, so the original error still wins.
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

    /// Open an upload session and return its id. Carries no data: Dropbox's
    /// performance guide recommends sending bytes only with `append_v2`, which
    /// is also what keeps `start` and `finish` cheap to replay on a retry.
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

    /// Append one chunk at `offset`, and return the offset Dropbox expects next.
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

    /// Commit the session at `total` bytes and return the new file's id.
    ///
    /// The commit args are the ones the single-shot path passes as its whole
    /// `Dropbox-API-Arg`, and the response is the same [`DropboxFileMetadata`].
    /// The returned id therefore means the same thing however it got there.
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
            // Resolved per attempt rather than closed over: a retry chain here
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

    /// Dropbox's upload session protocol: `start`, one `append_v2` per
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

            // Read the chunk fresh every round rather than caching it for a
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
                    crate::log!("[Backup] Uploaded chunk: {} / {} bytes", offset, total);
                }
                Ok(next) => {
                    // Dropbox resynced us to a byte we have already sent past.
                    // Its number wins (it owns the session's offset), but the
                    // round gained nothing, so it spends budget like a retry.
                    stalls += 1;
                    if let Some(e) = stalled_error(stalls, offset, "session kept resyncing") {
                        return Err(e);
                    }
                    crate::log!(
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
                    crate::log!(
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
            // The commit is ambiguous when its response is lost: Dropbox has
            // closed the session, so the replay inside `finish_upload_session`
            // sees a fatal 409 for an archive that is already there. Ask
            // whether the file landed before believing the error.
            Err(e) => match self.committed_file_id(&token, dropbox_path).await {
                Some(id) => {
                    crate::log!(
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
        crate::log!("[Backup] Upload complete: {} / {} bytes", total, total);
        Ok(id)
    }

    /// The original one-request path, kept for archives Dropbox accepts whole.
    /// Reading the file into memory is bounded by [`SINGLE_SHOT_MAX`], which is
    /// the only reason this branch is reachable at all.
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

/// Whether an archive of `total_bytes` can go up in one `/2/files/upload` POST.
/// The boundary is Dropbox's own (see [`SINGLE_SHOT_MAX`]) and it is inclusive:
/// exactly the limit still fits.
fn fits_single_shot(total_bytes: u64) -> bool {
    total_bytes <= SINGLE_SHOT_MAX
}

/// Length of the chunk starting at `offset`: a full `chunk_size` except for the
/// final one, which is whatever remains. An `offset` at or past `total` yields
/// 0, so a caller that has already sent everything reads nothing.
fn chunk_len(offset: u64, total: u64, chunk_size: u64) -> u64 {
    chunk_size.min(total.saturating_sub(offset))
}

/// The commit info for a backup archive. Shared by the single-shot POST, as its
/// entire `Dropbox-API-Arg`, and by `upload_session/finish` as its `commit`
/// field. The two paths therefore land a file with identical semantics.
fn commit_args(dropbox_path: &str) -> serde_json::Value {
    serde_json::json!({
        "path": dropbox_path,
        "mode": "add",
        "autorename": true,
        "mute": true,
    })
}

/// Backoff in seconds before the Nth retry: the ladder 1, 2, 4, 8, 16, 32 that
/// the Drive provider uses, but never shorter than Dropbox's own `retry_after`
/// hint (capped at [`MAX_RETRY_AFTER_SECS`]).
fn backoff_secs(retry: u32, retry_after: u64) -> u64 {
    let ladder: u64 = 1u64 << retry.min(5);
    ladder.max(retry_after.min(MAX_RETRY_AFTER_SECS))
}

/// The error a stalled upload session dies with, once `stalls` consecutive
/// rounds at `offset` have gained no ground. `None` while budget remains.
fn stalled_error(stalls: u32, offset: u64, last: &str) -> Option<BoxError> {
    (stalls > MAX_RETRIES_PER_CHUNK).then(|| -> BoxError {
        format!(
            "Dropbox upload made no progress at byte {} after {} retries: {}",
            offset, MAX_RETRIES_PER_CHUNK, last
        )
        .into()
    })
}

/// Pull `correct_offset` out of a `409 incorrect_offset` body, whichever shape
/// it arrives in: `append_v2` reports it at the top of its error union
/// (`{"error":{".tag":"incorrect_offset","correct_offset":N}}`) while `finish`
/// nests the same thing under `lookup_failed`. `None` for a 409 about anything
/// else, which is a real failure.
fn correct_offset_from_body(body: &str) -> Option<u64> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let error = &json["error"];
    error["correct_offset"]
        .as_u64()
        .or_else(|| error["lookup_failed"]["correct_offset"].as_u64())
}

/// Dropbox's "wait this many seconds" hint from a rate-limited response body.
/// It rides on the `RateLimitError` payload (`{"error":{"retry_after":N}}`);
/// the bare top-level form is accepted too, so a shape change still parses.
fn retry_after_from_body(body: &str) -> Option<u64> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    json["error"]["retry_after"]
        .as_u64()
        .or_else(|| json["retry_after"].as_u64())
}

/// Seconds from a `Retry-After` response header, when it carries a delta rather
/// than an HTTP date. Dropbox sends one on every rate-limited response.
fn retry_after_header(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// True when a 401 body says the *token* went stale, rather than the grant
/// behind it being wrong. Dropbox tags these `expired_access_token` and
/// `invalid_access_token`. Substring rather than a parse, because the tag
/// appears in both `error_summary` and the error union, and a non-JSON body
/// should still match.
fn is_stale_token_body(body: &str) -> bool {
    body.contains("expired_access_token") || body.contains("invalid_access_token")
}

/// Classify a FAILED upload response for the retry loop. 5xx, 408 and 429 are
/// transient per Dropbox's error-handling guide; everything else is fatal.
///
/// Pure over `(status, body)` so the classification is unit-testable without a
/// live response, and it keeps [`check_dropbox_status`]'s mapping: a permission
/// failure mid-upload still produces [`missing_scope_message`] rather than a raw
/// status, and every other failure still names the operation that failed.
fn classify_upload_failure(
    status: StatusCode,
    body: &str,
    retry_after_header: Option<u64>,
    context: &str,
) -> ChunkError {
    // Checked before anything else, so a permission problem can never be read
    // as something that will fix itself. Dropbox reports a missing scope on a
    // 401, the same status a merely stale token arrives on.
    if let Some(scope) = missing_scope_from_body(body) {
        return ChunkError::Fatal(missing_scope_message(&[scope.as_str()]).into());
    }
    // A token that went stale mid-session IS recoverable: the next round picks
    // up a fresh one (see [`DropboxBackupProvider::refresh_token`]). Only the
    // session path reaches this, because only it authenticates once per chunk
    // over minutes or hours.
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

/// Run one replayable upload call under the shared retry policy: up to
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
                crate::log!(
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

/// Decide whether the estimated upload fits in the account's free space, with
/// the same 10% headroom the Drive provider applies. Returns the over-quota
/// message when it does not.
///
/// An absent or zero `allocated` means Dropbox reported no usable allocation,
/// which a team account or a partly-read response can both produce. It then
/// always fits: preflight blocks only on clear evidence of insufficient space.
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

/// Turn a failed Dropbox response into an error the user can act on.
///
/// `context` names the operation ("upload the backup", "list backups") so the
/// message says which leg failed. A permission failure is rewritten into
/// [`missing_scope_message`]; everything else keeps the status and body, which
/// is all Dropbox gives us to go on. Preflight normally catches the scope case
/// first, so reaching this branch means a revoked scope or an over-reporting
/// stored grant.
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

/// Build the Dropbox web URL for the backups folder. The Dropbox web app
/// addresses folders by path under `/home`, and the folder path is fixed. So
/// this is always a real deep link, with no id lookup.
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
        // Cheap, ordered checks, same shape as Drive's: fail before any
        // expensive work and keep the permission guidance for an actual
        // permission problem.
        // 1. Account: connect / refresh. Fetched whole rather than as a bare
        //    token, so step 2 reads the granted scopes off the same row instead
        //    of repeating the lookup.
        let account = super::get_oauth_account(&self.pool, "dropbox").await?;
        // 2. Granted scopes, checked before the folder create. A short-scoped
        //    token would otherwise surface as a raw 400, after the archive
        //    work had already run.
        self.verify_scopes(&account.scopes)?;
        // 3. Free space: `users/get_space_usage` IS a quota endpoint, so the
        //    size hint has real work to do here. Best-effort, because that
        //    endpoint needs the one scope a backup does not require, so an
        //    account without it passes untested (see
        //    [`Self::check_free_space`]).
        self.check_free_space(&account.access_token, estimated_upload_bytes)
            .await?;
        // 4. Folder: verifies write access and warms the cache for the upload.
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

        // Size from the file's metadata, never from reading it: the archive is
        // routinely multiple GB and only one chunk of it belongs in memory.
        let total = tokio::fs::metadata(file_path).await?.len();
        if total == 0 {
            return Err("Refusing to upload empty backup file".into());
        }
        let dropbox_path = format!("{}/{}", BACKUP_FOLDER, filename);

        if fits_single_shot(total) {
            crate::log!(
                "[Backup] Starting single-shot upload: {} ({} bytes)",
                filename,
                total
            );
            return self
                .single_shot_upload(file_path, total, &dropbox_path, &token, progress)
                .await;
        }

        crate::log!(
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

        // Sort newest first
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
        // The space in "Lucidos Backups" must be percent-encoded, never raw.
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

    /// The reported failure: an account connected for its name only.
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

    /// The case that made readiness and preflight disagree while they held
    /// separate lists: a token that CAN write but cannot list. Readiness reads
    /// the same list now, so the page says not-ready and offers Grant access
    /// rather than enabling a backup that fails at preflight.
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

    /// `account_info.read` names the account and nothing else, so an otherwise
    /// complete grant without it must NOT read as broken.
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
        assert!(msg.contains("Settings > Accounts"));
        // Singular wording for one scope, plural for several.
        assert!(msg.contains("the permission files.content.write"));
        let many = missing_scope_message(&["files.content.write", "files.metadata.read"]);
        assert!(many.contains("the permissions files.content.write, files.metadata.read"));
    }

    /// The structured error body Dropbox returns for a scoped app.
    #[test]
    fn missing_scope_is_read_from_the_structured_body() {
        let body = r#"{"error_summary":"missing_scope/.","error":{".tag":"missing_scope","required_scope":"files.metadata.read"}}"#;
        assert_eq!(
            missing_scope_from_body(body).as_deref(),
            Some("files.metadata.read")
        );
    }

    /// The prose 400 form, which names the scope in quotes.
    #[test]
    fn missing_scope_is_read_from_the_prose_body() {
        let body = "Error in call to API function \"files/create_folder_v2\": Your app (ID: 1234567) is not permitted to access this endpoint because it does not have the required scope 'files.content.write'. The owner of the app can enable the scope for the app using the Permissions tab on the App Console.";
        assert_eq!(
            missing_scope_from_body(body).as_deref(),
            Some("files.content.write")
        );
    }

    /// An unrelated failure keeps its own message. Quoting something is not
    /// naming a scope. Mislabelling a path error as a permission problem would
    /// send the user to the App Console for nothing.
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

    // --- single-shot vs upload session (the 4.36 GB failure) --------------

    /// An ordinary small archive still goes up in one request: the session API
    /// costs three round trips and buys nothing under Dropbox's own cap.
    #[test]
    fn a_small_archive_still_goes_up_in_one_request() {
        assert!(fits_single_shot(1));
        assert!(fits_single_shot(64 * 1024 * 1024));
        assert!(fits_single_shot(SINGLE_SHOT_MAX - 1));
    }

    /// The boundary is Dropbox's documented limit and it is inclusive, so an
    /// archive of exactly 150 MB is still a single-shot upload.
    #[test]
    fn an_archive_of_exactly_the_limit_is_still_a_single_shot() {
        assert_eq!(SINGLE_SHOT_MAX, 150 * 1024 * 1024);
        assert!(fits_single_shot(SINGLE_SHOT_MAX));
    }

    /// One byte over, and the single-shot POST is the thing that fails with a
    /// bare transport error, so the session path must take it.
    #[test]
    fn one_byte_over_the_limit_needs_an_upload_session() {
        assert!(!fits_single_shot(SINGLE_SHOT_MAX + 1));
    }

    /// The reported failure: a 4.36 GB archive, ~30x the cap, single-shot POSTed
    /// until Dropbox closed the connection.
    #[test]
    fn the_archive_that_failed_would_now_take_the_session_path() {
        let four_point_three_six_gb = 4_360_000_000u64;
        assert!(!fits_single_shot(four_point_three_six_gb));
    }

    // --- chunk offset arithmetic ------------------------------------------

    #[test]
    fn a_full_chunk_is_read_until_the_tail() {
        let total = 100 * 1024 * 1024;
        assert_eq!(chunk_len(0, total, UPLOAD_CHUNK_SIZE), UPLOAD_CHUNK_SIZE);
        assert_eq!(
            chunk_len(UPLOAD_CHUNK_SIZE, total, UPLOAD_CHUNK_SIZE),
            UPLOAD_CHUNK_SIZE
        );
    }

    /// A file whose size is not a multiple of the chunk size: the last append
    /// must carry only the remainder, and the offsets must land exactly on the
    /// total so `finish` commits the right cursor.
    #[test]
    fn the_last_chunk_of_a_non_aligned_file_is_only_the_remainder() {
        // 442 MiB + 1234 bytes: 55 full 8 MiB chunks, then a 2 MiB + 1234 tail.
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

    /// A file smaller than one chunk is a single append, and an offset already
    /// at (or somehow past) the total reads nothing rather than underflowing.
    #[test]
    fn a_file_shorter_than_a_chunk_is_one_append() {
        let total: u64 = 1024 * 1024;
        assert_eq!(chunk_len(0, total, UPLOAD_CHUNK_SIZE), total);
        assert_eq!(chunk_len(total, total, UPLOAD_CHUNK_SIZE), 0);
        assert_eq!(chunk_len(total + 999, total, UPLOAD_CHUNK_SIZE), 0);
    }

    // --- retry classification ---------------------------------------------

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

    /// Dropbox's own transient set: every 5xx, plus 408 and 429. These are
    /// retried rather than failing a backup that has already done all the work.
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

    /// A 4xx that is not 408 or 429 is the client's fault and will not fix
    /// itself, so it fails immediately and names the operation.
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

    /// A scope revoked mid-upload keeps the App Console guidance
    /// `check_dropbox_status` produces, rather than degrading to a raw status.
    /// It arrives on a 401, the same status a merely stale token uses. So this
    /// also pins that a missing scope is never read as transient.
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

    /// A token that expires partway through a session is recoverable: the next
    /// round refreshes it. Only the chunked path can hit this, because only it
    /// authenticates once per chunk over minutes or hours. Classifying it fatal
    /// would abandon a multi-GB upload over a token Lucidos can renew.
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
        // A 401 that says neither is not a token we can renew, so it still
        // fails immediately rather than burning the whole retry budget.
        let opaque = classify_upload_failure(
            StatusCode::UNAUTHORIZED,
            "",
            None,
            "upload a chunk of the backup",
        );
        assert!(transient_of(&opaque).is_none(), "an opaque 401 is fatal");
    }

    /// A 429 carries Dropbox's own wait, in the header and again in the body.
    /// Either is honored, and the header wins when both are present.
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
        // 1, 2, 4, 8, 16, 32, capped at 32 thereafter, with no server hint.
        assert_eq!(backoff_secs(0, 0), 1);
        assert_eq!(backoff_secs(1, 0), 2);
        assert_eq!(backoff_secs(2, 0), 4);
        assert_eq!(backoff_secs(3, 0), 8);
        assert_eq!(backoff_secs(4, 0), 16);
        assert_eq!(backoff_secs(5, 0), 32);
        assert_eq!(backoff_secs(6, 0), 32);
        assert_eq!(backoff_secs(100, 0), 32);
    }

    /// A `retry_after` hint only ever lengthens the wait, and a wild one is
    /// capped rather than parking the backup indefinitely.
    #[test]
    fn a_server_hint_lengthens_the_wait_but_cannot_run_away() {
        assert_eq!(backoff_secs(0, 30), 30);
        // A hint shorter than the ladder does not shorten it: retrying sooner
        // than the ladder says is what re-spends the same rate limit.
        assert_eq!(backoff_secs(5, 3), 32);
        assert_eq!(backoff_secs(0, 99_999), MAX_RETRY_AFTER_SECS);
    }

    /// The budget is spent by consecutive no-progress rounds; the round that
    /// exceeds it names the byte the upload died at.
    #[test]
    fn a_session_that_stops_advancing_gives_up_at_the_budget() {
        assert!(stalled_error(1, 4096, "HTTP 503: ").is_none());
        assert!(stalled_error(MAX_RETRIES_PER_CHUNK, 4096, "HTTP 503: ").is_none());
        let err = stalled_error(MAX_RETRIES_PER_CHUNK + 1, 4096, "HTTP 503: ")
            .expect("one past the budget must fail")
            .to_string();
        assert!(err.contains("no progress at byte 4096"), "got: {err}");
    }

    // --- session resume (the 409 that is not a failure) -------------------

    /// `append_v2` reports the offset it actually holds at the top of its error
    /// union. Resuming there is how a retry after an unacknowledged success
    /// avoids sending the same chunk twice.
    #[test]
    fn an_incorrect_offset_reports_where_dropbox_actually_is() {
        let body = r#"{"error_summary":"incorrect_offset/","error":{".tag":"incorrect_offset","correct_offset":86736}}"#;
        assert_eq!(correct_offset_from_body(body), Some(86736));
    }

    /// `finish` reports the same thing nested under `lookup_failed`.
    #[test]
    fn a_lookup_failure_reports_the_same_offset_one_level_down() {
        let body = r#"{"error_summary":"lookup_failed/incorrect_offset/","error":{".tag":"lookup_failed","lookup_failed":{".tag":"incorrect_offset","correct_offset":8388608}}}"#;
        assert_eq!(correct_offset_from_body(body), Some(8_388_608));
    }

    /// Every other 409 is a real failure. Reading one as a resume instruction
    /// would silently truncate the archive at whatever offset we invented.
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

    // --- preflight free space ---------------------------------------------

    #[test]
    fn a_dropbox_with_room_passes_preflight() {
        let gib = 1024 * 1024 * 1024;
        assert!(space_check(Some(2048 * gib), 500 * gib, 5 * gib).is_ok());
    }

    /// A full Dropbox is rejected before pg_dump, with the free and needed
    /// sizes named, and never with the grant-access guidance: an account short
    /// of space has a permission problem with nothing.
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

    /// The same 10% headroom Drive applies, so estimate error cannot push a real
    /// archive over the edge at upload time.
    #[test]
    fn the_headroom_rejects_a_near_exact_fit() {
        assert!(space_check(Some(1_000_000), 0, 950_000).is_err());
    }

    /// An account whose allocation we could not read (no `account_info.read`,
    /// an unlimited team allocation, a partial response) is never blocked: the
    /// check exists to fail fast on clear evidence, not to gate the backup on
    /// an optional scope.
    #[test]
    fn an_unknown_allocation_never_blocks_a_backup() {
        assert!(space_check(None, u64::MAX, u64::MAX / 2).is_ok());
        assert!(space_check(Some(0), 0, u64::MAX / 2).is_ok());
    }
}
