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

const FOLDER_NAME: &str = "Lucidos Backups";
const DRIVE_FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";
const DRIVE_UPLOAD_URL: &str = "https://www.googleapis.com/upload/drive/v3/files";
/// `about` endpoint — `?fields=storageQuota` reports the account's limit/usage.
const DRIVE_ABOUT_URL: &str = "https://www.googleapis.com/drive/v3/about";
/// Token introspection — reports the scopes actually granted on the token.
const GOOGLE_TOKENINFO_URL: &str = "https://www.googleapis.com/oauth2/v3/tokeninfo";

/// Bytes per GB for the user-facing free-space message. Google reports its
/// quota in binary GB (a "100 GB" plan = 100 GiB) and labels it "GB", so divide
/// by 1024³ to match the number the user sees in Google's own UI.
const BYTES_PER_GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Chunk size for resumable uploads. Must be a multiple of 256 KiB per Drive's spec.
/// 8 MiB balances request count against the cost of re-uploading a failed chunk.
const RESUMABLE_CHUNK_SIZE: u64 = 8 * 1024 * 1024;

/// Per-chunk retry budget for transient failures (network, 5xx, 408, 429).
const MAX_RETRIES_PER_CHUNK: u32 = 6;

/// Per-request timeout for chunk PUTs and progress queries. Keeps a hung TCP
/// connection from blocking the whole backup — with 8 MiB chunks this allows
/// down to ~70 KB/s before triggering a retry.
const CHUNK_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// User-facing message for Drive auth failures. Shared between the metadata-call
/// path (`check_drive_status`) and the resumable-upload path (`classify_chunk_response`)
/// so the wording stays in sync.
const DRIVE_AUTH_401_MSG: &str = "Google Drive authentication failed (401). Go to Settings > Backup and click 'Grant access' to re-authorize.";
const DRIVE_AUTH_403_MSG: &str = "Google Drive access denied (403). Go to Settings > Backup and click 'Grant access' to authorize Drive permissions.";

/// Shown when a 403 carries a storage-quota reason — an over-quota failure, NOT
/// an access problem. Must NOT mention "Grant access": the old code mapped EVERY
/// 403 to `DRIVE_AUTH_403_MSG`, so an over-quota Drive sent the user re-granting
/// access repeatedly while the real fix was to free space.
const DRIVE_QUOTA_MSG: &str =
    "Google Drive is full — delete old backups or free space; this is NOT an access problem.";

/// Shown by preflight when the granted token is missing the required Drive
/// scope — the ONLY preflight case that should tell the user to re-grant access.
const DRIVE_SCOPE_MISSING_MSG: &str = "Google Drive backup is missing the required Drive permission. Go to Settings > Backup and click 'Grant access' to re-authorize.";

/// Drive-specific resumable-upload headers (reqwest has no constants for these).
const X_UPLOAD_CONTENT_TYPE: &str = "X-Upload-Content-Type";
const X_UPLOAD_CONTENT_LENGTH: &str = "X-Upload-Content-Length";

pub struct GoogleDriveBackupProvider {
    pool: PgPool,
    client: reqwest::Client,
    folder_id_cache: Arc<Mutex<Option<String>>>,
    /// Scope substring the token must carry, threaded from the provider
    /// registry's `required_scope` so there's a single source of truth for it.
    required_scope: &'static str,
}

#[derive(Deserialize)]
struct DriveFile {
    id: String,
    name: Option<String>,
    size: Option<String>,
    #[serde(rename = "createdTime")]
    created_time: Option<String>,
}

#[derive(Deserialize)]
struct DriveFileList {
    files: Vec<DriveFile>,
}

#[derive(Deserialize)]
struct DriveCreateResponse {
    id: String,
}

/// Outcome of a single chunk PUT or a progress query.
enum ChunkOutcome {
    /// Upload finished — Drive returned the file metadata with this ID.
    Done(String),
    /// Server accepted bytes up to (but not including) `next` — keep going.
    Continue(u64),
}

/// Classification of a chunk-level failure for the retry loop.
enum ChunkError {
    /// Network-layer error or 5xx/408/429 — safe to retry after backoff.
    Transient(String),
    /// 4xx (other than 408/429), parse errors, etc. — give up.
    Fatal(BoxError),
}

