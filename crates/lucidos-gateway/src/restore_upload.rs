//! Resumable restore uploads (ADR 0810): a subset of tus 1.0.
//!
//! A backup archive can be gigabytes, and a proxy in front of the gateway may
//! cap one request body far below that. So the client creates an upload
//! session, sends the archive as a run of `PATCH` chunks, and then commits the
//! restore by naming the session. Each session lives on disk as `upload.json`
//! plus `archive.part`. An upload therefore resumes from the gateway's offset
//! after a dropped connection, a reload or a gateway restart.
//!
//! The backup key never reaches this module. It travels in the commit body
//! only (`control::restore`).

use std::collections::HashSet;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{OriginalUri, Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncSeekExt as _, AsyncWriteExt as _};

use crate::error::ApiError;
use crate::server::GatewayState;

pub const TUS_VERSION: &str = "1.0.0";
pub const MAX_SESSIONS: usize = 4;
pub const EXPIRY: Duration = Duration::from_secs(24 * 3600);
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(3600);
/// Free space the create asks for, as a multiple of the archive: the staged
/// archive, the unpacked workspace, and the restored database.
const DISK_HEADROOM_FACTOR: u64 = 3;
const MAX_FILENAME_BYTES: usize = 255;
const RECORD_FILE: &str = "upload.json";
const PART: &str = "archive.part";
const COMMITTED_SUFFIX: &str = ".archive";

const TUS_RESUMABLE: HeaderName = HeaderName::from_static("tus-resumable");
const UPLOAD_LENGTH: HeaderName = HeaderName::from_static("upload-length");
const UPLOAD_OFFSET: HeaderName = HeaderName::from_static("upload-offset");
const UPLOAD_METADATA: HeaderName = HeaderName::from_static("upload-metadata");
const UPLOAD_CHECKSUM: HeaderName = HeaderName::from_static("upload-checksum");
const UPLOAD_EXPIRES: HeaderName = HeaderName::from_static("upload-expires");
const OFFSET_CONTENT_TYPE: &str = "application/offset+octet-stream";

/// The tus headers a client sends, which the control hop passes to B.
pub static TUS_REQUEST_HEADERS: [HeaderName; 5] = [
    TUS_RESUMABLE,
    UPLOAD_LENGTH,
    UPLOAD_OFFSET,
    UPLOAD_METADATA,
    UPLOAD_CHECKSUM,
];

/// The tus headers this module answers with, which the control hop passes
/// back. `Location` is not here: the hop rewrites it.
pub static TUS_RESPONSE_HEADERS: [HeaderName; 4] =
    [TUS_RESUMABLE, UPLOAD_LENGTH, UPLOAD_OFFSET, UPLOAD_EXPIRES];

/// tus checksum extension: the chunk's bytes do not match its checksum.
pub fn checksum_mismatch() -> StatusCode {
    StatusCode::from_u16(460).expect("460 is a valid status code")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadSessionRecord {
    pub length: u64,
    pub offset: u64,
    pub filename: String,
    /// Unix seconds of the create or the last chunk. Expiry counts from here.
    pub last_activity: u64,
}

impl UploadSessionRecord {
    pub fn is_complete(&self) -> bool {
        self.offset == self.length
    }

    fn expires_at(&self) -> u64 {
        self.last_activity + EXPIRY.as_secs()
    }

    fn expired(&self, now: u64) -> bool {
        now >= self.expires_at()
    }
}

/// The staged sessions under `<data dir>/restore-uploads/`, plus the set of
/// sessions a request holds right now. Only the set is in memory: it guards a
/// session against two concurrent writers and dies with the process.
pub struct RestoreUploads {
    root: PathBuf,
    busy: std::sync::Mutex<HashSet<String>>,
}

/// Exclusive hold on one session for the length of a request.
pub struct SessionClaim<'a> {
    uploads: &'a RestoreUploads,
    id: String,
    record: UploadSessionRecord,
}

