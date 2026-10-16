//! The engine socket: how the gateway reaches an engine, and knows it is ours.
//!
//! Full design: `docs/plans/2026-10-12-gateway-reaches-engines-over-a-peer-verified-socket.md`.
//!
//! A loopback TCP port does not say who owns it. An engine can die, and a
//! process of another local user can bind the freed port. The gateway's next
//! request would hand it the local token, a device cookie and the body.
//!
//! # Two layers, and no secret
//!
//! Every socket lives in `~/.lucidos/run`, which [`bind`] keeps 0700, so
//! another user cannot create one there. [`connect`] then asks the kernel for
//! the peer's uid, and hands back a stream only when it is this user's.
//! Nothing is written before that.
//!
//! No key is involved, so nothing is handed out or pinned. An engine the
//! gateway did not start binds the same path from its workspace alone.
//!
//! # A socket another program binds
//!
//! [`owner_only_subdir`] hands out an owner-only directory in the run
//! directory. It serves a server Lucidos launches but did not write, such as
//! the packaged Postgres cluster, which checks its own peers.

use std::fmt;
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{UnixListener, UnixStream};

/// The directory under `~/.lucidos` that holds every engine socket, and the
/// directories [`owner_only_subdir`] hands out.
const RUN_DIR: &str = "run";

/// Hex digits of the workspace digest that name a socket. 64 bits puts a
/// collision out of reach for one user's workspaces, and keeps the path short.
const NAME_HEX_DIGITS: usize = 16;

/// `~/.lucidos/run`. An error only when `HOME` is unset.
pub fn run_dir() -> io::Result<PathBuf> {
    lucidos_local_token::path_for(RUN_DIR)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))
}

/// The engine socket for `workspace`: `~/.lucidos/run/<digest>`.
///
/// A pure function of `HOME` and the canonical workspace path. An engine and
/// the gateway therefore agree on it unprompted, which is what lets the
/// gateway reach an engine it did not start.
pub fn path_for(workspace: &Path) -> io::Result<PathBuf> {
    path_in(&run_dir()?, workspace)
}

/// [`path_for`] under an explicit run directory, so tests need no `HOME`.
fn path_in(run_dir: &Path, workspace: &Path) -> io::Result<PathBuf> {
    let name = digest_name(workspace, "workspace")?;
    let path = run_dir.join(name);
    require_fits(&path)?;
    Ok(path)
}

/// An owner-only directory in `~/.lucidos/run`, for a socket that another
/// program binds, as Postgres binds `.s.PGSQL.<port>`.
///
/// Named `<prefix>-<digest of key>`, so it is a pure function of `HOME` and
/// the canonical `key`, like [`path_for`]. Created 0700, and refused rather
/// than repaired as [`bind`] refuses, so the other program never binds where
/// a stranger can reach.
///
/// `socket_name` is the file the other program will bind inside. The whole
/// path is checked against the kernel limit here, so the refusal names it.
pub fn owner_only_subdir(prefix: &str, key: &Path, socket_name: &str) -> io::Result<PathBuf> {
    lucidos_home::ensure()?;
    owner_only_subdir_in(&run_dir()?, prefix, key, socket_name)
}

/// [`owner_only_subdir`] under an explicit run directory.
fn owner_only_subdir_in(
    run_dir: &Path,
    prefix: &str,
    key: &Path,
    socket_name: &str,
) -> io::Result<PathBuf> {
    let dir = run_dir.join(format!("{prefix}-{}", digest_name(key, "directory")?));
    require_fits(&dir.join(socket_name))?;
    ensure_owner_only_dir(run_dir)?;
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    require_owner_only_dir(&dir)?;
    Ok(dir)
}

/// The first [`NAME_HEX_DIGITS`] hex digits of SHA-256 over the canonical
/// `key`. `what` names the key in the error when it does not exist.
fn digest_name(key: &Path, what: &str) -> io::Result<String> {
    let canonical = std::fs::canonicalize(key).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("{what} {} has no socket name: {e}", key.display()),
        )
    })?;
    let digest = Sha256::digest(canonical.as_os_str().as_bytes());
    Ok(digest
        .iter()
        .take(NAME_HEX_DIGITS / 2)
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Refuse a socket path the kernel would refuse with a bare `EINVAL`.
fn require_fits(path: &Path) -> io::Result<()> {
    let limit = max_path_bytes();
    if path.as_os_str().len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "socket {} is {} bytes, past this platform's limit of {limit}. \
                 A shorter HOME fixes it.",
                path.display(),
                path.as_os_str().len()
            ),
        ));
    }
    Ok(())
}