impl std::fmt::Display for ChunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChunkError::Transient(s) => write!(f, "{}", s),
            ChunkError::Fatal(e) => write!(f, "{}", e),
        }
    }
}

/// Check a Google Drive API response, returning actionable errors for auth failures
/// and the generic reqwest error for everything else. A 403 is classified by its
/// error body so an over-quota failure isn't mislabeled as an access problem
/// (see `classify_403_body`) — async because that classification reads the body.
async fn check_drive_status(resp: reqwest::Response) -> Result<reqwest::Response, BoxError> {
    match resp.status() {
        s if s.is_success() => Ok(resp),
        StatusCode::FORBIDDEN => {
            let body = resp.text().await.unwrap_or_default();
            Err(classify_403_body(&body).into())
        }
        StatusCode::UNAUTHORIZED => Err(DRIVE_AUTH_401_MSG.into()),
        _ => Ok(resp.error_for_status()?),
    }
}

/// True when a Drive 403 body indicates an over-quota failure rather than an
/// access denial. Parses Google's error JSON for a `storageQuotaExceeded`
/// reason in either the v3 (`error.errors[].reason`) or legacy (`errors[].reason`)
/// shape, and falls back to a substring check for "quota" so a shape change or a
/// non-JSON body still classifies correctly.
fn drive_403_is_quota(body: &str) -> bool {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        let reasons = v["error"]["errors"]
            .as_array()
            .or_else(|| v["errors"].as_array());
        if let Some(arr) = reasons {
            if arr.iter().any(|e| {
                e["reason"]
                    .as_str()
                    .is_some_and(|r| r.eq_ignore_ascii_case("storageQuotaExceeded"))
            }) {
                return true;
            }
        }
    }
    body.to_lowercase().contains("quota")
}

/// Pick the right user-facing message for a Drive 403 body: the over-quota
/// message when it's a storage-quota failure (NOT an access problem), otherwise
/// the genuine access-denied "Grant access" message.
fn classify_403_body(body: &str) -> &'static str {
    if drive_403_is_quota(body) {
        DRIVE_QUOTA_MSG
    } else {
        DRIVE_AUTH_403_MSG
    }
}

/// Decide whether the estimated upload fits in the free space (with 10%
/// headroom), returning the over-quota message when it doesn't. `limit == None`
/// means an unlimited quota (some Workspace accounts) and always fits. Pure so
/// the arithmetic is unit-testable without a live `about` response.
fn quota_check(limit: Option<u64>, usage: u64, estimated_upload_bytes: u64) -> Result<(), String> {
    let Some(limit) = limit else {
        return Ok(());
    };
    let free = limit.saturating_sub(usage);
    let needed = estimated_upload_bytes.saturating_add(estimated_upload_bytes / 10);
    if free < needed {
        Err(format!(
            "Google Drive is full: {:.1} GB free, need ~{:.1} GB. Delete old backups or free space.",
            free as f64 / BYTES_PER_GB,
            needed as f64 / BYTES_PER_GB,
        ))
    } else {
        Ok(())
    }
}

/// True when the granted-scopes string includes the required scope substring.
/// Mirrors the readiness check in `api::backup` (substring match against the
/// space-separated granted scopes); an empty requirement always passes.
fn scopes_include(granted: &str, required: &str) -> bool {
    required.is_empty() || granted.contains(required)
}

/// Parse Drive's `Range:` response header (`bytes=0-262143`) and return the
/// next byte to upload (one past the last received byte). Returns `None` if
/// the header is missing or malformed — callers should treat that as
/// "server has nothing yet" (next = 0).
fn parse_range_next(header: &str) -> Option<u64> {
    let s = header.trim();
    // Drive returns "bytes=0-N" but be lenient about leading "bytes "/"bytes=".
    let s = s
        .strip_prefix("bytes=")
        .or_else(|| s.strip_prefix("bytes "))
        .unwrap_or(s);
    let (_, last) = s.split_once('-')?;
    let last: u64 = last.trim().parse().ok()?;
    last.checked_add(1)
}