impl Drop for SessionClaim<'_> {
    fn drop(&mut self) {
        if let Ok(mut busy) = self.uploads.busy.lock() {
            busy.remove(&self.id);
        }
    }
}

impl SessionClaim<'_> {
    pub fn record(&self) -> &UploadSessionRecord {
        &self.record
    }

    /// Move the complete archive out of the session and remove the session.
    /// The caller owns the returned file from here on.
    pub fn take(self) -> Result<PathBuf, ApiError> {
        let dest = self
            .uploads
            .root
            .join(format!("{}{COMMITTED_SUFFIX}", self.id));
        let dir = self.uploads.session_dir(&self.id);
        std::fs::rename(dir.join(PART), &dest)
            .map_err(|e| ApiError::internal(format!("stage the restore archive: {e}")))?;
        // The rename transferred ownership. Return the archive even if the
        // session metadata needs to be cleaned up by the next sweep.
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            crate::log_legacy!("[Gateway] could not remove committed upload session: {error}");
        }
        Ok(dest)
    }
}

pub fn restore_uploads_path(app_data: &Path) -> PathBuf {
    app_data.join("restore-uploads")
}

impl RestoreUploads {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            busy: std::sync::Mutex::default(),
        }
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    /// Start a session for an archive of `length` bytes. Refuses up front when
    /// four sessions are staged or the disk cannot hold the restore.
    pub fn create(&self, length: u64, filename: String, now: u64) -> Result<String, ApiError> {
        owner_only_dir(&self.root)
            .map_err(|e| ApiError::internal(format!("create the upload directory: {e}")))?;
        self.sweep(now);
        if self.live_sessions(now) >= MAX_SESSIONS {
            return Err(ApiError::too_many_requests(format!(
                "{MAX_SESSIONS} restore uploads are already staged on this gateway. \
                 Finish or cancel one first."
            )));
        }
        let needed = length.saturating_mul(DISK_HEADROOM_FACTOR);
        let free = fs2::available_space(&self.root)
            .map_err(|e| ApiError::internal(format!("read free disk space: {e}")))?;
        if free < needed {
            return Err(ApiError::with_status(
                StatusCode::INSUFFICIENT_STORAGE,
                format!(
                    "This gateway has {} free, and restoring this backup needs about {}.",
                    human_bytes(free),
                    human_bytes(needed)
                ),
            ));
        }
        let id = new_session_id().map_err(|e| ApiError::internal(format!("session id: {e}")))?;
        let dir = self.session_dir(&id);
        let staged = owner_only_dir(&dir).and_then(|()| owner_only_file(&dir.join(PART)));
        if let Err(e) = staged {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(ApiError::internal(format!(
                "create the upload session: {e}"
            )));
        }
        let record = UploadSessionRecord {
            length,
            offset: 0,
            filename,
            last_activity: now,
        };
        if let Err(e) = write_record(&dir, &record) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        crate::log_legacy!("[Gateway] restore upload {id} created ({length} bytes)");
        Ok(id)
    }

    /// Hold a live session for one request. A second concurrent request on the
    /// same session gets a 409, and an expired session reads as missing.
    pub fn claim(&self, id: &str, now: u64) -> Result<SessionClaim<'_>, ApiError> {
        if !valid_session_id(id) {
            return Err(not_found());
        }
        {
            let mut busy = self
                .busy
                .lock()
                .map_err(|_| ApiError::internal("restore upload state poisoned"))?;
            if !busy.insert(id.to_string()) {
                return Err(ApiError::conflict(
                    "Another request is writing to this upload. Retry from its offset.",
                ));
            }
        }
        // Built before the reads, so every early return below releases the hold.
        let mut held = SessionClaim {
            uploads: self,
            id: id.to_string(),
            record: UploadSessionRecord::default(),
        };
        held.record = read_record(&self.session_dir(id))?;
        if held.record.expired(now) {
            let _ = std::fs::remove_dir_all(self.session_dir(id));
            return Err(not_found());
        }
        Ok(held)
    }

    /// Hold a session whose every byte has arrived, for the restore commit.
    pub fn claim_complete(&self, id: &str, now: u64) -> Result<SessionClaim<'_>, ApiError> {
        let claim = self.claim(id, now)?;
        if !claim.record.is_complete() {
            return Err(ApiError::conflict(format!(
                "The upload is incomplete: {} of {} bytes arrived. Resume it before restoring.",
                claim.record.offset, claim.record.length
            )));
        }
        Ok(claim)
    }

    /// Append one chunk at `offset`. A wrong offset is a 409, a checksum
    /// mismatch a 460, and either way the session keeps exactly what it had.
    pub async fn append(
        &self,
        id: &str,
        offset: u64,
        checksum: Option<[u8; 32]>,
        body: Body,
        now: u64,
    ) -> Result<UploadSessionRecord, ApiError> {
        let mut claim = self.claim(id, now)?;
        let record = &mut claim.record;
        if offset != record.offset {
            return Err(ApiError::conflict(format!(
                "The upload is at byte {}, not {offset}. Resume from its offset.",
                record.offset
            )));
        }
        let dir = self.session_dir(id);
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(dir.join(PART))
            .await
            .map_err(|e| ApiError::internal(format!("open the staged archive: {e}")))?;
        let written = match write_chunk(&mut file, record, body).await {
            Ok((written, digest)) if checksum.is_none_or(|want| want == digest) => written,
            Ok(_) => {
                discard_after(&mut file, record.offset).await;
                return Err(ApiError::with_status(
                    checksum_mismatch(),
                    "The chunk does not match its checksum. Send it again.",
                ));
            }
            Err(e) => {
                discard_after(&mut file, record.offset).await;
                return Err(e);
            }
        };
        record.offset += written;
        record.last_activity = now;
        write_record(&dir, record)?;
        Ok(record.clone())
    }

    pub fn delete(&self, id: &str, now: u64) -> Result<(), ApiError> {
        let claim = self.claim(id, now)?;
        std::fs::remove_dir_all(self.session_dir(&claim.id))
            .map_err(|e| ApiError::internal(format!("remove the upload session: {e}")))?;
        crate::log_legacy!("[Gateway] restore upload {id} cancelled");
        Ok(())
    }

    fn live_sessions(&self, now: u64) -> usize {
        self.session_ids()
            .into_iter()
            .filter(|id| read_record(&self.session_dir(id)).is_ok_and(|m| !m.expired(now)))
            .count()
    }

    fn session_ids(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|id| valid_session_id(id))
            .collect()
    }

    fn is_busy(&self, id: &str) -> bool {
        self.busy.lock().map_or(true, |busy| busy.contains(id))
    }

    /// Remove expired sessions, and sessions with no readable record that
    /// sat untouched past the expiry. A session a request holds is kept.
    pub fn sweep(&self, now: u64) -> usize {
        let mut removed = 0;
        for id in self.session_ids() {
            if self.is_busy(&id) {
                continue;
            }
            let dir = self.session_dir(&id);
            let stale = match read_record(&dir) {
                Ok(record) => record.expired(now),
                Err(_) => modified_secs(&dir).is_none_or(|at| now >= at + EXPIRY.as_secs()),
            };
            if stale && std::fs::remove_dir_all(&dir).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    /// The startup sweep: expired sessions, plus any committed archive a
    /// restore left when the gateway stopped under it. No restore runs yet.
    pub fn sweep_on_start(&self, now: u64) -> usize {
        let mut removed = self.sweep(now);
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return removed;
        };
        for entry in entries.filter_map(Result::ok) {
            let committed = entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.ends_with(COMMITTED_SUFFIX));
            if committed && std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

async fn write_chunk(
    file: &mut tokio::fs::File,
    record: &UploadSessionRecord,
    body: Body,
) -> Result<(u64, [u8; 32]), ApiError> {
    file.set_len(record.offset)
        .await
        .map_err(|e| ApiError::internal(format!("trim the staged archive: {e}")))?;
    file.seek(std::io::SeekFrom::Start(record.offset))
        .await
        .map_err(|e| ApiError::internal(format!("seek the staged archive: {e}")))?;
    let room = record.length - record.offset;
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ApiError::bad_request(format!("upload read: {e}")))?;
        written += chunk.len() as u64;
        if written > room {
            return Err(ApiError::bad_request(format!(
                "The chunk runs past the upload's length of {} bytes.",
                record.length
            )));
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| ApiError::internal(format!("write the staged archive: {e}")))?;
    }
    file.sync_data()
        .await
        .map_err(|e| ApiError::internal(format!("sync the staged archive: {e}")))?;
    Ok((written, hasher.finalize().into()))
}

