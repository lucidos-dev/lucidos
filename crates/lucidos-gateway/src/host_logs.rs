//! Host-owned, bounded raw-log snapshots (ADR 0712). No contents cross this API.
//!
//! Before consent only metadata is inspected. Consent names each inode and
//! its maximum prefix length, captured at execution time, not a content hash.
//! An active capture may append; the approved prefix never grows. Replacement,
//! truncation, hard links and directory indirection fail closed.

use crate::registry::CreatedBy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

type Result<T> = std::result::Result<T, crate::BoxError>;
const MAX_BYTES: u64 = 128 * 1024 * 1024;
const MAX_OPERATIONS: usize = 8;
const MAX_DIRECTORY_ENTRIES: usize = 4096;
const TTL_SECONDS: u64 = 15 * 60;
const NO_CAPTURE: &str = "No retained file-backed host captures are available; journal-only and foreground output are unsupported";
const TOO_LARGE: &str = "Host-log snapshot exceeds the 128 MiB limit; nothing transferred";
const UNKNOWN_ROTATION: &str = "Unrecognized host-log rotation; no files selected";
const STORE_FULL: &str =
    "Host-log operation storage is full; wait for a pending operation to expire after 15 minutes";
const BASES: &[&str] = &[
    "gateway.log",
    "logs/gateway.out.log",
    "logs/gateway.err.log",
    "logs/engine-service.out.log",
    "logs/engine-service.err.log",
];

