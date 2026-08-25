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

/// The scope substring a Drive backup needs, matched against the granted-scope
/// string (the fragment matches the full `.../auth/drive.file` URL). This IS the
/// provider registry's `required_scopes` entry, so the readiness verdict the
/// Settings page renders and the preflight below check one list.
pub const BACKUP_SCOPES: &[&str] = &["drive"];

/// The scope a user actually grants, as opposed to the matcher above. Naming
/// the matcher to a human would be naming a "drive permission" that appears in
/// no Google console; this is the string the authorization request carries and
/// the console lists. See [`super::name_missing_scopes`].
pub const GRANT_SCOPES: &[&str] = &["https://www.googleapis.com/auth/drive.file"];

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
    /// Scopes the token must carry, threaded from the provider registry's
    /// `required_scopes` so there's a single source of truth for them.
    required_scopes: &'static [&'static str],
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
    /// Present only when the caller asked for it: the resumable upload does,
    /// folder creation doesn't. Held untyped on purpose. Drive documents it as
    /// a decimal string. A typed field would turn any other shape into a parse
    /// error, failing an upload Drive had already finished.
    size: Option<serde_json::Value>,
}

/// A file Drive says it finished storing.
struct UploadedFile {
    id: String,
    /// Bytes Drive reports it holds, when it reported any.
    stored_size: Option<u64>,
}

/// Outcome of a single chunk PUT or a progress query.
enum ChunkOutcome {
    /// Upload finished, and Drive returned the file metadata.
    Done(UploadedFile),
    /// Resume Incomplete, carrying the high-water mark Drive reported. `None`
    /// means Drive reported nothing, which is unknown progress and never zero
    /// progress: reading it as zero is what skipped a whole chunk.
    Continue(Option<u64>),
}

/// Where the chunk loop goes after Drive answers 308.
#[derive(Debug, PartialEq, Eq)]
enum Resume {
    /// Carry on from this byte.
    At(u64),
    /// Drive gained no ground on the chunk we just sent. Send it again.
    Retry,
}

/// Tunables for the chunk loop. Production takes the defaults. Tests shrink the
/// chunk and drop the backoff, so a stalled session is exercised in
/// milliseconds instead of a minute of real sleeping.
struct ChunkLoopConfig {
    chunk_size: u64,
    backoff: fn(u32) -> Duration,
}