/// Backoff in seconds for the Nth retry: 1, 2, 4, 8, 16, 32, 32, ...
fn backoff_secs(retry: u32) -> u64 {
    let base: u64 = 1u64 << retry.min(5);
    base.min(32)
}

/// Compute the inclusive end byte of the chunk that starts at `start`.
fn chunk_end(start: u64, total: u64, chunk_size: u64) -> u64 {
    let exclusive_end = start.saturating_add(chunk_size).min(total);
    exclusive_end.saturating_sub(1)
}

impl GoogleDriveBackupProvider {
    pub fn new(pool: PgPool, required_scope: &'static str) -> Self {
        Self {
            pool,
            client: reqwest::Client::new(),
            folder_id_cache: Arc::new(Mutex::new(None)),
            required_scope,
        }
    }

    async fn get_token(&self) -> Result<String, BoxError> {
        super::get_oauth_token(&self.pool, "google").await
    }

    /// Verify the access token actually carries the required Drive scope, via
    /// Google's tokeninfo endpoint. A confirmed-missing scope is the ONLY
    /// preflight failure that tells the user to re-grant access; a tokeninfo
    /// HTTP failure is surfaced as a generic error rather than misattributed to
    /// a missing scope.
    async fn verify_scope(&self, token: &str) -> Result<(), BoxError> {
        #[derive(Deserialize)]
        struct TokenInfo {
            scope: Option<String>,
        }
        let resp = self
            .client
            .get(GOOGLE_TOKENINFO_URL)
            .query(&[("access_token", token)])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!(
                "Could not verify Google Drive token scope (tokeninfo HTTP {})",
                resp.status().as_u16()
            )
            .into());
        }
        let info: TokenInfo = resp.json().await?;
        let granted = info.scope.unwrap_or_default();
        if scopes_include(&granted, self.required_scope) {
            Ok(())
        } else {
            Err(DRIVE_SCOPE_MISSING_MSG.into())
        }
    }

    /// Verify the Drive has room for the estimated upload, via the `about`
    /// endpoint's `storageQuota`. Fails BEFORE encrypting with an actionable
    /// over-quota message (never a grant-access message). An absent limit means
    /// an unlimited quota and passes.
    async fn check_free_space(
        &self,
        token: &str,
        estimated_upload_bytes: u64,
    ) -> Result<(), BoxError> {
        #[derive(Deserialize)]
        struct StorageQuota {
            limit: Option<String>,
            usage: Option<String>,
        }
        #[derive(Deserialize)]
        struct AboutResponse {
            #[serde(rename = "storageQuota")]
            storage_quota: Option<StorageQuota>,
        }
        let resp = self
            .client
            .get(DRIVE_ABOUT_URL)
            .bearer_auth(token)
            .query(&[("fields", "storageQuota")])
            .send()
            .await?;
        let resp = check_drive_status(resp).await?;
        let about: AboutResponse = resp.json().await?;
        let Some(quota) = about.storage_quota else {
            // No quota info reported — don't block the backup on its absence.
            return Ok(());
        };
        // Drive reports limit/usage as stringified bytes; an absent limit means
        // unlimited. Like the None cases above, only block on clear evidence of
        // insufficient space — an unparseable usage defaults to 0 (don't block).
        let limit = quota.limit.as_deref().and_then(|s| s.parse::<u64>().ok());
        let usage = quota
            .usage
            .as_deref()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        quota_check(limit, usage, estimated_upload_bytes).map_err(BoxError::from)
    }

    /// Look up the existing backups folder id WITHOUT creating it. Checks the
    /// in-memory cache first, then searches Drive by folder name. Returns `None`
    /// when no such folder exists yet (e.g. before the first backup). Used by the
    /// Settings page to deep-link the folder, and by `get_or_create_folder`.
    async fn find_folder(&self, token: &str) -> Result<Option<String>, BoxError> {
        {
            let cache = self.folder_id_cache.lock().await;
            if let Some(ref id) = *cache {
                return Ok(Some(id.clone()));
            }
        }

        let query = format!(
            "name = '{}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false",
            FOLDER_NAME
        );
        let resp = self
            .client
            .get(DRIVE_FILES_URL)
            .bearer_auth(token)
            .query(&[("q", &query), ("fields", &"files(id)".to_string())])
            .send()
            .await?;
        let resp = check_drive_status(resp).await?;

        let list: DriveFileList = resp.json().await?;
        let folder_id = list.files.into_iter().next().map(|f| f.id);
        if let Some(ref id) = folder_id {
            let mut cache = self.folder_id_cache.lock().await;
            *cache = Some(id.clone());
        }
        Ok(folder_id)
    }

    async fn get_or_create_folder(&self, token: &str) -> Result<String, BoxError> {
        if let Some(id) = self.find_folder(token).await? {
            return Ok(id);
        }

        // Create the folder
        let metadata = serde_json::json!({
            "name": FOLDER_NAME,
            "mimeType": "application/vnd.google-apps.folder",
        });
        let resp = check_drive_status(
            self.client
                .post(DRIVE_FILES_URL)
                .bearer_auth(token)
                .json(&metadata)
                .send()
                .await?,
        )
        .await?;
        let created: DriveCreateResponse = resp.json().await?;
        let folder_id = created.id;

        // Cache the newly-created folder ID
        {
            let mut cache = self.folder_id_cache.lock().await;
            *cache = Some(folder_id.clone());
        }

        Ok(folder_id)
    }

    /// Step 1 of resumable upload: POST metadata, get back the session URI in
    /// the `Location` header.
    async fn initiate_resumable_session(
        &self,
        token: &str,
        metadata: &serde_json::Value,
        total_size: u64,
    ) -> Result<String, BoxError> {
        let url = format!("{}?uploadType=resumable&fields=id", DRIVE_UPLOAD_URL);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(token)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/json; charset=UTF-8",
            )
            .header(X_UPLOAD_CONTENT_TYPE, "application/octet-stream")
            .header(X_UPLOAD_CONTENT_LENGTH, total_size.to_string())
            .body(metadata.to_string())
            .send()
            .await?;
        let resp = check_drive_status(resp).await?;
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .ok_or("Drive resumable upload: missing Location header")?
            .to_str()
            .map_err(|e| format!("Invalid Location header: {}", e))?
            .to_string();
        Ok(location)
    }

    /// Send one PUT to the resumable session URI — either a chunk (with body)
    /// or a progress query (empty body, `Content-Range: bytes */<total>`).
    /// Network errors become `ChunkError::Transient`; the response is then
    /// classified by `classify_chunk_response`.
    async fn send_resumable_put(
        &self,
        session_uri: &str,
        content_range: String,
        body: Option<Bytes>,
    ) -> Result<ChunkOutcome, ChunkError> {
        let mut req = self
            .client
            .put(session_uri)
            .timeout(CHUNK_REQUEST_TIMEOUT)
            .header(reqwest::header::CONTENT_RANGE, content_range);

        req = match body {
            Some(b) => req.body(b),
            // reqwest sets Content-Length automatically for `Body`s but not
            // for an empty PUT — be explicit so the request is well-formed.
            None => req.header(reqwest::header::CONTENT_LENGTH, "0"),
        };

        match req.send().await {
            Ok(resp) => classify_chunk_response(resp).await,
            Err(e) => Err(ChunkError::Transient(format!("network: {}", e))),
        }
    }

    async fn put_chunk(
        &self,
        session_uri: &str,
        start: u64,
        end: u64,
        total: u64,
        body: Bytes,
    ) -> Result<ChunkOutcome, ChunkError> {
        self.send_resumable_put(
            session_uri,
            format!("bytes {}-{}/{}", start, end, total),
            Some(body),
        )
        .await
    }

    async fn query_progress(
        &self,
        session_uri: &str,
        total: u64,
    ) -> Result<ChunkOutcome, ChunkError> {
        self.send_resumable_put(session_uri, format!("bytes */{}", total), None)
            .await
    }

    /// Drive's resumable upload protocol — chunks the file and recovers from
    /// transient failures by querying server-side progress and resuming from
    /// the next byte the server says it needs.
    ///
    /// See https://developers.google.com/workspace/drive/api/guides/manage-uploads#resumable
    async fn resumable_upload(
        &self,
        file_path: &Path,
        total_size: u64,
        metadata: &serde_json::Value,
        token: &str,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<String, BoxError> {
        if total_size == 0 {
            return Err("Refusing to upload empty backup file".into());
        }

        let session_uri = self
            .initiate_resumable_session(token, metadata, total_size)
            .await?;

        let mut file = tokio::fs::File::open(file_path).await?;
        let mut start: u64 = 0;
        progress(0, total_size);

        while start < total_size {
            let end = chunk_end(start, total_size, RESUMABLE_CHUNK_SIZE);
            let len = (end - start + 1) as usize;

            file.seek(SeekFrom::Start(start)).await?;
            let mut buf = vec![0u8; len];
            file.read_exact(&mut buf).await?;
            // Bytes::clone is an O(1) refcount bump, so each retry is free
            // (vs. the 8 MiB heap copy a Vec<u8>::clone would cost).
            let body = Bytes::from(buf);

            let mut retry: u32 = 0;
            loop {
                match self
                    .put_chunk(&session_uri, start, end, total_size, body.clone())
                    .await
                {
                    Ok(ChunkOutcome::Done(id)) => {
                        progress(total_size, total_size);
                        crate::log!(
                            "[Backup] Upload complete: {} / {} bytes",
                            total_size,
                            total_size
                        );
                        return Ok(id);
                    }
                    Ok(ChunkOutcome::Continue(next)) => {
                        // Don't let the server move us backwards.
                        start = next.max(end + 1);
                        progress(start, total_size);
                        crate::log!("[Backup] Uploaded chunk: {} / {} bytes", start, total_size);
                        break;
                    }
                    Err(ChunkError::Fatal(e)) => return Err(e),
                    Err(ChunkError::Transient(msg)) => {
                        if retry >= MAX_RETRIES_PER_CHUNK {
                            return Err(format!(
                                "Drive chunk upload exhausted {} retries at byte {}: {}",
                                MAX_RETRIES_PER_CHUNK, start, msg
                            )
                            .into());
                        }
                        let backoff = backoff_secs(retry);
                        crate::log!(
                            "[Backup] Chunk failed at byte {} (retry {}/{} after {}s): {}",
                            start,
                            retry + 1,
                            MAX_RETRIES_PER_CHUNK,
                            backoff,
                            msg
                        );
                        tokio::time::sleep(Duration::from_secs(backoff)).await;
                        retry += 1;

                        // Ask the server where it is. If it has more bytes
                        // than we thought, jump forward and read the next chunk.
                        // If the query itself fails, fall through and retry the PUT.
                        match self.query_progress(&session_uri, total_size).await {
                            Ok(ChunkOutcome::Done(id)) => {
                                progress(total_size, total_size);
                                return Ok(id);
                            }
                            Ok(ChunkOutcome::Continue(server_next)) => {
                                if server_next > start {
                                    start = server_next;
                                    progress(start, total_size);
                                    crate::log!(
                                        "[Backup] Resuming at byte {} (server-side progress)",
                                        start
                                    );
                                    break;
                                }
                                // Server has same bytes as us — retry the same chunk.
                            }
                            Err(qe) => {
                                crate::log!(
                                    "[Backup] Progress query failed (will retry chunk): {}",
                                    qe
                                );
                            }
                        }
                    }
                }
            }
        }

        // Reached the end of the file without seeing a 200/201 — Drive returned
        // 308 on the final chunk. Ask once for the final state.
        match self.query_progress(&session_uri, total_size).await {
            Ok(ChunkOutcome::Done(id)) => {
                progress(total_size, total_size);
                Ok(id)
            }
            _ => Err("Resumable upload finished without success response".into()),
        }
    }
}

