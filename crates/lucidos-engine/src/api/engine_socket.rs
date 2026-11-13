//! The engine socket: the Unix socket the gateway reaches this engine on.
//!
//! The gateway never dials the engine's TCP port, because a TCP port does not
//! say who owns it (ADR 0445). It connects here instead, once the kernel names
//! this process's uid as the listener's. This side checks the same thing in
//! reverse, and drops any peer that is not this user.
//!
//! An agent's subprocess runs as this user too. So each request must also
//! carry the socket credential, which the agent's sandbox cannot read (ADR
//! 0645). A request without it is never routed: the connection closes with no
//! answer.
//!
//! The socket serves the same router as the TCP listeners, so the gateway's
//! requests meet the same door and the same routes. It also drains the way
//! they do: on shutdown it stops accepting, and open connections get the same
//! grace before they close.

use std::path::PathBuf;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use hyper::body::Incoming;
use lucidos_engine_socket::CREDENTIAL_HEADER;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tower::ServiceExt as _;

/// Serve `app` on the engine socket at `socket`, to this user's processes
/// holding its credential only, until `draining` turns true. Then stop
/// accepting, and give open connections up to `drain_for` to finish.
pub async fn serve(
    listener: UnixListener,
    socket: PathBuf,
    app: Router,
    draining: watch::Receiver<bool>,
    drain_for: Duration,
) {
    serve_checked(
        listener,
        socket,
        app,
        draining,
        drain_for,
        lucidos_engine_socket::peer_is_owner,
    )
    .await;
}

/// [`serve`] with the peer check passed in, so a test can stage a stranger.
async fn serve_checked(
    listener: UnixListener,
    socket: PathBuf,
    app: Router,
    mut draining: watch::Receiver<bool>,
    drain_for: Duration,
    is_owner: fn(&UnixStream) -> bool,
) {
    let mut connections = JoinSet::new();
    loop {
        while connections.try_join_next().is_some() {}
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            () = drain_begins(&mut draining) => break,
        };
        let stream = match accepted {
            Ok((stream, _)) => stream,
            // A connection that died before it was accepted is that client's
            // problem. Anything else, such as `EMFILE`, would fail again at
            // once, so pause the way axum's own serve loop does.
            Err(e) if is_connection_error(&e) => continue,
            Err(e) => {
                crate::log!("[API] engine socket accept failed: {e}");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        if !is_owner(&stream) {
            crate::log!("[API] engine socket refused a peer that is not this user");
            continue;
        }
        let credential = match lucidos_engine_socket::credential_for(&socket) {
            Ok(credential) => credential,
            Err(e) => {
                crate::log!("[API] engine socket refused a connection it cannot check: {e}");
                continue;
            }
        };
        connections.spawn(serve_connection(
            stream,
            credential,
            app.clone(),
            draining.clone(),
        ));
    }
    let drained = tokio::time::timeout(drain_for, async {
        while connections.join_next().await.is_some() {}
    });
    if drained.await.is_err() {
        crate::log!(
            "[API] engine socket closed {} connection(s) still open after {}s",
            connections.len(),
            drain_for.as_secs()
        );
    }
}

/// One gateway connection, asked to finish its current exchange and close
/// once the engine starts draining.
///
/// A request without `credential` fails the service, so hyper closes the
/// connection without writing a byte.
async fn serve_connection(
    stream: UnixStream,
    credential: String,
    app: Router,
    mut draining: watch::Receiver<bool>,
) {
    let service = hyper::service::service_fn(move |req: Request<Incoming>| {
        let app = app.clone();
        let admitted = admit(req, &credential);
        async move {
            let Some(req) = admitted else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    UNCREDENTIALED,
                ));
            };
            match app.oneshot(req.map(Body::new)).await {
                Ok(response) => Ok(response),
                Err(never) => match never {},
            }
        }
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
        .with_upgrades();
    tokio::pin!(conn);
    let served = tokio::select! {
        served = conn.as_mut() => served,
        () = drain_begins(&mut draining) => {
            conn.as_mut().graceful_shutdown();
            conn.await
        }
    };
    if let Err(e) = served {
        if !e.is_incomplete_message() {
            crate::log!("[API] engine socket connection ended: {e}");
        }
    }
}

/// Why a request was refused, as hyper logs it when the connection ends.
const UNCREDENTIALED: &str = "a request without the engine socket credential was refused";

/// `req` with its credential removed, or `None` when it presents none. The
/// header never reaches the router, so no handler can log or forward it.
fn admit(mut req: Request<Incoming>, credential: &str) -> Option<Request<Incoming>> {
    let presented = req
        .headers()
        .get(CREDENTIAL_HEADER)
        .and_then(|v| v.to_str().ok());
    if !lucidos_engine_socket::presents_credential(presented, credential) {
        return None;
    }
    req.headers_mut().remove(CREDENTIAL_HEADER);
    Some(req)
}

/// Resolves once the engine starts draining. A dropped sender counts too:
/// the engine is leaving either way.
async fn drain_begins(draining: &mut watch::Receiver<bool>) {
    if draining.wait_for(|d| *d).await.is_err() {
        crate::log!("[API] engine socket lost its drain signal; draining now");
    }
}

