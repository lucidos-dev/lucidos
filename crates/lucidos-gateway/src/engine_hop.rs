//! The gateway's one road to an engine: its engine socket, peer-checked.
//!
//! Plan: `docs/plans/2026-10-12-gateway-reaches-engines-over-a-peer-verified-socket.md`.
//!
//! Every request the gateway makes of an engine goes through [`send`] or
//! [`send_upgrade`]. Both run over a connection that
//! `lucidos_engine_socket::connect` has already proved belongs to this user.
//! A credential is attached here and nowhere else, so it can only ever cross
//! such a connection.
//!
//! Each request opens its own connection. A Unix socket connect costs
//! microseconds, and a fresh connection is a fresh peer check.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{header, HeaderValue, Request, Response};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use lucidos_engine_socket::ConnectError;

/// Where one engine answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineAddress {
    /// The engine socket, or why the workspace has none. A missing socket
    /// reads as an unreachable engine, and the engine itself refuses to boot
    /// over the same cause, naming it.
    socket: Result<PathBuf, String>,
    /// The engine's TCP port. Only `Host` reads it, so the engine sees the
    /// request it saw when the gateway dialled that port.
    port: u16,
    /// The uid a test claims owns the socket. Lets one user stage a stranger.
    #[cfg(test)]
    owner_uid: Option<libc::uid_t>,
}

impl EngineAddress {
    /// The engine serving the workspace at `dir`, registered on `port`.
    pub fn for_workspace(dir: &Path, port: u16) -> Self {
        Self {
            socket: socket_for(dir).map_err(|e| e.to_string()),
            port,
            #[cfg(test)]
            owner_uid: None,
        }
    }

    /// An engine socket a test bound itself.
    #[cfg(test)]
    pub fn at(socket: PathBuf, port: u16) -> Self {
        Self {
            socket: Ok(socket),
            port,
            owner_uid: None,
        }
    }

    /// The same socket, as though another user held it.
    #[cfg(test)]
    pub fn held_by_a_stranger(mut self) -> Self {
        let own = lucidos_engine_socket::test_support::own_uid();
        self.owner_uid = Some(own.wrapping_add(1));
        self
    }

    fn host(&self) -> HeaderValue {
        HeaderValue::try_from(format!("127.0.0.1:{}", self.port))
            .expect("an IPv4 address and a port make a valid header value")
    }

    async fn connect(&self) -> Result<lucidos_engine_socket::OwnedEngineStream, HopError> {
        let socket = self
            .socket
            .as_ref()
            .map_err(|why| HopError::NoSocket(why.clone()))?;
        #[cfg(test)]
        if let Some(uid) = self.owner_uid {
            return lucidos_engine_socket::test_support::connect_expecting(socket, uid)
                .await
                .map_err(HopError::Connect);
        }
        lucidos_engine_socket::connect(socket)
            .await
            .map_err(HopError::Connect)
    }
}

/// The engine socket for the workspace at `dir`.
#[cfg(not(test))]
fn socket_for(dir: &Path) -> io::Result<PathBuf> {
    lucidos_engine_socket::path_for(dir)
}

/// The same derivation, rooted in a per-user temp directory, so the
/// gateway's tests never write into the machine's own `~/.lucidos/run`.
#[cfg(test)]
fn socket_for(dir: &Path) -> io::Result<PathBuf> {
    lucidos_engine_socket::test_support::path_in(&test_engine::run_dir(), dir)
}

impl fmt::Display for EngineAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.socket {
            Ok(socket) => write!(f, "{} (:{})", socket.display(), self.port),
            Err(_) => write!(f, "no engine socket (:{})", self.port),
        }
    }
}