/// The longest socket path the kernel takes: `sun_path` less its NUL.
fn max_path_bytes() -> usize {
    std::mem::size_of::<libc::sockaddr_un>() - std::mem::offset_of!(libc::sockaddr_un, sun_path) - 1
}

/// This process's effective uid.
fn own_uid() -> libc::uid_t {
    // SAFETY: `geteuid` takes no arguments, cannot fail and touches no memory.
    unsafe { libc::geteuid() }
}

/// Bind the engine socket at `path`, reachable by this user alone.
///
/// The caller must hold its workspace's engine owner lock. The stale socket
/// removed here is then certainly dead, since no other engine for this
/// workspace can be running.
pub fn bind(path: &Path) -> io::Result<UnixListener> {
    let dir = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("engine socket {} has no directory", path.display()),
        )
    })?;
    ensure_owner_only_dir(dir)?;
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Create `dir` and its missing parents at 0700, then refuse it unless it is
/// owner-only.
fn ensure_owner_only_dir(dir: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    require_owner_only_dir(dir)
}

/// Refuse a socket directory another user could create a socket in.
///
/// Refused rather than repaired. A directory that was ever open to others may
/// already hold their socket, and only a person can say how it got that way.
fn require_owner_only_dir(dir: &Path) -> io::Result<()> {
    require_owner_only_dir_of(dir, own_uid())
}

fn require_owner_only_dir_of(dir: &Path, owner: libc::uid_t) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(dir)?;
    let refuse = |why: String| {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("socket directory {} {why}", dir.display()),
        ))
    };
    if !meta.is_dir() {
        return refuse("is not a directory".to_string());
    }
    if meta.uid() != owner {
        return refuse(format!(
            "belongs to uid {}, not this user (uid {owner})",
            meta.uid()
        ));
    }
    if meta.mode() & 0o077 != 0 {
        return refuse(format!(
            "is mode {:o}, and must be 0700",
            meta.mode() & 0o777
        ));
    }
    Ok(())
}

/// Connect to the engine socket at `path`, and prove the listener is ours.
///
/// The only way to an [`OwnedEngineStream`]. The peer check runs before the
/// stream is handed back, so a caller cannot write a byte to a stranger.
pub async fn connect(path: &Path) -> Result<OwnedEngineStream, ConnectError> {
    connect_expecting(path, own_uid()).await
}

async fn connect_expecting(
    path: &Path,
    expected: libc::uid_t,
) -> Result<OwnedEngineStream, ConnectError> {
    let stream = UnixStream::connect(path)
        .await
        .map_err(|source| ConnectError::Unreachable {
            path: path.to_path_buf(),
            source,
        })?;
    let peer_uid = stream.peer_cred().ok().map(|cred| cred.uid());
    if peer_uid != Some(expected) {
        return Err(ConnectError::NotOwner {
            path: path.to_path_buf(),
            peer_uid,
            own_uid: expected,
        });
    }
    Ok(OwnedEngineStream(stream))
}

/// Whether an accepted connection comes from this user.
///
/// The engine's half of the check. The directory and socket modes already keep
/// others out, and this keeps them out if a mode is ever loosened.
pub fn peer_is_owner(stream: &UnixStream) -> bool {
    peer_has_uid(stream, own_uid())
}

fn peer_has_uid(stream: &UnixStream, expected: libc::uid_t) -> bool {
    stream.peer_cred().is_ok_and(|cred| cred.uid() == expected)
}