fn is_connection_error(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::{ConnectionAborted, ConnectionRefused, ConnectionReset};
    matches!(
        e.kind(),
        ConnectionAborted | ConnectionRefused | ConnectionReset
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    fn scratch() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("les-engine-")
            .tempdir_in("/tmp")
            .expect("scratch dir")
    }

    /// Answers every path, so the route scan sees no unmounted route here.
    /// The body says whether the credential header reached the router.
    fn health_router() -> Router {
        Router::new().fallback(|headers: axum::http::HeaderMap| async move {
            if headers.contains_key(CREDENTIAL_HEADER) {
                "leaked"
            } else {
                "ok"
            }
        })
    }

    /// A health request carrying `credential`, if any. Without `keep_alive`
    /// it asks the server to close after answering.
    fn health_request(credential: Option<&str>, keep_alive: bool) -> Vec<u8> {
        let mut head = String::from("GET /api/v1/health HTTP/1.1\r\nHost: 127.0.0.1:5173\r\n");
        if let Some(credential) = credential {
            head.push_str(&format!("{CREDENTIAL_HEADER}: {credential}\r\n"));
        }
        if !keep_alive {
            head.push_str("Connection: close\r\n");
        }
        head.push_str("\r\n");
        head.into_bytes()
    }

    async fn get_health(socket: &std::path::Path) -> String {
        let mut stream = lucidos_engine_socket::connect(socket)
            .await
            .expect("this user's engine socket");
        let request = health_request(Some(stream.credential()), false);
        stream.write_all(&request).await.expect("write");
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.expect("read");
        answer
    }

    /// Whatever a refused caller reads back. Linux resets a Unix stream closed
    /// with unread bytes where macOS ends it, so a reset is also no answer.
    async fn answer_to(socket: &std::path::Path, credential: Option<&str>) -> Vec<u8> {
        use std::io::ErrorKind::{BrokenPipe, ConnectionReset};
        let mut stream = lucidos_engine_socket::connect(socket)
            .await
            .expect("this user's engine socket");
        let mut answer = Vec::new();
        let exchange = async {
            stream.write_all(&health_request(credential, false)).await?;
            stream.read_to_end(&mut answer).await
        };
        match exchange.await {
            Ok(_) => {}
            Err(e) if matches!(e.kind(), BrokenPipe | ConnectionReset) => {}
            Err(e) => panic!("the refusal was not a close: {e}"),
        }
        answer
    }

    fn bound(dir: &tempfile::TempDir) -> (std::path::PathBuf, UnixListener) {
        let socket = dir.path().join("run").join("engine");
        let listener = lucidos_engine_socket::bind(&socket).expect("bind");
        (socket, listener)
    }

    fn assert_unanswered(answer: &[u8]) {
        assert!(
            answer.is_empty(),
            "a refused caller was answered: {}",
            String::from_utf8_lossy(answer)
        );
    }

    const DRAIN: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn the_socket_serves_the_engines_router() {
        let dir = scratch();
        let (socket, listener) = bound(&dir);
        let (_drain, draining) = watch::channel(false);
        tokio::spawn(serve(
            listener,
            socket.clone(),
            health_router(),
            draining,
            DRAIN,
        ));
        let answer = get_health(&socket).await;
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        assert!(
            answer.ends_with("ok"),
            "the router saw the credential: {answer}"
        );
    }

    /// I3: a peer the check does not name as this user is dropped before a
    /// request is read, so it gets no answer at all.
    #[tokio::test]
    async fn a_peer_that_is_not_this_user_gets_nothing() {
        let dir = scratch();
        let (socket, listener) = bound(&dir);
        let (_drain, draining) = watch::channel(false);
        tokio::spawn(serve_checked(
            listener,
            socket.clone(),
            health_router(),
            draining,
            DRAIN,
            |_| false,
        ));
        let credential = lucidos_engine_socket::credential_for(&socket).expect("credential");
        assert_unanswered(&answer_to(&socket, Some(&credential)).await);
    }

    /// The socket invariant (ADR 0645): this user's process without the
    /// credential, such as an agent's subprocess, gets no answer, not even a
    /// refusal status. A wrong credential fares the same.
    #[tokio::test]
    async fn a_request_without_the_credential_gets_nothing() {
        let dir = scratch();
        let (socket, listener) = bound(&dir);
        let (_drain, draining) = watch::channel(false);
        tokio::spawn(serve(
            listener,
            socket.clone(),
            health_router(),
            draining,
            DRAIN,
        ));
        assert_unanswered(&answer_to(&socket, None).await);
        let wrong = "0".repeat(lucidos_local_token::TOKEN_BYTES * 2);
        assert_unanswered(&answer_to(&socket, Some(&wrong)).await);
        let answer = get_health(&socket).await;
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    }

    /// The drain mirrors the TCP listeners: an idle connection is closed, the
    /// server returns well inside its grace, and nothing new is accepted.
    #[tokio::test]
    async fn draining_closes_idle_connections_and_ends_the_server() {
        let dir = scratch();
        let (socket, listener) = bound(&dir);
        let (drain, draining) = watch::channel(false);
        let server = tokio::spawn(serve(
            listener,
            socket.clone(),
            health_router(),
            draining,
            DRAIN,
        ));
        let mut idle = lucidos_engine_socket::connect(&socket)
            .await
            .expect("an idle gateway connection");
        // One answered exchange proves the server accepted the connection. A
        // connection still in the backlog at the drain is reset, not closed.
        let request = health_request(Some(idle.credential()), true);
        idle.write_all(&request).await.expect("write");
        let mut answered = Vec::new();
        while !answered.ends_with(b"\r\n\r\nok") {
            let mut chunk = [0u8; 512];
            let n = idle.read(&mut chunk).await.expect("read");
            assert!(n > 0, "closed before answering");
            answered.extend_from_slice(&chunk[..n]);
        }
        drain.send_replace(true);
        tokio::time::timeout(DRAIN, server)
            .await
            .expect("the server returns once drained")
            .expect("the server task");
        let mut rest = Vec::new();
        idle.read_to_end(&mut rest)
            .await
            .expect("the idle connection closes");
        assert!(
            lucidos_engine_socket::connect(&socket).await.is_err(),
            "a drained socket accepts nothing"
        );
    }
}