/// Map a Drive resumable-upload response into the loop's outcome / error type.
async fn classify_chunk_response(resp: reqwest::Response) -> Result<ChunkOutcome, ChunkError> {
    let status = resp.status();

    if status.is_success() {
        let created: DriveCreateResponse = resp.json().await.map_err(|e| {
            ChunkError::Fatal(format!("Drive success response parse: {}", e).into())
        })?;
        return Ok(ChunkOutcome::Done(created.id));
    }

    if status == StatusCode::PERMANENT_REDIRECT {
        // Resume Incomplete. Range header tells us the high-water mark.
        // Missing Range == server has nothing yet.
        let next = resp
            .headers()
            .get(reqwest::header::RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_range_next)
            .unwrap_or(0);
        return Ok(ChunkOutcome::Continue(next));
    }

    if status == StatusCode::UNAUTHORIZED {
        return Err(ChunkError::Fatal(DRIVE_AUTH_401_MSG.into()));
    }
    if status == StatusCode::FORBIDDEN {
        // Classify by the error body so an over-quota 403 (the production
        // failure) isn't mislabeled as an access problem during the upload.
        let body = resp.text().await.unwrap_or_default();
        return Err(ChunkError::Fatal(classify_403_body(&body).into()));
    }

    // 5xx / 408 / 429 are transient per Drive's resumable-upload guidance.
    if status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
    {
        let code = status.as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(ChunkError::Transient(format!("HTTP {}: {}", code, body)));
    }

    // Anything else (4xx) — give up.
    let code = status.as_u16();
    let body = resp.text().await.unwrap_or_default();
    Err(ChunkError::Fatal(
        format!("Drive upload failed with HTTP {}: {}", code, body).into(),
    ))
}