async fn discard_after(file: &mut tokio::fs::File, offset: u64) {
    if let Err(e) = file.set_len(offset).await {
        crate::log_legacy!("[Gateway] restore upload: could not trim a refused chunk: {e}");
    }
}

fn not_found() -> ApiError {
    ApiError::not_found("No such restore upload. It may have expired: start the upload again.")
}

fn valid_session_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn new_session_id() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn owner_only_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

fn owner_only_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map(drop)
}

fn read_record(dir: &Path) -> Result<UploadSessionRecord, ApiError> {
    let text = std::fs::read_to_string(dir.join(RECORD_FILE)).map_err(|_| not_found())?;
    serde_json::from_str(&text)
        .map_err(|e| ApiError::internal(format!("read the upload session: {e}")))
}

fn write_record(dir: &Path, record: &UploadSessionRecord) -> Result<(), ApiError> {
    use std::io::Write as _;
    let save = || -> std::io::Result<()> {
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        tmp.write_all(&serde_json::to_vec(record)?)?;
        tmp.as_file().sync_all()?;
        tmp.persist(dir.join(RECORD_FILE))?;
        Ok(())
    };
    save().map_err(|e| ApiError::internal(format!("save the upload session: {e}")))
}

fn modified_secs(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn human_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.1} GiB", b / GIB)
    } else {
        format!("{:.0} MiB", (b / MIB).ceil())
    }
}