/// The credential a hop presents to the engine.
#[derive(Clone, Copy)]
pub enum Credential<'a> {
    /// Nothing: a probe, or a read that needs no authority.
    None,
    /// The full-authority local token, which the proxy presents.
    Local(&'a str),
    /// The hook socket token, which only the hook socket presents (ADR 0155).
    HookSocket(&'a str),
}

/// Why a hop got no answer.
#[derive(Debug)]
pub enum HopError {
    /// The workspace has no engine socket path. Nothing was sent.
    NoSocket(String),
    /// No connection, or one whose peer is not this user. Nothing was sent.
    Connect(ConnectError),
    /// The engine accepted, and the HTTP exchange failed.
    Http(hyper::Error),
}

impl HopError {
    /// Something not provably this user's held the engine socket.
    pub fn is_foreign(&self) -> bool {
        matches!(self, HopError::Connect(ConnectError::NotOwner { .. }))
    }

    /// Nothing accepted: a booting or a dead engine, or a workspace with no
    /// engine socket at all.
    pub fn is_unreachable(&self) -> bool {
        matches!(
            self,
            HopError::NoSocket(_) | HopError::Connect(ConnectError::Unreachable { .. })
        )
    }
}

impl fmt::Display for HopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HopError::NoSocket(why) => write!(f, "no engine socket: {why}"),
            HopError::Connect(e) => e.fmt(f),
            HopError::Http(e) => write!(f, "engine exchange failed: {e}"),
        }
    }
}

impl std::error::Error for HopError {}

/// Send `req` to the engine at `addr`, and return its response head.
///
/// `req` carries an origin-form URI: the path and query the engine serves.
/// The body streams in both directions, so an SSE stream or an upload never
/// sits in gateway memory.
pub async fn send(
    addr: &EngineAddress,
    req: Request<Body>,
    credential: Credential<'_>,
) -> Result<Response<Incoming>, HopError> {
    let (mut sender, conn) = open(addr).await?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    sender
        .send_request(prepare(addr, req, credential))
        .await
        .map_err(HopError::Http)
}

/// [`send`] for a WebSocket handshake. The connection survives a 101, so
/// `hyper::upgrade::on` can take the socket over.
pub async fn send_upgrade(
    addr: &EngineAddress,
    req: Request<Body>,
    credential: Credential<'_>,
) -> Result<Response<Incoming>, HopError> {
    let (mut sender, conn) = open(addr).await?;
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });
    sender
        .send_request(prepare(addr, req, credential))
        .await
        .map_err(HopError::Http)
}

type Opened = (
    hyper::client::conn::http1::SendRequest<Body>,
    hyper::client::conn::http1::Connection<TokioIo<lucidos_engine_socket::OwnedEngineStream>, Body>,
);

async fn open(addr: &EngineAddress) -> Result<Opened, HopError> {
    let stream = addr.connect().await?;
    hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(HopError::Http)
}

/// Stamp the gateway's own headers, replacing any value already present.
///
/// `Host` is the engine's TCP authority. The credential is the one the caller
/// named, and none at all for [`Credential::None`]. Callers strip a client's
/// copy of both token headers before this point (`proxy::gateway_owns_header`).
fn prepare(
    addr: &EngineAddress,
    mut req: Request<Body>,
    credential: Credential<'_>,
) -> Request<Body> {
    let headers = req.headers_mut();
    headers.insert(header::HOST, addr.host());
    let (name, token) = match credential {
        Credential::None => return req,
        Credential::Local(token) => (crate::auth::HEADER_LOCAL_TOKEN, token),
        Credential::HookSocket(token) => (crate::auth::HEADER_HOOK_SOCKET_TOKEN, token),
    };
    if let Some(value) = crate::auth::sensitive_credential(token) {
        headers.insert(name, value);
    }
    req
}

/// A stand-in engine on a real engine socket, for the gateway's tests.
#[cfg(test)]
pub(crate) mod test_engine {
    use super::EngineAddress;
    use std::path::PathBuf;

    /// The engine port tests register. Only `Host` carries it now.
    pub const TEST_PORT: u16 = 5173;