#[async_trait]
impl BackupProvider for GoogleDriveBackupProvider {
    fn name(&self) -> &str {
        "Google Drive"
    }

    fn id(&self) -> &str {
        "google_drive"
    }

    fn oauth_provider(&self) -> &str {
        "google"
    }

    async fn folder_url(&self) -> Option<String> {
        // Resolve the real folder id live — no persisted, workspace-global state.
        // Best-effort: with no token or no folder yet (before the first backup),
        // omit the link rather than point at a search the user didn't ask for.
        let token = self.get_token().await.ok()?;
        let folder_id = self.find_folder(&token).await.ok()??;
        Some(format!(
            "https://drive.google.com/drive/folders/{folder_id}"
        ))
    }

    async fn preflight(&self, estimated_upload_bytes: u64) -> Result<(), BoxError> {
        // Cheap, ordered checks — fail before any expensive work, and keep the
        // grant-access guidance reserved for an actual permission problem.
        // 1. Token: connect / refresh (get_token surfaces the connect/grant guidance).
        let token = self.get_token().await?;
        // 2. Required scope: the ONLY case that tells the user to Grant access.
        self.verify_scope(&token).await?;
        // 3. Free space: fail before pg_dump/compress/encrypt if it won't fit.
        self.check_free_space(&token, estimated_upload_bytes)
            .await?;
        // 4. Folder: verifies write access and warms the id cache for the upload.
        self.get_or_create_folder(&token).await?;
        Ok(())
    }