fn http_date(unix_secs: u64) -> String {
    chrono::DateTime::from_timestamp(unix_secs as i64, 0)
        .unwrap_or_default()
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}

// ── HTTP surface ─────────────────────────────────────────────────────────────

fn header_str<'a>(headers: &'a HeaderMap, name: &HeaderName) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn require_tus(headers: &HeaderMap) -> Result<(), ApiError> {
    match header_str(headers, &TUS_RESUMABLE) {
        Some(TUS_VERSION) => Ok(()),
        _ => Err(ApiError::with_status(
            StatusCode::PRECONDITION_FAILED,
            format!("This gateway speaks tus {TUS_VERSION}. Send Tus-Resumable: {TUS_VERSION}."),
        )),
    }
}

fn parse_u64_header(headers: &HeaderMap, name: &HeaderName) -> Result<u64, ApiError> {
    header_str(headers, name)
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| ApiError::bad_request(format!("missing or malformed {name}")))
}

/// The `filename` pair of `Upload-Metadata` (`key base64value,...`), reduced to
/// its last path segment.
pub fn parse_filename(metadata: Option<&str>) -> Result<String, ApiError> {
    let Some(metadata) = metadata else {
        return Ok(String::new());
    };
    for pair in metadata.split(',') {
        let mut parts = pair.trim().splitn(2, ' ');
        if parts.next() != Some("filename") {
            continue;
        }
        let encoded = parts.next().unwrap_or("").trim();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| ApiError::bad_request("Upload-Metadata filename is not base64"))?;
        let name = String::from_utf8(bytes)
            .map_err(|_| ApiError::bad_request("Upload-Metadata filename is not UTF-8"))?;
        let base = name.rsplit(['/', '\\']).next().unwrap_or("").to_string();
        if base.len() > MAX_FILENAME_BYTES {
            return Err(ApiError::bad_request("The backup's filename is too long"));
        }
        return Ok(base);
    }
    Ok(String::new())
}

