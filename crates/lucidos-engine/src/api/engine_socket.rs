//! The engine socket: the Unix socket the gateway reaches this engine on.
//!
//! The gateway never dials the engine's TCP port, because a TCP port does not
//! say who owns it (ADR 0445). It connects here instead, once the kernel names
//! this process's uid as the listener's. This side checks the same thing in
//! reverse, and drops any peer that is not this user.
//!
//! The socket serves the same router as the TCP listeners, so the gateway's
//! requests meet the same door and the same routes. It also drains the way
//! they do: on shutdown it stops accepting, and open connections get the same
//! grace before they close.

use std::time::Duration;

use axum::Router;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::JoinSet;

/// Serve `app` on the engine socket, to this user's processes only, until
/// `draining` turns true. Then stop accepting, and give open connections up to
/// `drain_for` to finish.
pub async fn serve(
    listener: UnixListener,
    app: Router,
    draining: watch::Receiver<bool>,
    drain_for: Duration,
) {
    serve_checked(
        listener,
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
        connections.spawn(serve_connection(stream, app.clone(), draining.clone()));
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
async fn serve_connection(stream: UnixStream, app: Router, mut draining: watch::Receiver<bool>) {
    let service = hyper_util::service::TowerToHyperService::new(app);
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
    fn health_router() -> Router {
        Router::new().fallback(|| async { "ok" })
    }

    const HEALTH_REQUEST: &[u8] =
        b"GET /api/v1/health HTTP/1.1\r\nHost: 127.0.0.1:5173\r\nConnection: close\r\n\r\n";
    /// The same request without `Connection: close`, so the connection stays
    /// open and idle after its answer.
    const KEEP_ALIVE_HEALTH_REQUEST: &[u8] =
        b"GET /api/v1/health HTTP/1.1\r\nHost: 127.0.0.1:5173\r\n\r\n";

    async fn get_health(socket: &std::path::Path) -> String {
        let mut stream = lucidos_engine_socket::connect(socket)
            .await
            .expect("this user's engine socket");
        stream.write_all(HEALTH_REQUEST).await.expect("write");
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.expect("read");
        answer
    }

    /// Whatever a refused peer reads back. Linux resets a Unix stream closed
    /// with unread bytes where macOS ends it, so a reset is also no answer.
    async fn answer_to_a_stranger(socket: &std::path::Path) -> Vec<u8> {
        use std::io::ErrorKind::{BrokenPipe, ConnectionReset};
        let mut stream = lucidos_engine_socket::connect(socket)
            .await
            .expect("this user's engine socket");
        let mut answer = Vec::new();
        let exchange = async {
            stream.write_all(HEALTH_REQUEST).await?;
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

    const DRAIN: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn the_socket_serves_the_engines_router() {
        let dir = scratch();
        let (socket, listener) = bound(&dir);
        let (_drain, draining) = watch::channel(false);
        tokio::spawn(serve(listener, health_router(), draining, DRAIN));
        let answer = get_health(&socket).await;
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        assert!(answer.ends_with("ok"), "{answer}");
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
            health_router(),
            draining,
            DRAIN,
            |_| false,
        ));
        let answer = answer_to_a_stranger(&socket).await;
        assert!(
            answer.is_empty(),
            "a stranger was answered: {}",
            String::from_utf8_lossy(&answer)
        );
    }

    /// The drain mirrors the TCP listeners: an idle connection is closed, the
    /// server returns well inside its grace, and nothing new is accepted.
    #[tokio::test]
    async fn draining_closes_idle_connections_and_ends_the_server() {
        let dir = scratch();
        let (socket, listener) = bound(&dir);
        let (drain, draining) = watch::channel(false);
        let server = tokio::spawn(serve(listener, health_router(), draining, DRAIN));
        let mut idle = lucidos_engine_socket::connect(&socket)
            .await
            .expect("an idle gateway connection");
        // One answered exchange proves the server accepted the connection. A
        // connection still in the backlog at the drain is reset, not closed.
        idle.write_all(KEEP_ALIVE_HEALTH_REQUEST)
            .await
            .expect("write");
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