    async fn upload(
        &self,
        file_path: &Path,
        filename: &str,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<String, BoxError> {
        let token = self.get_token().await?;
        let folder_id = self.get_or_create_folder(&token).await?;

        let total_size = tokio::fs::metadata(file_path).await?.len();

        let metadata = serde_json::json!({
            "name": filename,
            "parents": [folder_id],
        });

        crate::log!(
            "[Backup] Starting resumable upload: {} ({} bytes, chunk {} bytes)",
            filename,
            total_size,
            RESUMABLE_CHUNK_SIZE
        );

        self.resumable_upload(file_path, total_size, &metadata, &token, progress)
            .await
    }

    async fn list_backups(&self) -> Result<Vec<BackupEntry>, BoxError> {
        let token = self.get_token().await?;
        let folder_id = self.get_or_create_folder(&token).await?;

        let query = format!("'{}' in parents and trashed = false", folder_id);
        let resp = check_drive_status(
            self.client
                .get(DRIVE_FILES_URL)
                .bearer_auth(&token)
                .query(&[
                    ("q", query.as_str()),
                    ("fields", "files(id,name,size,createdTime)"),
                    ("orderBy", "createdTime desc"),
                    ("pageSize", "100"),
                ])
                .send()
                .await?,
        )
        .await?;

        let list: DriveFileList = resp.json().await?;

        let mut entries = Vec::with_capacity(list.files.len());
        for file in list.files {
            let size_bytes = file
                .size
                .as_deref()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);

            let created_at = file
                .created_time
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);

            entries.push(BackupEntry {
                id: file.id,
                filename: file.name.unwrap_or_default(),
                size_bytes,
                created_at,
            });
        }