pub(crate) fn router() -> axum::Router<crate::server::GatewayState> {
    use axum::routing::post;
    axum::Router::new()
        .route("/prepare", post(prepare_request))
        .route("/:id/status", post(status_request))
        .route("/:id/transfer", post(transfer_request))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareBody {
    caller: CreatedBy,
    recipient: Recipient,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallerBody {
    caller: CreatedBy,
}

fn safe_failure() -> crate::error::ApiError {
    // Neither I/O nor serde errors may expose data from a corrupt private
    // record. Transport errors will follow the same metadata-only contract.
    crate::error::ApiError::conflict("Host-log operation unavailable or refused. Check capture availability, source identity, limits and operation status; no automatic retry is safe.")
}

fn operation_failure(error: crate::BoxError) -> crate::error::ApiError {
    // Only exact host-authored sentences cross this boundary. In particular,
    // serde errors can include bytes from a damaged operation record.
    match error.to_string().as_str() {
        NO_CAPTURE => crate::error::ApiError::conflict(NO_CAPTURE),
        TOO_LARGE => crate::error::ApiError::conflict(TOO_LARGE),
        UNKNOWN_ROTATION => crate::error::ApiError::conflict(UNKNOWN_ROTATION),
        STORE_FULL => crate::error::ApiError::too_many_requests(STORE_FULL),
        _ => safe_failure(),
    }
}

async fn prepare_request(
    axum::extract::State(state): axum::extract::State<crate::server::GatewayState>,
    local: Option<axum::Extension<crate::auth::AuthenticatedLocalProcess>>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<PrepareBody>,
) -> std::result::Result<axum::Json<Operation>, crate::error::ApiError> {
    crate::broker::local_engine(local, &headers)?;
    crate::broker::valid_caller(&body.caller)?;
    body.recipient.validate().map_err(|_| {
        crate::error::ApiError::bad_request("Invalid SSH recipient or relative directory")
    })?;
    let root = state.app_data().clone();
    blocking(move || prepare(&installation_root(&root)?, body.caller, body.recipient))
        .await
        .map(axum::Json)
        .map_err(operation_failure)
}

async fn status_request(
    axum::extract::State(state): axum::extract::State<crate::server::GatewayState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    local: Option<axum::Extension<crate::auth::AuthenticatedLocalProcess>>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<CallerBody>,
) -> std::result::Result<axum::Json<Operation>, crate::error::ApiError> {
    crate::broker::local_engine(local, &headers)?;
    crate::broker::valid_caller(&body.caller)?;
    let root = state.app_data().clone();
    blocking(move || recover_status(&installation_root(&root)?, &id, &body.caller))
        .await
        .map(axum::Json)
        .map_err(operation_failure)
}

async fn transfer_request(
    axum::extract::State(state): axum::extract::State<crate::server::GatewayState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    local: Option<axum::Extension<crate::auth::AuthenticatedLocalProcess>>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<TransferBody>,
) -> std::result::Result<axum::Json<Operation>, crate::error::ApiError> {
    crate::broker::local_engine(local, &headers)?;
    crate::broker::valid_caller(&body.caller)?;
    let root = state.app_data().clone();
    transfer(root, id, body.caller, body.approval)
        .await
        .map(axum::Json)
        .map_err(operation_failure)
}

/// Runs file work and file locks off the async runtime, so a large capture
/// never stalls the requests queued on a gateway worker.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work).await?
}

/// The data directory is trusted, so its own path may cross symlinks (a
/// symlinked home, say). Every path below it is opened without following one.
fn installation_root(root: &Path) -> Result<PathBuf> {
    Ok(root.canonicalize()?)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferBody {
    caller: CreatedBy,
    approval: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Recipient {
    pub user: String,
    pub host: String,
    /// Relative to the SSH account's home; no shell expansion is needed.
    pub directory: String,
}

impl Recipient {
    pub(crate) fn validate(&self) -> Result<()> {
        fn atom(value: &str, max: usize, extra: &[u8]) -> bool {
            !value.is_empty()
                && value.len() <= max
                && value.as_bytes()[0].is_ascii_alphanumeric()
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
        }
        if !atom(&self.user, 64, b"_-")
            || !(atom(&self.host, 253, b".-") || self.host.parse::<std::net::Ipv6Addr>().is_ok())
            || self.directory.len() > 512
            || !self.directory.split('/').all(|p| atom(p, 128, b"._-"))
        {
            return Err("Use an SSH user, DNS name or IP address, and a simple relative destination directory".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Source {
    pub path: String,
    pub bytes: u64,
    pub device: u64,
    pub inode: u64,
    pub sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Status {
    Prepared,
    Capturing,
    Snapshotted,
    /// A restart in this state means uncertain remote outcome; never retry.
    Transferring,
    Verified,
    Uncertain,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Operation {
    pub id: String,
    pub approval: String,
    pub caller: CreatedBy,
    pub installation: PathBuf,
    pub installation_device: u64,
    pub installation_inode: u64,
    pub recipient: Recipient,
    pub expires_at: u64,
    pub files: Vec<Source>,
    pub state: Status,
    pub detail: Option<String>,
    pub warning: String,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Traverse every absolute directory component without following symlinks.
/// Holding this descriptor also pins the parent against rename races.
fn directory(path: &Path) -> Result<File> {
    if !path.is_absolute() {
        return Err("Host capture directory is not absolute".into());
    }
    let mut current = File::open("/")?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                current = at(
                    &current,
                    name.as_bytes(),
                    libc::O_RDONLY | libc::O_DIRECTORY,
                )?
            }
            _ => return Err("Host capture directory contains traversal".into()),
        }
    }
    Ok(current)
}

fn at(parent: &File, name: &[u8], flags: i32) -> io::Result<File> {
    let name = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: live directory descriptor and NUL-terminated name; ownership of
    // the returned descriptor moves exactly once into File.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn source(root: &File, name: &str) -> Result<File> {
    let mut parts = name.split('/');
    let first = parts.next().ok_or("Missing source name")?;
    let file = match parts.next() {
        Some(last) => {
            let logs = at(root, first.as_bytes(), libc::O_RDONLY | libc::O_DIRECTORY)?;
            at(&logs, last.as_bytes(), libc::O_RDONLY)?
        }
        None => at(root, first.as_bytes(), libc::O_RDONLY)?,
    };
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 || meta.uid() != unsafe { libc::geteuid() } {
        return Err(
            "Host capture must be a singly linked regular file owned by this host user".into(),
        );
    }
    Ok(file)
}

fn recognized(name: &str) -> bool {
    BASES.iter().any(|base| {
        name == *base
            || (1..=9).any(|n| name == format!("{base}.{n}") || name == format!("{base}.{n}.gz"))
    })
}

fn scan(root: &Path) -> Result<Vec<Source>> {
    let pinned = directory(root)?;
    // Bounded enumeration detects unsupported rotations instead of silently
    // presenting a partial file set as complete. No file contents are read.
    for folder in [root.to_path_buf(), root.join("logs")] {
        match directory(&folder) {
            Ok(_) => {}
            Err(e)
                if e.downcast_ref::<io::Error>()
                    .is_some_and(|e| e.kind() == io::ErrorKind::NotFound) =>
            {
                continue
            }
            Err(e) => return Err(e),
        }
        for (count, entry) in std::fs::read_dir(&folder)?.enumerate() {
            if count >= MAX_DIRECTORY_ENTRIES {
                return Err("Host capture directory has too many entries".into());
            }
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let relative = if folder == root {
                name.into_owned()
            } else {
                format!("logs/{name}")
            };
            if BASES
                .iter()
                .any(|base| relative.starts_with(&format!("{base}.")))
                && !recognized(&relative)
            {
                return Err(UNKNOWN_ROTATION.into());
            }
        }
    }
    let mut files = Vec::new();
    let mut bytes = 0u64;
    for base in BASES {
        let mut names = vec![base.to_string()];
        for n in 1..=9 {
            names.extend([format!("{base}.{n}"), format!("{base}.{n}.gz")]);
        }
        for name in names {
            let file = match source(&pinned, &name) {
                Ok(file) => file,
                Err(e)
                    if e.downcast_ref::<io::Error>()
                        .is_some_and(|e| e.kind() == io::ErrorKind::NotFound) =>
                {
                    continue
                }
                Err(e) => return Err(e),
            };
            let meta = file.metadata()?;
            bytes = bytes
                .checked_add(meta.len())
                .ok_or("Capture size overflow")?;
            if bytes > MAX_BYTES {
                return Err(TOO_LARGE.into());
            }
            files.push(Source {
                path: name,
                bytes: meta.len(),
                device: meta.dev(),
                inode: meta.ino(),
                sha256: None,
            });
        }
    }
    if files.is_empty() {
        return Err(NO_CAPTURE.into());
    }
    Ok(files)
}

fn store(root: &Path) -> Result<PathBuf> {
    directory(root)?;
    let path = root.join("host-log-transfers");
    match std::fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    let meta = directory(&path)?.metadata()?;
    if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o777 != 0o700 {
        return Err("Host-log operation store is not private".into());
    }
    Ok(path)
}

fn lock(root: &Path) -> Result<File> {
    let store = directory(&store(root)?)?;
    let lock = at(&store, b"lock", libc::O_RDWR | libc::O_CREAT)?;
    if !lock.metadata()?.is_file() || lock.metadata()?.nlink() != 1 {
        return Err("Unsafe operation lock".into());
    }
    fs2::FileExt::lock_exclusive(&lock)?;
    Ok(lock)
}

fn operation_dir(root: &Path, id: &str) -> Result<PathBuf> {
    if !id.starts_with("transfer-")
        || id.len() > 64
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("Invalid host-log operation".into());
    }
    Ok(store(root)?.join(id))
}

fn save(root: &Path, operation: &Operation) -> Result<()> {
    let path = operation_dir(root, &operation.id)?;
    directory(&path)?;
    crate::atomic_file::write(
        &path.join("operation.json"),
        &serde_json::to_vec(operation)?,
    )?;
    File::open(&path)?.sync_all()?;
    Ok(())
}

pub(crate) fn prepare(root: &Path, caller: CreatedBy, recipient: Recipient) -> Result<Operation> {
    recipient.validate()?;
    let _lock = lock(root)?;
    let files = scan(root)?;
    let storage = store(root)?;
    if prune(root, &storage)? >= MAX_OPERATIONS {
        return Err(STORE_FULL.into());
    }
    let dir = tempfile::Builder::new()
        .prefix("transfer-")
        .tempdir_in(&storage)?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    let id = dir
        .path()
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("Invalid operation directory")?
        .to_string();
    let meta = directory(root)?.metadata()?;
    let mut operation = Operation {
        id, approval: String::new(), caller, recipient, files, installation: root.to_path_buf(),
        installation_device: meta.dev(), installation_inode: meta.ino(),
        expires_at: now() + TTL_SECONDS, state: Status::Prepared, detail: None,
        warning: "Raw host logs may contain other workspaces' text and secrets. Only the listed inode prefixes, at most the listed bytes, will be captured at execution time after approval. History already deleted or truncated cannot be recovered. Rotations supported: .1 through .9, optionally .gz.".into(),
    };
    operation.approval = consent_digest(&operation)?;
    save(root, &operation)?;
    let retained = dir.keep();
    File::open(retained)?.sync_all()?;
    File::open(&storage)?.sync_all()?;
    Ok(operation)
}

/// Removes every operation that can no longer act: past its expiry, with no
/// transfer holding its active lock. Returns how many operations remain.
/// Callers hold the store lock, so no snapshot can start meanwhile.
fn prune(root: &Path, storage: &Path) -> Result<usize> {
    let mut remaining = 0;
    for (count, entry) in std::fs::read_dir(storage)?.enumerate() {
        if count >= MAX_DIRECTORY_ENTRIES {
            return Err("Host-log operation store has too many entries".into());
        }
        let name = entry?.file_name();
        let Some(id) = name.to_str().filter(|n| n.starts_with("transfer-")) else {
            continue;
        };
        // A record this host cannot read is not its to judge; it stays.
        if !read(root, id).is_ok_and(|operation| now() > operation.expires_at) {
            remaining += 1;
            continue;
        }
        let active = active_lock(root, id)?;
        match fs2::FileExt::try_lock_exclusive(&active) {
            Ok(()) => std::fs::remove_dir_all(operation_dir(root, id)?)?,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => remaining += 1,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(remaining)
}

/// An operation record, bound to its id but to no caller.
fn read(root: &Path, id: &str) -> Result<Operation> {
    let dir = directory(&operation_dir(root, id)?)?;
    let file = at(&dir, b"operation.json", libc::O_RDONLY)?;
    if !file.metadata()?.is_file() || file.metadata()?.nlink() != 1 {
        return Err("Invalid operation record".into());
    }
    let operation: Operation = serde_json::from_reader(file.take(64 * 1024))?;
    if operation.id != id {
        return Err("Invalid operation record".into());
    }
    Ok(operation)
}

pub(crate) fn load(root: &Path, id: &str, caller: &CreatedBy) -> Result<Operation> {
    let operation = read(root, id)?;
    let meta = directory(root)?.metadata()?;
    if &operation.caller != caller
        || operation.installation != root
        || operation.installation_device != meta.dev()
        || operation.installation_inode != meta.ino()
    {
        return Err("Host-log operation belongs to another caller or installation".into());
    }
    if operation.approval != consent_digest(&operation)? {
        return Err("Operation manifest changed".into());
    }
    Ok(operation)
}

/// Called only by the local engine broker after it spends the exact once grant.
/// Capturing is persisted before the first read; a crash is never replayable.
pub(crate) fn snapshot(root: &Path, id: &str, caller: &CreatedBy) -> Result<Operation> {
    let _lock = lock(root)?;
    let mut operation = load(root, id, caller)?;
    if operation.state != Status::Prepared || now() > operation.expires_at {
        return Err("Host-log operation expired or already spent; it cannot be retried".into());
    }
    operation.state = Status::Capturing;
    save(root, &operation)?;
    if capture(root, &mut operation).is_err() {
        operation.state = Status::Failed;
        save(root, &operation)?;
        return Err("Host capture changed, became unavailable, or could not be privately snapshotted; nothing transferred".into());
    }
    operation.state = Status::Snapshotted;
    save(root, &operation)?;
    Ok(operation)
}

fn capture(root: &Path, operation: &mut Operation) -> Result<()> {
    let started = std::time::Instant::now();
    let root_fd = directory(root)?;
    let dir = operation_dir(root, &operation.id)?;
    let stage = tempfile::Builder::new()
        .prefix("capturing-")
        .tempdir_in(&dir)?;
    std::fs::set_permissions(stage.path(), std::fs::Permissions::from_mode(0o700))?;
    for (index, approved) in operation.files.iter_mut().enumerate() {
        if !recognized(&approved.path) {
            return Err("Unrecognized capture".into());
        }
        let mut input = source(&root_fd, &approved.path)?;
        let before = input.metadata()?;
        if before.dev() != approved.device
            || before.ino() != approved.inode
            || before.len() < approved.bytes
        {
            return Err("Capture was replaced or truncated".into());
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(stage.path().join(format!("{index}.log")))?;
        let mut remaining = approved.bytes;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        while remaining > 0 {
            if started.elapsed().as_secs() >= 30 {
                return Err("Capture deadline exceeded".into());
            }
            let count = input.read(&mut buffer[..remaining.min(64 * 1024) as usize])?;
            if count == 0 {
                return Err("Capture shrank while copying".into());
            }
            output.write_all(&buffer[..count])?;
            digest.update(&buffer[..count]);
            remaining -= count as u64;
        }
        let after = input.metadata()?;
        let named = source(&root_fd, &approved.path)?.metadata()?;
        if after.nlink() != 1
            || after.len() < approved.bytes
            || named.ino() != before.ino()
            || named.dev() != before.dev()
            || (after.len() == before.len()
                && (after.mtime() != before.mtime()
                    || after.mtime_nsec() != before.mtime_nsec()
                    || after.ctime() != before.ctime()
                    || after.ctime_nsec() != before.ctime_nsec()))
        {
            return Err("Capture changed while copying".into());
        }
        output.sync_all()?;
        approved.sha256 = Some(format!("{:x}", digest.finalize()));
    }
    std::fs::rename(stage.path(), dir.join("snapshot"))?;
    File::open(&dir)?.sync_all()?;
    Ok(())
}

fn consent_digest(operation: &Operation) -> Result<String> {
    let mut bound = operation.clone();
    bound.approval.clear();
    bound.state = Status::Prepared;
    bound.detail = None;
    for file in &mut bound.files {
        file.sha256 = None;
    }
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&bound)?)))
}

async fn transfer(
    root: PathBuf,
    id: String,
    caller: CreatedBy,
    approval: String,
) -> Result<Operation> {
    let (root, active, mut operation) = blocking(move || {
        let root = installation_root(&root)?;
        // Bind the caller before creating even an operation lock file.
        let prepared = load(&root, &id, &caller)?;
        let active = active_lock(&root, &id)?;
        fs2::FileExt::try_lock_exclusive(&active)?;
        // The engine has already spent this exact digest's owner permission.
        if prepared.approval != approval {
            return Err("Manifest consent mismatch".into());
        }
        let mut operation = snapshot(&root, &id, &caller)?;
        operation.state = Status::Transferring;
        save(&root, &operation)?;
        Ok((root, active, operation))
    })
    .await?;
    let result = transport::send(&root, &operation).await;
    operation.state = if result.is_ok() {
        Status::Verified
    } else {
        Status::Uncertain
    };
    operation.detail = Some(if result.is_ok() {
        "Every approved file and manifest was downloaded back and SHA256-verified. Existing evidence was preserved."
    } else {
        "SFTP was unavailable, refused, timed out or failed integrity verification. Remote files may be incomplete; do not retry this operation."
    }.into());
    blocking(move || {
        // The active lock stays held until the verdict is saved.
        let _active = active;
        // Remove only this operation's raw staging, retaining durable metadata.
        if cleanup_staging(&root, &operation.id).is_err() {
            operation.detail = Some("Private staging cleanup failed; owner cleanup is required. Check state for remote outcome.".into());
            save(&root, &operation)?;
            return Err("Private staging cleanup failed; owner cleanup is required".into());
        }
        save(&root, &operation)?;
        Ok(operation)
    })
    .await
}

#[path = "host_logs_transport.rs"]
mod transport;

fn active_lock(root: &Path, id: &str) -> Result<File> {
    let dir = directory(&operation_dir(root, id)?)?;
    let file = at(&dir, b"active", libc::O_RDWR | libc::O_CREAT)?;
    if !file.metadata()?.is_file() || file.metadata()?.nlink() != 1 {
        return Err("Invalid operation lock".into());
    }
    Ok(file)
}
fn cleanup_staging(root: &Path, id: &str) -> Result<()> {
    let dir = operation_dir(root, id)?;
    for entry in std::fs::read_dir(&dir)?.take(MAX_DIRECTORY_ENTRIES) {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().ok_or("Invalid staging name")?;
        if name == "snapshot" || name.starts_with("capturing-") || name.starts_with("verify-") {
            directory(&entry.path())?;
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}
fn recover_status(root: &Path, id: &str, caller: &CreatedBy) -> Result<Operation> {
    let read = load(root, id, caller)?;
    // A prepared operation has nothing to recover. Its active lock belongs to
    // a transfer that may start now, after the owner's grant is spent.
    if read.state == Status::Prepared {
        return Ok(read);
    }
    let active = active_lock(root, id)?;
    let mut operation;
    match fs2::FileExt::try_lock_exclusive(&active) {
        Ok(()) => {
            // Reload under the lock: an active transfer may have finished
            // since the caller-binding read. Never overwrite its verdict.
            operation = load(root, id, caller)?;
            if matches!(
                operation.state,
                Status::Capturing | Status::Snapshotted | Status::Transferring
            ) {
                operation.state = if operation.state == Status::Transferring {
                    Status::Uncertain
                } else {
                    Status::Failed
                };
                operation.detail = Some(if operation.state == Status::Uncertain {
                    "Interrupted transfer: remote outcome uncertain; do not retry this operation."
                } else { "Interrupted capture: nothing transferred; this operation cannot replay." }.into());
                save(root, &operation)?;
            }
            if operation.state != Status::Prepared {
                cleanup_staging(root, id)?;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            operation = load(root, id, caller)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(operation)
}

#[cfg(test)]
#[path = "host_logs_tests.rs"]
mod tests;