impl Default for ChunkLoopConfig {
    fn default() -> Self {
        Self {
            chunk_size: RESUMABLE_CHUNK_SIZE,
            backoff: |retry| Duration::from_secs(backoff_secs(retry)),
        }
    }
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

/// Parse Drive's `Range:` response header (`bytes=0-262143`) and return the
/// next byte to upload (one past the last received byte). A malformed header
/// yields `None`, which means unknown progress. It never means zero progress:
/// see [`resume_from`].
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

/// Read the high-water mark out of a 308 response's `Range` header. A missing
/// or malformed header is unknown progress, so it stays `None`.
fn continue_from_range(header: Option<&str>) -> ChunkOutcome {
    ChunkOutcome::Continue(header.and_then(parse_range_next))
}

/// Decide where the cursor goes after Drive answered 308 for the chunk
/// `start..=end`.
///
/// The Range header is the whole point of a 308: it says how much Drive
/// actually stored, which can be LESS than we sent. So resume from it whenever
/// it is ahead of us, and the gap in a partly-stored chunk goes again.
///
/// Two answers are not progress, and both re-send the same chunk. A mark at or
/// below `start` is stale, or says the chunk stored nothing. A `None` means
/// Drive told us nothing at all.
///
/// A mark beyond the chunk claims bytes we never sent, so it clamps to
/// `end + 1`. The cursor never passes a byte that we sent AND Drive confirmed.
fn resume_from(start: u64, end: u64, next: Option<u64>) -> Resume {
    match next {
        Some(n) if n > start => Resume::At(n.min(end.saturating_add(1))),
        _ => Resume::Retry,
    }
}

/// Why a chunk has to go again after a 308 that gained no ground.
fn no_progress_reason(next: Option<u64>) -> String {
    match next {
        Some(n) => format!("Drive is still at byte {}, so it stored nothing new", n),
        None => "Drive sent no Range header, so its progress is unknown".to_string(),
    }
}

/// Read the byte count out of Drive's `size` field. Absent, null, or a shape
/// we cannot read all mean unknown, which [`verify_stored_size`] lets through.
/// A number is accepted beside the documented decimal string, so a wider API
/// never fails a finished upload.
fn parse_stored_size(size: Option<&serde_json::Value>) -> Option<u64> {
    match size? {
        serde_json::Value::String(s) => s.parse().ok(),
        other => other.as_u64(),
    }
}

/// Compare the size Drive reports against the archive we uploaded. A short
/// store is silent data loss: the backup reads as fine until someone restores
/// it, so it fails here instead.
///
/// An ABSENT size passes, with a log line. We ask for the field by name, so
/// absence would mean Drive changed its API. Failing every backup on that
/// leaves the user with no backups at all. The chunk loop is the first defence
/// against a short upload; this is the second.
fn verify_stored_size(stored: Option<u64>, expected: u64) -> Result<(), String> {
    match stored {
        Some(n) if n != expected => Err(format!(
            "Google Drive stored {} of {} bytes. The backup is incomplete and was not recorded: \
             delete the partial file in Drive and run the backup again.",
            n, expected
        )),
        Some(_) => Ok(()),
        None => {
            crate::log!("[Backup] Drive reported no size for the upload, so it cannot be verified");
            Ok(())
        }
    }
}

/// Finish an upload Drive reported as done: verify it holds the whole archive,
/// then hand back the file id.
fn finish_upload(
    uploaded: UploadedFile,
    total_size: u64,
    progress: &(dyn Fn(u64, u64) + Send + Sync),
) -> Result<String, BoxError> {
    verify_stored_size(uploaded.stored_size, total_size)?;
    progress(total_size, total_size);
    crate::log!(
        "[Backup] Upload complete: {} / {} bytes",
        total_size,
        total_size
    );
    Ok(uploaded.id)
}

impl GoogleDriveBackupProvider {
    pub fn new(pool: PgPool, required_scopes: &'static [&'static str]) -> Self {
        Self {
            pool,
            client: reqwest::Client::new(),
            folder_id_cache: Arc::new(Mutex::new(None)),
            required_scopes,
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
        if super::missing_scopes(&granted, self.required_scopes).is_empty() {
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
        // `size` rides along with `id` so the completed upload can be checked
        // against the local archive (see `verify_stored_size`).
        let url = format!("{}?uploadType=resumable&fields=id,size", DRIVE_UPLOAD_URL);
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

    /// Drive's resumable upload protocol: open a session, then feed the file to
    /// [`upload_chunks`].
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

        upload_chunks(
            &self.client,
            &session_uri,
            file_path,
            total_size,
            &ChunkLoopConfig::default(),
            progress,
        )
        .await
    }
}

/// Send one PUT to the resumable session URI: either a chunk (with body) or a
/// progress query (empty body, `Content-Range: bytes */<total>`). Network
/// errors become `ChunkError::Transient`; the response is then classified by
/// `classify_chunk_response`.
async fn send_resumable_put(
    client: &reqwest::Client,
    session_uri: &str,
    content_range: String,
    body: Option<Bytes>,
) -> Result<ChunkOutcome, ChunkError> {
    let mut req = client
        .put(session_uri)
        .timeout(CHUNK_REQUEST_TIMEOUT)
        .header(reqwest::header::CONTENT_RANGE, content_range);

    req = match body {
        Some(b) => req.body(b),
        // reqwest sets Content-Length automatically for `Body`s but not for an
        // empty PUT, so be explicit and keep the request well-formed.
        None => req.header(reqwest::header::CONTENT_LENGTH, "0"),
    };

    match req.send().await {
        Ok(resp) => classify_chunk_response(resp).await,
        Err(e) => Err(ChunkError::Transient(format!("network: {}", e))),
    }
}

async fn put_chunk(
    client: &reqwest::Client,
    session_uri: &str,
    start: u64,
    end: u64,
    total: u64,
    body: Bytes,
) -> Result<ChunkOutcome, ChunkError> {
    send_resumable_put(
        client,
        session_uri,
        format!("bytes {}-{}/{}", start, end, total),
        Some(body),
    )
    .await
}

async fn query_progress(
    client: &reqwest::Client,
    session_uri: &str,
    total: u64,
) -> Result<ChunkOutcome, ChunkError> {
    send_resumable_put(client, session_uri, format!("bytes */{}", total), None).await
}

/// Feed the file to an open resumable session, chunk by chunk.
///
/// The cursor only ever moves to a byte Drive confirmed holding, so a chunk
/// Drive stored in part goes again for its gap. Anything that gains no ground
/// spends the same per-chunk retry budget: a transient failure, a stale Range,
/// no Range at all. A session that never advances then fails loudly, rather
/// than looping or skipping bytes.
///
/// See https://developers.google.com/workspace/drive/api/guides/manage-uploads#resumable
async fn upload_chunks(
    client: &reqwest::Client,
    session_uri: &str,
    file_path: &Path,
    total_size: u64,
    cfg: &ChunkLoopConfig,
    progress: &(dyn Fn(u64, u64) + Send + Sync),
) -> Result<String, BoxError> {
    let mut file = tokio::fs::File::open(file_path).await?;
    let mut start: u64 = 0;
    progress(0, total_size);

    while start < total_size {
        let end = chunk_end(start, total_size, cfg.chunk_size);
        let len = (end - start + 1) as usize;

        file.seek(SeekFrom::Start(start)).await?;
        let mut buf = vec![0u8; len];
        file.read_exact(&mut buf).await?;
        // Bytes::clone is an O(1) refcount bump, so each retry is free
        // (vs. the 8 MiB heap copy a Vec<u8>::clone would cost).
        let body = Bytes::from(buf);

        let mut retry: u32 = 0;
        loop {
            // Why this chunk has to go again, once we know it does.
            let stalled: String =
                match put_chunk(client, session_uri, start, end, total_size, body.clone()).await {
                    Ok(ChunkOutcome::Done(uploaded)) => {
                        return finish_upload(uploaded, total_size, progress)
                    }
                    Ok(ChunkOutcome::Continue(next)) => match resume_from(start, end, next) {
                        Resume::At(n) => {
                            start = n;
                            progress(start, total_size);
                            crate::log!(
                                "[Backup] Uploaded chunk: {} / {} bytes",
                                start,
                                total_size
                            );
                            break;
                        }
                        Resume::Retry => no_progress_reason(next),
                    },
                    Err(ChunkError::Fatal(e)) => return Err(e),
                    Err(ChunkError::Transient(msg)) => msg,
                };

            if retry >= MAX_RETRIES_PER_CHUNK {
                return Err(format!(
                    "Drive chunk upload exhausted {} retries at byte {}: {}",
                    MAX_RETRIES_PER_CHUNK, start, stalled
                )
                .into());
            }
            let backoff = (cfg.backoff)(retry);
            crate::log!(
                "[Backup] Chunk failed at byte {} (retry {}/{} after {:?}): {}",
                start,
                retry + 1,
                MAX_RETRIES_PER_CHUNK,
                backoff,
                stalled
            );
            tokio::time::sleep(backoff).await;
            retry += 1;

            // Ask the server where it is. If it is ahead of us, jump forward
            // and read the next chunk. If the query itself fails, fall through
            // and retry the PUT.
            match query_progress(client, session_uri, total_size).await {
                Ok(ChunkOutcome::Done(uploaded)) => {
                    return finish_upload(uploaded, total_size, progress)
                }
                Ok(ChunkOutcome::Continue(server_next)) => {
                    if let Resume::At(n) = resume_from(start, end, server_next) {
                        start = n;
                        progress(start, total_size);
                        crate::log!("[Backup] Resuming at byte {} (server-side progress)", start);
                        break;
                    }
                    // Server is no further along than we are: send it again.
                }
                Err(qe) => {
                    crate::log!("[Backup] Progress query failed (will retry chunk): {}", qe);
                }
            }
        }
    }

    // Reached the end of the file without seeing a 200/201, so Drive returned
    // 308 on the final chunk. Ask once for the final state.
    match query_progress(client, session_uri, total_size).await {
        Ok(ChunkOutcome::Done(uploaded)) => finish_upload(uploaded, total_size, progress),
        _ => Err("Resumable upload finished without success response".into()),
    }
}

/// Map a Drive resumable-upload response into the loop's outcome / error type.
async fn classify_chunk_response(resp: reqwest::Response) -> Result<ChunkOutcome, ChunkError> {
    let status = resp.status();

    if status.is_success() {
        let created: DriveCreateResponse = resp.json().await.map_err(|e| {
            ChunkError::Fatal(format!("Drive success response parse: {}", e).into())
        })?;
        return Ok(ChunkOutcome::Done(UploadedFile {
            id: created.id,
            stored_size: parse_stored_size(created.size.as_ref()),
        }));
    }

    if status == StatusCode::PERMANENT_REDIRECT {
        // Resume Incomplete. The Range header carries the high-water mark, and
        // its absence is unknown progress rather than none.
        let header = resp
            .headers()
            .get(reqwest::header::RANGE)
            .and_then(|v| v.to_str().ok());
        return Ok(continue_from_range(header));
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

    // --- cursor arithmetic (the truncated-archive bug) -------------------

    fn continued_at(header: Option<&str>) -> Option<u64> {
        match continue_from_range(header) {
            ChunkOutcome::Continue(n) => n,
            ChunkOutcome::Done(_) => panic!("a 308 is never Done"),
        }
    }

    /// The bug: Drive stored 40 of the 100 bytes we sent, and the old
    /// `next.max(end + 1)` jumped to 100. The 60-byte gap was never re-sent.
    #[test]
    fn resume_from_honours_a_partly_stored_chunk() {
        assert_eq!(resume_from(0, 99, Some(40)), Resume::At(40));
        assert_eq!(resume_from(500, 599, Some(512)), Resume::At(512));
    }

    /// A full store is the ordinary case, and the cursor lands past the chunk.
    #[test]
    fn resume_from_advances_on_a_full_store() {
        assert_eq!(resume_from(0, 99, Some(100)), Resume::At(100));
    }

    /// A mark past the chunk claims bytes we never sent, so it clamps.
    #[test]
    fn resume_from_clamps_a_mark_beyond_the_chunk() {
        assert_eq!(resume_from(0, 99, Some(5_000)), Resume::At(100));
    }

    /// No Range header means unknown progress, never zero progress. The old
    /// `.unwrap_or(0)` read it as zero, which then forced the cursor forward
    /// past the whole chunk.
    #[test]
    fn resume_from_treats_a_missing_mark_as_no_progress() {
        assert_eq!(resume_from(0, 99, None), Resume::Retry);
        assert_eq!(resume_from(500, 599, None), Resume::Retry);
    }

    /// The behavior `.max(end + 1)` was protecting: a stale mark below `start`
    /// must not move the cursor backwards. It is not progress either.
    #[test]
    fn resume_from_ignores_a_stale_mark() {
        assert_eq!(resume_from(500, 599, Some(20)), Resume::Retry);
        assert_eq!(resume_from(500, 599, Some(500)), Resume::Retry);
    }

    #[test]
    fn continue_from_range_carries_absence_as_unknown() {
        assert_eq!(continued_at(Some("bytes=0-99")), Some(100));
        assert_eq!(continued_at(None), None);
        assert_eq!(continued_at(Some("nonsense")), None);
    }

    #[test]
    fn no_progress_reason_names_both_stalls() {
        assert!(no_progress_reason(Some(42)).contains("42"));
        assert!(no_progress_reason(None).contains("no Range header"));
    }

    // --- stored-size check ------------------------------------------------

    /// A short store is the silent loss this check exists to catch.
    #[test]
    fn verify_stored_size_rejects_a_short_upload() {
        let err = verify_stored_size(Some(900), 1000).unwrap_err();
        assert!(err.contains("900"), "got: {err}");
        assert!(err.contains("1000"), "got: {err}");
        assert!(err.contains("run the backup again"), "got: {err}");
    }

    #[test]
    fn verify_stored_size_accepts_an_exact_match() {
        assert!(verify_stored_size(Some(1000), 1000).is_ok());
    }

    /// A size Drive reports that is too LARGE is just as wrong as a short one.
    #[test]
    fn verify_stored_size_rejects_a_long_upload() {
        assert!(verify_stored_size(Some(1200), 1000).is_err());
    }

    /// An absent size passes. Absence means Drive changed its API, since we ask
    /// for the field by name. Failing every backup on that would leave the user
    /// with none at all.
    #[test]
    fn verify_stored_size_passes_when_drive_reports_nothing() {
        assert!(verify_stored_size(None, 1000).is_ok());
    }

    /// The documented shape: an int64 as a decimal string.
    #[test]
    fn stored_size_reads_drives_decimal_string() {
        let created: DriveCreateResponse =
            serde_json::from_str(r#"{"id":"f","size":"4096"}"#).unwrap();
        assert_eq!(parse_stored_size(created.size.as_ref()), Some(4096));
    }

    /// A number instead of a string must not fail the response. It used to:
    /// the field was typed `Option<String>`, so serde rejected the whole body
    /// and a finished upload came back as a parse error.
    #[test]
    fn stored_size_survives_a_number_from_a_wider_api() {
        let created: DriveCreateResponse =
            serde_json::from_str(r#"{"id":"f","size":4096}"#).unwrap();
        assert_eq!(parse_stored_size(created.size.as_ref()), Some(4096));
    }

    /// Absent, null, and unreadable all mean unknown, never zero.
    #[test]
    fn stored_size_reads_every_other_shape_as_unknown() {
        for body in [
            r#"{"id":"f"}"#,
            r#"{"id":"f","size":null}"#,
            r#"{"id":"f","size":"not a number"}"#,
            r#"{"id":"f","size":{"value":10}}"#,
        ] {
            let created: DriveCreateResponse = serde_json::from_str(body).unwrap();
            assert_eq!(parse_stored_size(created.size.as_ref()), None, "{body}");
        }
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

    /// The registry entry Drive's preflight verifies against. A backup writes
    /// with `drive.file`, which the fragment matches.
    #[test]
    fn backup_scopes_match_the_drive_file_grant() {
        let granted = "https://www.googleapis.com/auth/drive.file https://www.googleapis.com/auth/userinfo.email";
        assert!(super::super::missing_scopes(granted, BACKUP_SCOPES).is_empty());
        assert_eq!(
            super::super::missing_scopes(
                "https://www.googleapis.com/auth/userinfo.email",
                BACKUP_SCOPES
            ),
            vec!["drive"]
        );
    }
}

/// End-to-end tests for [`upload_chunks`] against a mock Drive on loopback.
///
/// The mock is strict where the real thing is strict: a chunk whose start runs
/// past what it holds is a 400, because those bytes are gone. That is what
/// turns the cursor bug from a quiet truncation into a failing test.
#[cfg(test)]
mod chunk_loop_tests {
    use super::*;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::response::Response;

    const FILE_ID: &str = "mock-file-id";

    /// What the mock does with one chunk PUT.
    #[derive(Clone, Copy)]
    enum Reply {
        /// Store the whole chunk and report it honestly.
        Store,
        /// Store the first `n` bytes only, and report that honestly. This is
        /// the case the 308 Range header exists to signal.
        Partial(usize),
        /// Store nothing, and send no Range header at all.
        Dropped,
        /// Store the whole chunk, but report a stale mark at byte `n`.
        Stale(u64),
        /// Store nothing, and report the mark it already had.
        NoProgress,
    }

    struct MockDrive {
        stored: Vec<u8>,
        total: u64,
        /// Scripted replies, consumed in order. `fallback` covers the rest.
        script: Vec<Reply>,
        fallback: Reply,
        /// Start offset of every chunk PUT, in order.
        offsets: Vec<u64>,
        /// Size the completion response reports, when it should lie.
        size_override: Option<u64>,
        /// Set when the client asked the mock to store bytes it never received.
        gap: Option<String>,
    }

    impl MockDrive {
        fn new(total: u64) -> Self {
            Self {
                stored: Vec::new(),
                total,
                script: Vec::new(),
                fallback: Reply::Store,
                offsets: Vec::new(),
                size_override: None,
                gap: None,
            }
        }

        fn script(mut self, script: Vec<Reply>) -> Self {
            self.script = script;
            self
        }

        fn fallback(mut self, reply: Reply) -> Self {
            self.fallback = reply;
            self
        }

        fn reports_size(mut self, size: u64) -> Self {
            self.size_override = Some(size);
            self
        }

        fn shared(self) -> Arc<Mutex<Self>> {
            Arc::new(Mutex::new(self))
        }

        fn next_reply(&mut self) -> Reply {
            if self.script.is_empty() {
                self.fallback
            } else {
                self.script.remove(0)
            }
        }

        fn done(&self) -> Response {
            let size = self.size_override.unwrap_or(self.stored.len() as u64);
            Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(format!(
                    r#"{{"id":"{}","size":"{}"}}"#,
                    FILE_ID, size
                )))
                .unwrap()
        }

        fn complete(&self) -> bool {
            self.stored.len() as u64 == self.total
        }
    }

    /// A 308, carrying the next byte Drive wants. Real Drive sends no Range
    /// header when it holds nothing, so neither does this.
    fn incomplete(next: Option<u64>) -> Response {
        let mut builder = Response::builder().status(308);
        if let Some(n) = next.filter(|n| *n > 0) {
            builder = builder.header("range", format!("bytes=0-{}", n - 1));
        }
        builder.body(axum::body::Body::empty()).unwrap()
    }

    fn bad_request() -> Response {
        Response::builder()
            .status(400)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    /// Split `bytes <start>-<end>/<total>` into its start and end.
    fn parse_content_range(header: &str) -> Option<(u64, u64)> {
        let (range, _) = header.strip_prefix("bytes ")?.split_once('/')?;
        let (start, end) = range.split_once('-')?;
        Some((start.parse().ok()?, end.parse().ok()?))
    }

    fn write_at(buf: &mut Vec<u8>, start: usize, bytes: &[u8]) {
        let end = start + bytes.len();
        if buf.len() < end {
            buf.resize(end, 0);
        }
        buf[start..end].copy_from_slice(bytes);
    }

    async fn drive_handler(
        State(state): State<Arc<Mutex<MockDrive>>>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let mut drive = state.lock().await;
        let header = headers
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();

        // `bytes */<total>` is a progress query, not a chunk.
        if header.starts_with("bytes */") {
            return if drive.complete() {
                drive.done()
            } else {
                incomplete(Some(drive.stored.len() as u64))
            };
        }

        let Some((start, end)) = parse_content_range(&header) else {
            drive.gap = Some(format!("malformed Content-Range: {}", header));
            return bad_request();
        };
        drive.offsets.push(start);

        if start > drive.stored.len() as u64 {
            drive.gap = Some(format!(
                "chunk starts at byte {} but only {} bytes are stored",
                start,
                drive.stored.len()
            ));
            return bad_request();
        }
        if body.len() as u64 != end - start + 1 {
            drive.gap = Some(format!("body of {} bytes for {}", body.len(), header));
            return bad_request();
        }

        let reply = drive.next_reply();
        let take = match reply {
            Reply::Store | Reply::Stale(_) => body.len(),
            Reply::Partial(n) => n.min(body.len()),
            Reply::Dropped | Reply::NoProgress => 0,
        };
        write_at(&mut drive.stored, start as usize, &body[..take]);

        match reply {
            Reply::Stale(n) => incomplete(Some(n)),
            Reply::Dropped => incomplete(None),
            _ if drive.complete() => drive.done(),
            _ => incomplete(Some(drive.stored.len() as u64)),
        }
    }

    /// Boot the mock on a loopback port and hand back its session URI.
    async fn spawn_mock(drive: Arc<Mutex<MockDrive>>) -> String {
        let app = axum::Router::new()
            .route("/session", axum::routing::put(drive_handler))
            .with_state(drive);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{}/session", addr)
    }

    /// A byte pattern with a prime period, so a misplaced chunk cannot happen
    /// to match the bytes it displaced.
    fn source_bytes(total: usize) -> Vec<u8> {
        (0..total).map(|i| (i % 251) as u8).collect()
    }

    /// Small chunks and no backoff, so a stalled session runs in milliseconds.
    fn test_cfg(chunk_size: u64) -> ChunkLoopConfig {
        ChunkLoopConfig {
            chunk_size,
            backoff: |_| Duration::ZERO,
        }
    }

    const TOTAL: usize = 300;
    const CHUNK: u64 = 100;

    struct Fixture {
        drive: Arc<Mutex<MockDrive>>,
        uri: String,
        path: std::path::PathBuf,
        source: Vec<u8>,
        _dir: tempfile::TempDir,
    }

    async fn fixture(drive: MockDrive) -> Fixture {
        let source = source_bytes(TOTAL);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.bin");
        std::fs::write(&path, &source).unwrap();
        let drive = drive.shared();
        let uri = spawn_mock(drive.clone()).await;
        Fixture {
            drive,
            uri,
            path,
            source,
            _dir: dir,
        }
    }

    impl Fixture {
        async fn run(&self) -> Result<String, BoxError> {
            upload_chunks(
                &reqwest::Client::new(),
                &self.uri,
                &self.path,
                TOTAL as u64,
                &test_cfg(CHUNK),
                &|_, _| {},
            )
            .await
        }

        /// Every byte arrived, in order, and the mock was never asked to skip.
        async fn assert_intact(&self) {
            let drive = self.drive.lock().await;
            assert_eq!(drive.gap, None, "the loop skipped bytes");
            assert_eq!(drive.stored, self.source, "Drive holds a different archive");
            let mut sorted = drive.offsets.clone();
            sorted.sort_unstable();
            assert_eq!(
                drive.offsets, sorted,
                "the cursor moved backwards: {:?}",
                drive.offsets
            );
        }
    }

    /// The baseline: nothing goes wrong, and the whole archive lands.
    #[tokio::test]
    async fn a_clean_run_uploads_every_byte() {
        let f = fixture(MockDrive::new(TOTAL as u64)).await;
        assert_eq!(f.run().await.unwrap(), FILE_ID);
        f.assert_intact().await;
    }

    /// Case 1: Drive stored 40 of the 100 bytes we sent. The old cursor jumped
    /// to 100 and left a 60-byte hole in the archive.
    #[tokio::test]
    async fn a_partly_stored_chunk_re_sends_its_gap() {
        let f = fixture(MockDrive::new(TOTAL as u64).script(vec![Reply::Partial(40)])).await;
        assert_eq!(f.run().await.unwrap(), FILE_ID);
        f.assert_intact().await;
    }

    /// Case 2: Drive reports no Range header at all. The old `.unwrap_or(0)`
    /// read that as byte 0, which then forced the cursor past the whole chunk.
    #[tokio::test]
    async fn a_missing_range_header_re_sends_the_chunk() {
        let f = fixture(MockDrive::new(TOTAL as u64).script(vec![Reply::Dropped])).await;
        assert_eq!(f.run().await.unwrap(), FILE_ID);
        f.assert_intact().await;
    }

    /// Case 3: a stale mark below `start`. This is the case the old
    /// `.max(end + 1)` was protecting, and it must keep working: the cursor
    /// never follows a mark backwards.
    #[tokio::test]
    async fn a_stale_range_never_moves_the_cursor_back() {
        let f = fixture(MockDrive::new(TOTAL as u64).script(vec![Reply::Store, Reply::Stale(10)]))
            .await;
        assert_eq!(f.run().await.unwrap(), FILE_ID);
        f.assert_intact().await;
    }

    /// Re-sending a chunk spends retry budget, so a session that never gains
    /// ground fails loudly instead of looping.
    #[tokio::test]
    async fn a_session_that_never_advances_gives_up() {
        let f = fixture(MockDrive::new(TOTAL as u64).fallback(Reply::NoProgress)).await;
        let err = f.run().await.unwrap_err().to_string();
        assert!(err.contains("exhausted"), "got: {err}");
        assert!(err.contains("byte 0"), "got: {err}");
    }

    /// The last line of defence: Drive says it finished, but holds less than
    /// the archive. That is never recorded as a successful backup.
    #[tokio::test]
    async fn a_short_stored_size_fails_the_upload() {
        let f = fixture(MockDrive::new(TOTAL as u64).reports_size(299)).await;
        let err = f.run().await.unwrap_err().to_string();
        assert!(err.contains("stored 299 of 300 bytes"), "got: {err}");
        assert!(err.contains("was not recorded"), "got: {err}");
    }
}