    /// Where [`EngineAddress::for_workspace`] roots its sockets under test.
    /// One owner-only directory per test process, because `bind` refuses a
    /// directory another uid owns. Under `/tmp`, so a path fits `sun_path`.
    pub fn run_dir() -> PathBuf {
        static RUN_DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        RUN_DIR
            .get_or_init(|| {
                use std::os::unix::fs::PermissionsExt;
                let dir = tempfile::Builder::new()
                    .prefix("lucidos-run-")
                    .tempdir_in("/tmp")
                    .expect("a test run directory");
                std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
                    .expect("an owner-only test run directory");
                dir
            })
            .path()
            .to_path_buf()
    }

    /// Bind the engine socket the gateway derives for the workspace at `dir`,
    /// as that workspace's engine would.
    pub fn bind_for_workspace(dir: &std::path::Path) -> tokio::net::UnixListener {
        let socket = lucidos_engine_socket::test_support::path_in(&run_dir(), dir)
            .expect("a test workspace has a socket path");
        lucidos_engine_socket::bind(&socket).expect("bind a test workspace's engine socket")
    }

    /// A scratch socket directory short enough for `sun_path`.
    pub fn scratch() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("lgw-")
            .tempdir_in("/tmp")
            .expect("scratch dir")
    }

    /// Bind an engine socket inside `dir`, and the address that reaches it.
    pub fn bind(dir: &tempfile::TempDir) -> (tokio::net::UnixListener, EngineAddress) {
        let socket: PathBuf = dir.path().join("run").join("engine");
        let listener = lucidos_engine_socket::bind(&socket).expect("bind a test engine socket");
        (listener, EngineAddress::at(socket, TEST_PORT))
    }

    /// An address nothing listens on: a booting or a dead engine.
    pub fn unreachable(dir: &tempfile::TempDir) -> EngineAddress {
        EngineAddress::at(dir.path().join("run").join("nobody"), TEST_PORT)
    }

    /// An engine socket that records every byte of one connection, to EOF.
    ///
    /// For a refusal test: the gateway closes a refused connection, so the
    /// task finishes, and an empty result proves nothing was sent.
    pub fn record_one(
        dir: &tempfile::TempDir,
    ) -> (EngineAddress, tokio::task::JoinHandle<Vec<u8>>) {
        use tokio::io::AsyncReadExt as _;
        let (listener, addr) = bind(dir);
        let recorded = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut got = Vec::new();
            let read = stream.read_to_end(&mut got);
            tokio::time::timeout(std::time::Duration::from_secs(5), read)
                .await
                .expect("the gateway closes a refused connection")
                .expect("read");
            got
        });
        (addr, recorded)
    }

    /// Serve `app` on a fresh engine socket, as an engine would.
    pub fn serve(dir: &tempfile::TempDir, app: axum::Router) -> EngineAddress {
        let (listener, addr) = bind(dir);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let app = app.clone();
                tokio::spawn(async move {
                    let service = hyper_util::service::TowerToHyperService::new(app);
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                        .with_upgrades()
                        .await;
                });
            }
        });
        addr
    }
}

#[cfg(test)]
mod tests {
    use super::test_engine::{bind, scratch, unreachable, TEST_PORT};
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    /// Accept one request, record its head, and answer 200.
    async fn capture_one(listener: tokio::net::UnixListener) -> String {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let mut buf = vec![0u8; 4096];
        let n = sock.read(&mut buf).await.unwrap_or(0);
        let _ = sock
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await;
        String::from_utf8_lossy(&buf[..n]).to_lowercase()
    }

    fn get(path: &str) -> Request<Body> {
        Request::builder()
            .uri(path)
            .body(Body::empty())
            .expect("request")
    }

    #[tokio::test]
    async fn the_engine_sees_its_own_tcp_authority_as_host() {
        let dir = scratch();
        let (listener, addr) = bind(&dir);
        let captured = tokio::spawn(capture_one(listener));
        let resp = send(&addr, get("/api/v1/health"), Credential::None)
            .await
            .expect("send");
        assert_eq!(resp.status(), 200);
        let head = captured.await.expect("captured");
        assert!(head.starts_with("get /api/v1/health http/1.1"), "{head}");
        assert!(
            head.contains(&format!("host: 127.0.0.1:{TEST_PORT}")),
            "{head}"
        );
    }