/// Why [`connect`] handed back no stream.
#[derive(Debug)]
pub enum ConnectError {
    /// Nothing accepted: no socket file, or a socket nobody listens on. A
    /// booting or dead engine looks exactly like this.
    Unreachable { path: PathBuf, source: io::Error },
    /// Something accepted, and the kernel did not name this user as its owner.
    /// `peer_uid` is `None` when the kernel would not name anyone.
    NotOwner {
        path: PathBuf,
        peer_uid: Option<libc::uid_t>,
        own_uid: libc::uid_t,
    },
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectError::Unreachable { path, source } => {
                write!(
                    f,
                    "engine socket {} is unreachable: {source}",
                    path.display()
                )
            }
            ConnectError::NotOwner {
                path,
                peer_uid: Some(peer),
                own_uid,
            } => write!(
                f,
                "engine socket {} is held by uid {peer}, not this user (uid {own_uid}), \
                 so nothing was sent",
                path.display()
            ),
            ConnectError::NotOwner { path, .. } => write!(
                f,
                "engine socket {} has a peer the kernel would not name, so nothing was sent",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ConnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConnectError::Unreachable { source, .. } => Some(source),
            ConnectError::NotOwner { .. } => None,
        }
    }
}

/// A connection to an engine socket whose peer is this user.
///
/// [`connect`] is its only constructor, so holding one is the proof.
#[derive(Debug)]
pub struct OwnedEngineStream(UnixStream);

impl AsyncRead for OwnedEngineStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for OwnedEngineStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }
}

/// Seams for another crate's tests, which cannot otherwise stage a stranger.
#[cfg(feature = "test-support")]
pub mod test_support {
    use super::{ConnectError, OwnedEngineStream};
    use std::path::Path;

    /// This process's effective uid.
    pub fn own_uid() -> libc::uid_t {
        super::own_uid()
    }

    /// [`super::path_for`] under `run_dir` rather than `~/.lucidos/run`.
    pub fn path_in(run_dir: &Path, workspace: &Path) -> std::io::Result<std::path::PathBuf> {
        super::path_in(run_dir, workspace)
    }

    /// [`super::owner_only_subdir`] under `run_dir` rather than `~/.lucidos/run`.
    pub fn owner_only_subdir_in(
        run_dir: &Path,
        prefix: &str,
        key: &Path,
        socket_name: &str,
    ) -> std::io::Result<std::path::PathBuf> {
        super::owner_only_subdir_in(run_dir, prefix, key, socket_name)
    }

    /// [`super::connect`], checking for `expected` rather than this user.
    pub async fn connect_expecting(
        path: &Path,
        expected: libc::uid_t,
    ) -> Result<OwnedEngineStream, ConnectError> {
        super::connect_expecting(path, expected).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    /// A short scratch directory. A test socket must fit `sun_path` too, so
    /// this avoids macOS's long per-user temp directory.
    fn scratch() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("les-")
            .tempdir_in("/tmp")
            .expect("scratch dir")
    }

    fn workspace_in(root: &Path, name: &str) -> PathBuf {
        let ws = root.join(name);
        std::fs::create_dir_all(&ws).expect("workspace dir");
        ws
    }

    #[test]
    fn a_workspace_always_maps_to_the_same_socket() {
        let root = scratch();
        let ws = workspace_in(root.path(), "ws");
        let run = root.path().join("run");
        let first = path_in(&run, &ws).expect("path");
        assert_eq!(first, path_in(&run, &ws).expect("path"));
        assert_eq!(first.parent(), Some(run.as_path()));
        let name = first.file_name().and_then(|n| n.to_str()).expect("name");
        assert_eq!(name.len(), NAME_HEX_DIGITS);
        assert!(name.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    /// The gateway reads a registry path and the engine reads
    /// `LUCIDOS_WORKSPACE`. Either may be spelled through a symlink, as
    /// macOS's `/tmp` is, so both must land on the same socket.
    #[test]
    fn a_symlinked_spelling_maps_to_the_same_socket() {
        let root = scratch();
        let ws = workspace_in(root.path(), "real");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&ws, &link).expect("symlink");
        let run = root.path().join("run");
        assert_eq!(
            path_in(&run, &ws).expect("real"),
            path_in(&run, &link).expect("link")
        );
    }

    #[test]
    fn two_workspaces_get_two_sockets() {
        let root = scratch();
        let run = root.path().join("run");
        let a = path_in(&run, &workspace_in(root.path(), "a")).expect("a");
        let b = path_in(&run, &workspace_in(root.path(), "b")).expect("b");
        assert_ne!(a, b);
    }

    #[test]
    fn a_missing_workspace_is_an_error_naming_it() {
        let root = scratch();
        let err = path_in(&root.path().join("run"), &root.path().join("absent"))
            .expect_err("no workspace, no socket");
        assert!(err.to_string().contains("absent"), "{err}");
    }

    /// The kernel would refuse the bind with a bare `EINVAL`. The error names
    /// the path instead, and says what to change.
    #[test]
    fn a_path_past_the_kernel_limit_is_refused_by_name() {
        let root = scratch();
        let ws = workspace_in(root.path(), "ws");
        let deep = root.path().join("d".repeat(max_path_bytes()));
        let err = path_in(&deep, &ws).expect_err("too long");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("HOME"), "{err}");
    }