fn parse_checksum(headers: &HeaderMap) -> Result<Option<[u8; 32]>, ApiError> {
    let Some(value) = header_str(headers, &UPLOAD_CHECKSUM) else {
        return Ok(None);
    };
    let (algorithm, encoded) = value
        .trim()
        .split_once(' ')
        .ok_or_else(|| ApiError::bad_request("malformed Upload-Checksum"))?;
    if algorithm != "sha256" {
        return Err(ApiError::bad_request(format!(
            "Upload-Checksum algorithm {algorithm} is not supported. Use sha256."
        )));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| ApiError::bad_request("Upload-Checksum is not base64"))?;
    let digest: [u8; 32] = bytes
        .try_into()
        .map_err(|_| ApiError::bad_request("Upload-Checksum is not a SHA-256 digest"))?;
    Ok(Some(digest))
}

fn tus_headers(record: &UploadSessionRecord) -> [(HeaderName, HeaderValue); 4] {
    let value = |s: String| HeaderValue::from_str(&s).expect("ASCII header value");
    [
        (TUS_RESUMABLE, HeaderValue::from_static(TUS_VERSION)),
        (UPLOAD_OFFSET, value(record.offset.to_string())),
        (UPLOAD_LENGTH, value(record.length.to_string())),
        (UPLOAD_EXPIRES, value(http_date(record.expires_at()))),
    ]
}

/// Start a session. Its `Location` sits under the path the call came in on,
/// so it names the right route both on this gateway and through the hop.
pub async fn create(
    State(state): State<GatewayState>,
    OriginalUri(original): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_tus(&headers)?;
    let length = parse_u64_header(&headers, &UPLOAD_LENGTH)?;
    if length == 0 {
        return Err(ApiError::bad_request("The backup file is empty"));
    }
    let filename = parse_filename(header_str(&headers, &UPLOAD_METADATA))?;
    let id = state
        .restore_uploads()
        .create(length, filename, now_secs())?;
    let location = HeaderValue::from_str(&format!("{}/{id}", original.path()))
        .map_err(|_| ApiError::internal("the upload location is not a header value"))?;
    Ok((
        StatusCode::CREATED,
        [
            (axum::http::header::LOCATION, location),
            (TUS_RESUMABLE, HeaderValue::from_static(TUS_VERSION)),
        ],
    )
        .into_response())
}

pub async fn head(
    State(state): State<GatewayState>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_tus(&headers)?;
    let claim = state.restore_uploads().claim(&id, now_secs())?;
    Ok((
        StatusCode::OK,
        tus_headers(claim.record()),
        [(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        )],
    )
        .into_response())
}

pub async fn patch(
    State(state): State<GatewayState>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    require_tus(&headers)?;
    let content_type = header_str(&headers, &axum::http::header::CONTENT_TYPE);
    if content_type != Some(OFFSET_CONTENT_TYPE) {
        return Err(ApiError::with_status(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("A chunk is sent as {OFFSET_CONTENT_TYPE}"),
        ));
    }
    let offset = parse_u64_header(&headers, &UPLOAD_OFFSET)?;
    let checksum = parse_checksum(&headers)?;
    let record = state
        .restore_uploads()
        .append(&id, offset, checksum, body, now_secs())
        .await?;
    Ok((StatusCode::NO_CONTENT, tus_headers(&record)).into_response())
}

pub async fn delete(
    State(state): State<GatewayState>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_tus(&headers)?;
    state.restore_uploads().delete(&id, now_secs())?;
    Ok((
        StatusCode::NO_CONTENT,
        [(TUS_RESUMABLE, HeaderValue::from_static(TUS_VERSION))],
    )
        .into_response())
}

#[cfg(test)]
#[path = "restore_upload_tests.rs"]
mod tests;