    #[tokio::test]
    async fn only_the_named_credential_is_attached() {
        for (credential, present, absent) in [
            (
                Credential::Local("own-local"),
                Some("x-lucidos-local-token: own-local"),
                "x-lucidos-hook-socket-token",
            ),
            (
                Credential::HookSocket("own-hook"),
                Some("x-lucidos-hook-socket-token: own-hook"),
                "x-lucidos-local-token",
            ),
            (Credential::None, None, "x-lucidos-"),
        ] {
            let dir = scratch();
            let (listener, addr) = bind(&dir);
            let captured = tokio::spawn(capture_one(listener));
            send(&addr, get("/"), credential).await.expect("send");
            let head = captured.await.expect("captured");
            if let Some(present) = present {
                assert!(head.contains(present), "{head}");
            }
            assert!(!head.contains(absent), "{head}");
        }
    }

    /// A stamped value replaces whatever the request already carried, so a
    /// header that slipped past a caller's strip still cannot reach the engine.
    #[tokio::test]
    async fn the_gateways_values_replace_any_already_present() {
        let dir = scratch();
        let (listener, addr) = bind(&dir);
        let captured = tokio::spawn(capture_one(listener));
        let req = Request::builder()
            .uri("/")
            .header(header::HOST, "attacker.example")
            .header(crate::auth::HEADER_LOCAL_TOKEN, "forged")
            .body(Body::empty())
            .expect("request");
        send(&addr, req, Credential::Local("own-local"))
            .await
            .expect("send");
        let head = captured.await.expect("captured");
        assert!(!head.contains("attacker.example"), "{head}");
        assert!(!head.contains("forged"), "{head}");
        assert!(head.contains("x-lucidos-local-token: own-local"), "{head}");
    }

    #[tokio::test]
    async fn a_missing_engine_is_unreachable_not_foreign() {
        let dir = scratch();
        let err = send(&unreachable(&dir), get("/"), Credential::Local("own-local"))
            .await
            .expect_err("nobody listens");
        assert!(err.is_unreachable(), "{err}");
        assert!(!err.is_foreign(), "{err}");
    }

    /// I2: the hop is the only road to an engine. A token header is attached
    /// here and nowhere else, and no other module builds a loopback URL. The
    /// one exception is the peer lookup, which asks other GATEWAYS.
    #[test]
    fn no_other_module_reaches_an_engine() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offences = Vec::new();
        let mut dirs = vec![src.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                let name = path
                    .strip_prefix(&src)
                    .expect("under src")
                    .to_string_lossy()
                    .into_owned();
                if !name.ends_with(".rs") || name.ends_with("_tests.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("read source");
                let shipped = text.split("#[cfg(test)]").next().unwrap_or_default();
                for (n, line) in shipped.lines().enumerate() {
                    let code = line.trim_start();
                    if code.starts_with("//") {
                        continue;
                    }
                    let names_a_token = code.contains("HEADER_LOCAL_TOKEN")
                        || code.contains("HEADER_HOOK_SOCKET_TOKEN");
                    let token_ok = name == "engine_hop.rs"
                        || name == "auth.rs"
                        || code.contains("eq_ignore_ascii_case");
                    if names_a_token && !token_ok {
                        offences.push(format!("{name}:{} attaches a token: {code}", n + 1));
                    }
                    if code.contains("://127.0.0.1") && name != "peers.rs" {
                        offences.push(format!("{name}:{} dials loopback: {code}", n + 1));
                    }
                }
            }
        }
        assert!(
            offences.is_empty(),
            "reach an engine through `engine_hop` only:\n{}",
            offences.join("\n")
        );
    }
}