        Ok(entries)
    }

    async fn download(
        &self,
        backup_id: &str,
        dest: &Path,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), BoxError> {
        let token = self.get_token().await?;

        let url = format!("{}/{}?alt=media", DRIVE_FILES_URL, backup_id);
        let resp =
            check_drive_status(self.client.get(&url).bearer_auth(&token).send().await?).await?;

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
        let token = self.get_token().await?;

        let url = format!("{}/{}", DRIVE_FILES_URL, backup_id);
        check_drive_status(self.client.delete(&url).bearer_auth(&token).send().await?).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_range_next_handles_drive_format() {
        // Drive's documented format: "bytes=0-262143" → next byte is 262144.
        assert_eq!(parse_range_next("bytes=0-262143"), Some(262_144));
        assert_eq!(parse_range_next("bytes=0-0"), Some(1));
    }

    #[test]
    fn parse_range_next_lenient_with_whitespace_and_alt_form() {
        assert_eq!(parse_range_next("  bytes=0-99  "), Some(100));
        // Some clients use the bare "0-N" form; accept it.
        assert_eq!(parse_range_next("0-99"), Some(100));
        // RFC 7233 uses `bytes 0-99/total`; the spec for Drive omits the slash
        // in the range header but we should be defensive against "bytes "
        // (space) too.
        assert_eq!(parse_range_next("bytes 0-99"), Some(100));
    }

    #[test]
    fn parse_range_next_rejects_garbage() {
        assert_eq!(parse_range_next(""), None);
        assert_eq!(parse_range_next("bytes=abc-def"), None);
        assert_eq!(parse_range_next("bytes=0"), None);
    }

    #[test]
    fn backoff_progression_matches_documented_policy() {
        // 1, 2, 4, 8, 16, 32, capped at 32 thereafter.
        assert_eq!(backoff_secs(0), 1);
        assert_eq!(backoff_secs(1), 2);
        assert_eq!(backoff_secs(2), 4);
        assert_eq!(backoff_secs(3), 8);
        assert_eq!(backoff_secs(4), 16);
        assert_eq!(backoff_secs(5), 32);
        assert_eq!(backoff_secs(6), 32);
        assert_eq!(backoff_secs(100), 32);
    }

    #[test]
    fn chunk_end_full_chunk() {
        // 8 MiB chunk starting at 0 of a 100 MiB file → end at 8 MiB - 1.
        let chunk = 8 * 1024 * 1024;
        let total = 100 * 1024 * 1024;
        assert_eq!(chunk_end(0, total, chunk), chunk - 1);
        assert_eq!(chunk_end(chunk, total, chunk), 2 * chunk - 1);
    }

    #[test]
    fn chunk_end_final_partial_chunk() {
        // A 442 MB file's last chunk: 442 * 1024 * 1024 bytes, 8 MiB chunks.
        // 442 / 8 = 55.25 → 55 full chunks + one 0.25-chunk tail.
        let chunk: u64 = 8 * 1024 * 1024;
        let total: u64 = 442 * 1024 * 1024;
        let last_chunk_start = 55 * chunk;
        let end = chunk_end(last_chunk_start, total, chunk);
        // End is inclusive, so end = total - 1.
        assert_eq!(end, total - 1);
        // And the chunk length is the partial remainder.
        assert_eq!(end - last_chunk_start + 1, total - last_chunk_start);
    }

    #[test]
    fn chunk_end_handles_total_smaller_than_chunk_size() {
        // 1 MiB file, 8 MiB nominal chunk → single chunk covering the whole file.
        let chunk: u64 = 8 * 1024 * 1024;
        let total: u64 = 1024 * 1024;
        assert_eq!(chunk_end(0, total, chunk), total - 1);
    }

    // --- 403 classification (the mislabeling bug) ------------------------

    /// An over-quota 403 (Drive's v3 `error.errors[].reason == storageQuotaExceeded`)
    /// must map to the quota message — NOT the access-denied "Grant access" one
    /// that previously sent the user re-granting access while the Drive was full.
    #[test]
    fn classify_403_quota_body_returns_quota_message() {
        let body = r#"{"error":{"errors":[{"domain":"usageLimits","reason":"storageQuotaExceeded","message":"The user's Drive storage quota has been exceeded."}],"code":403,"message":"The user's Drive storage quota has been exceeded."}}"#;
        assert_eq!(classify_403_body(body), DRIVE_QUOTA_MSG);
        assert!(
            !classify_403_body(body).contains("Grant access"),
            "an over-quota 403 must never tell the user to grant access"
        );
    }

    /// A genuine auth-denied 403 (`insufficientPermissions`) keeps the existing
    /// grant-access message.
    #[test]
    fn classify_403_insufficient_permissions_returns_grant_message() {
        let body = r#"{"error":{"errors":[{"domain":"global","reason":"insufficientPermissions","message":"Insufficient Permission"}],"code":403,"message":"Insufficient Permission"}}"#;
        assert_eq!(classify_403_body(body), DRIVE_AUTH_403_MSG);
        assert!(classify_403_body(body).contains("Grant access"));
    }

    /// The legacy top-level `errors[]` shape and a plain-text "quota" body both
    /// classify as quota; an empty / hint-less body is a genuine access denial.
    #[test]
    fn classify_403_legacy_shape_and_text_fallback() {
        let legacy = r#"{"errors":[{"reason":"storageQuotaExceeded"}]}"#;
        assert_eq!(classify_403_body(legacy), DRIVE_QUOTA_MSG);
        assert_eq!(
            classify_403_body("403: The user's storage quota has been exceeded"),
            DRIVE_QUOTA_MSG
        );
        assert_eq!(classify_403_body(""), DRIVE_AUTH_403_MSG);
        // A non-quota reason embedded in otherwise-noisy JSON stays a grant case.
        assert_eq!(
            classify_403_body(r#"{"error":{"errors":[{"reason":"forbidden"}]}}"#),
            DRIVE_AUTH_403_MSG
        );
    }

    // --- preflight quota / scope decision core --------------------------

    /// Preflight passes when the Drive has room for the estimated upload.
    #[test]
    fn quota_check_passes_with_room() {
        let gib = 1024 * 1024 * 1024;
        assert!(quota_check(Some(100 * gib), 50 * gib, gib).is_ok());
    }

    /// The production case: 100 GiB limit, ~96.5 GiB used (~3.5 GiB free) and a
    /// 5 GiB upload → preflight fails with the over-quota message and never the
    /// grant-access one.
    #[test]
    fn quota_check_fails_when_free_below_estimate_with_quota_message() {
        let gib = 1024 * 1024 * 1024;
        let usage = (96.5 * gib as f64) as u64;
        let err = quota_check(Some(100 * gib), usage, 5 * gib).unwrap_err();
        assert!(err.contains("Google Drive is full"), "got: {err}");
        assert!(err.contains("free"), "should report free GB: {err}");
        assert!(
            !err.contains("Grant access"),
            "over-quota is not an access problem: {err}"
        );
    }

    /// The 10% headroom rejects an upload that would *just barely* fit raw, so
    /// estimate error can't push a real backup over the edge at upload time.
    #[test]
    fn quota_check_headroom_rejects_a_near_exact_fit() {
        // free = 1_000_000, estimate = 950_000 → needed = 1_045_000 > free.
        assert!(quota_check(Some(1_000_000), 0, 950_000).is_err());
    }

    /// An absent limit (some Workspace accounts report no quota) is unlimited.
    #[test]
    fn quota_check_unlimited_limit_always_passes() {
        assert!(quota_check(None, u64::MAX, u64::MAX / 2).is_ok());
    }

    /// Scope verification is a substring match against the space-separated
    /// granted scopes (mirrors `api::backup`'s readiness check). A token without
    /// the Drive scope fails — the case preflight maps to the re-grant message.
    #[test]
    fn scopes_include_substring_and_empty() {
        let granted = "https://www.googleapis.com/auth/drive.file https://www.googleapis.com/auth/userinfo.email";
        assert!(scopes_include(granted, "drive"));
        assert!(!scopes_include(
            "https://www.googleapis.com/auth/userinfo.email",
            "drive"
        ));
        // An empty requirement always passes (the Dropbox no-scope case).
        assert!(scopes_include("", ""));
    }
}
