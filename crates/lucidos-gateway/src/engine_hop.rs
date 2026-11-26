use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{header, HeaderValue, Request, Response};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use lucidos_engine_socket::ConnectError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineAddress {
    socket: Result<PathBuf, String>,
    port: u16,
    #[cfg(test)]
    owner_uid: Option<libc::uid_t>,
}

impl EngineAddress {
    pub fn for_workspace(dir: &Path, port: u16) -> Self {
        Self {
            socket: socket_for(dir).map_err(|e| e.to_string()),
            port,
            #[cfg(test)]
            owner_uid: None,
        }
    }

    #[cfg(test)]
    pub fn at(socket: PathBuf, port: u16) -> Self {
        Self {
            socket: Ok(socket),
            port,
            owner_uid: None,
        }
    }

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

#[cfg(not(test))]
fn socket_for(dir: &Path) -> io::Result<PathBuf> {
    lucidos_engine_socket::path_for(dir)
}

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

#[derive(Clone, Copy)]
pub enum Credential<'a> {
    None,
    Local(&'a str),
    HookSocket(&'a str),
}

#[derive(Debug)]
pub enum HopError {
    NoSocket(String),
    Connect(ConnectError),
    Http(hyper::Error),
}

impl HopError {
    pub fn is_foreign(&self) -> bool {
        matches!(self, HopError::Connect(ConnectError::NotOwner { .. }))
    }

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

pub async fn send(
    addr: &EngineAddress,
    req: Request<Body>,
    credential: Credential<'_>,
) -> Result<Response<Incoming>, HopError> {
    let (mut sender, conn, socket_credential) = open(addr).await?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    sender
        .send_request(prepare(addr, req, &socket_credential, credential))
        .await
        .map_err(HopError::Http)
}

pub async fn send_upgrade(
    addr: &EngineAddress,
    req: Request<Body>,
    credential: Credential<'_>,
) -> Result<Response<Incoming>, HopError> {
    let (mut sender, conn, socket_credential) = open(addr).await?;
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });
    sender
        .send_request(prepare(addr, req, &socket_credential, credential))
        .await
        .map_err(HopError::Http)
}

type Opened = (
    hyper::client::conn::http1::SendRequest<Body>,
    hyper::client::conn::http1::Connection<TokioIo<lucidos_engine_socket::OwnedEngineStream>, Body>,
    String,
);

async fn open(addr: &EngineAddress) -> Result<Opened, HopError> {
    let stream = addr.connect().await?;
    let socket_credential = stream.credential().to_string();
    let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(HopError::Http)?;
    Ok((sender, conn, socket_credential))
}

fn prepare(
    addr: &EngineAddress,
    mut req: Request<Body>,
    socket_credential: &str,
    credential: Credential<'_>,
) -> Request<Body> {
    let headers = req.headers_mut();
    headers.insert(header::HOST, addr.host());
    if let Some(value) = crate::auth::sensitive_credential(socket_credential) {
        headers.insert(lucidos_engine_socket::CREDENTIAL_HEADER, value);
    }
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

#[cfg(test)]
pub(crate) mod test_engine {
    use super::EngineAddress;
    use std::path::PathBuf;

    pub const TEST_PORT: u16 = 5173;

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

    pub fn bind_for_workspace(dir: &std::path::Path) -> tokio::net::UnixListener {
        let socket = lucidos_engine_socket::test_support::path_in(&run_dir(), dir)
            .expect("a test workspace has a socket path");
        lucidos_engine_socket::bind(&socket).expect("bind a test workspace's engine socket")
    }

    pub fn scratch() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("lgw-")
            .tempdir_in("/tmp")
            .expect("scratch dir")
    }

    pub fn bind(dir: &tempfile::TempDir) -> (tokio::net::UnixListener, EngineAddress) {
        let socket: PathBuf = dir.path().join("run").join("engine");
        let listener = lucidos_engine_socket::bind(&socket).expect("bind a test engine socket");
        (listener, EngineAddress::at(socket, TEST_PORT))
    }

    pub fn unreachable(dir: &tempfile::TempDir) -> EngineAddress {
        EngineAddress::at(dir.path().join("run").join("nobody"), TEST_PORT)
    }

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
            (Credential::None, None, "x-lucidos-local-token"),
            (Credential::None, None, "x-lucidos-hook-socket-token"),
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

    #[tokio::test]
    async fn every_hop_carries_the_socket_credential() {
        for credential in [Credential::None, Credential::Local("own-local")] {
            let dir = scratch();
            let (listener, addr) = bind(&dir);
            let socket = dir.path().join("run").join("engine");
            let expected = lucidos_engine_socket::credential_for(&socket).expect("credential");
            let captured = tokio::spawn(capture_one(listener));
            let req = Request::builder()
                .uri("/")
                .header(lucidos_engine_socket::CREDENTIAL_HEADER, "forged")
                .body(Body::empty())
                .expect("request");
            send(&addr, req, credential).await.expect("send");
            let head = captured.await.expect("captured");
            let line = format!("{}: {expected}", lucidos_engine_socket::CREDENTIAL_HEADER);
            assert!(head.contains(&line), "{head}");
            assert!(!head.contains("forged"), "{head}");
        }
    }

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
                        || code.contains("HEADER_HOOK_SOCKET_TOKEN")
                        || code.contains("CREDENTIAL_HEADER");
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