    // ── Owner-only subdirectories, for a socket another program binds ──────

    const PG_SOCKET: &str = ".s.PGSQL.5432";

    #[test]
    fn a_subdir_is_owner_only_and_named_by_the_digest_of_its_key() {
        let root = scratch();
        let key = workspace_in(root.path(), "pgdata");
        let run = root.path().join("run");
        let dir = owner_only_subdir_in(&run, "pg", &key, PG_SOCKET).expect("subdir");
        assert_eq!(dir.parent(), Some(run.as_path()));
        let name = dir.file_name().and_then(|n| n.to_str()).expect("name");
        let digest = name.strip_prefix("pg-").expect("prefixed");
        assert_eq!(digest.len(), NAME_HEX_DIGITS);
        assert!(digest.bytes().all(|b| b.is_ascii_hexdigit()));
        for dir in [&run, &dir] {
            let mode = std::fs::metadata(dir).expect("dir").mode();
            assert_eq!(mode & 0o777, 0o700, "{}", dir.display());
        }
        assert_eq!(
            dir,
            owner_only_subdir_in(&run, "pg", &key, PG_SOCKET).expect("again"),
            "a second call finds the same directory"
        );
    }

    #[test]
    fn a_subdir_follows_its_key_through_a_symlink() {
        let root = scratch();
        let key = workspace_in(root.path(), "real");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&key, &link).expect("symlink");
        let run = root.path().join("run");
        assert_eq!(
            owner_only_subdir_in(&run, "pg", &key, PG_SOCKET).expect("real"),
            owner_only_subdir_in(&run, "pg", &link, PG_SOCKET).expect("link")
        );
    }

    #[test]
    fn a_subdir_others_can_enter_is_refused_not_repaired() {
        let root = scratch();
        let key = workspace_in(root.path(), "pgdata");
        let run = root.path().join("run");
        let dir = owner_only_subdir_in(&run, "pg", &key, PG_SOCKET).expect("subdir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).expect("chmod");
        let err = owner_only_subdir_in(&run, "pg", &key, PG_SOCKET).expect_err("loose subdir");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        let message = err.to_string();
        assert!(message.contains(&dir.display().to_string()), "{message}");
        assert!(message.contains("0700"), "{message}");
        let mode = std::fs::metadata(&dir).expect("dir").mode();
        assert_eq!(
            mode & 0o777,
            0o750,
            "a refusal leaves the mode as it found it"
        );
    }

    #[test]
    fn a_subdir_under_a_loose_run_directory_is_refused() {
        let root = scratch();
        let key = workspace_in(root.path(), "pgdata");
        let run = root.path().join("run");
        std::fs::create_dir(&run).expect("run");
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let err = owner_only_subdir_in(&run, "pg", &key, PG_SOCKET).expect_err("loose run dir");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            err.to_string().contains(&run.display().to_string()),
            "{err}"
        );
    }

    /// The owner check reads the uid, so a test can stage a stranger's
    /// directory only by expecting another uid.
    #[test]
    fn a_subdir_of_another_user_is_refused_naming_both_uids() {
        let root = scratch();
        let dir = root.path().join("pg-x");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .expect("dir");
        let stranger = own_uid().wrapping_add(1);
        let err = require_owner_only_dir_of(&dir, stranger).expect_err("foreign owner");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        let message = err.to_string();
        assert!(message.contains(&format!("uid {}", own_uid())), "{message}");
        assert!(message.contains(&format!("uid {stranger}")), "{message}");
    }

    /// The other program would fail its own bind with a bare `EINVAL` in a log
    /// nobody reads. The refusal comes first, and names the socket path.
    #[test]
    fn a_subdir_whose_socket_would_pass_the_kernel_limit_is_refused_by_name() {
        let root = scratch();
        let key = workspace_in(root.path(), "pgdata");
        let deep = root.path().join("d".repeat(max_path_bytes() - 40));
        let err = owner_only_subdir_in(&deep, "pg", &key, PG_SOCKET).expect_err("too long");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        let message = err.to_string();
        assert!(message.contains(PG_SOCKET), "{message}");
        assert!(message.contains("HOME"), "{message}");
        assert!(
            !deep.exists(),
            "nothing is created for a path that cannot work"
        );
    }

    #[tokio::test]
    async fn bind_makes_an_owner_only_directory_and_socket() {
        let root = scratch();
        let path = root.path().join("run").join("sock");
        let _listener = bind(&path).expect("bind");
        let dir_mode = std::fs::metadata(root.path().join("run"))
            .expect("dir")
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        let sock_mode = std::fs::metadata(&path).expect("socket").mode();
        assert_eq!(sock_mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn bind_refuses_a_directory_others_can_enter() {
        let root = scratch();
        let dir = root.path().join("run");
        std::fs::create_dir(&dir).expect("dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let err = bind(&dir.join("sock")).expect_err("a loose directory is refused");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("0700"), "{err}");
    }

    /// A crashed engine leaves its socket file behind. The next lock holder
    /// replaces it.
    #[tokio::test]
    async fn bind_replaces_a_dead_socket() {
        let root = scratch();
        let path = root.path().join("run").join("sock");
        drop(bind(&path).expect("first bind"));
        assert!(path.exists(), "a dropped listener leaves its file");
        let _listener = bind(&path).expect("second bind");
        connect(&path).await.expect("the new listener answers");
    }

    #[tokio::test]
    async fn connect_reaches_this_users_listener() {
        let root = scratch();
        let path = root.path().join("run").join("sock");
        let listener = bind(&path).expect("bind");
        // The client stays connected until the server has checked, because
        // macOS stops naming a peer that has already hung up.
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let owner = peer_is_owner(&stream);
            let mut got = [0u8; 5];
            stream.read_exact(&mut got).await.expect("read");
            (owner, got)
        });
        let mut stream = connect(&path).await.expect("connect");
        stream.write_all(b"hello").await.expect("write");
        let (owner, got) = server.await.expect("server");
        assert!(owner, "the engine side names this user");
        assert_eq!(&got, b"hello");
    }

    /// I1: nothing reaches a listener the kernel does not name as ours. The
    /// listener here IS ours, so the test expects another uid, which is the
    /// only way one user can stage a stranger.
    #[tokio::test]
    async fn connect_sends_nothing_to_a_listener_of_another_user() {
        let root = scratch();
        let path = root.path().join("run").join("sock");
        let listener = bind(&path).expect("bind");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut got = Vec::new();
            tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut got))
                .await
                .expect("the refused connection closes")
                .expect("read");
            got
        });
        let stranger = own_uid().wrapping_add(1);
        let err = connect_expecting(&path, stranger)
            .await
            .expect_err("a stranger's listener is refused");
        match &err {
            ConnectError::NotOwner {
                peer_uid, own_uid, ..
            } => {
                assert_eq!(*peer_uid, Some(super::own_uid()));
                assert_eq!(*own_uid, stranger);
            }
            other => panic!("expected NotOwner, got {other:?}"),
        }
        assert!(err.to_string().contains("nothing was sent"), "{err}");
        assert!(
            server.await.expect("server").is_empty(),
            "a refused listener must read zero bytes"
        );
    }

    #[tokio::test]
    async fn the_engine_side_refuses_a_peer_of_another_user() {
        let root = scratch();
        let path = root.path().join("run").join("sock");
        let listener = bind(&path).expect("bind");
        let _client = connect(&path).await.expect("connect");
        let (stream, _) = listener.accept().await.expect("accept");
        assert!(peer_has_uid(&stream, own_uid()));
        assert!(!peer_has_uid(&stream, own_uid().wrapping_add(1)));
    }

    #[tokio::test]
    async fn a_socket_nobody_listens_on_is_unreachable() {
        let root = scratch();
        let path = root.path().join("run").join("sock");
        drop(bind(&path).expect("bind"));
        let err = connect(&path).await.expect_err("dead socket");
        assert!(matches!(err, ConnectError::Unreachable { .. }), "{err:?}");
        let missing = root.path().join("run").join("never-bound");
        assert!(matches!(
            connect(&missing).await,
            Err(ConnectError::Unreachable { .. })
        ));
    }
}
