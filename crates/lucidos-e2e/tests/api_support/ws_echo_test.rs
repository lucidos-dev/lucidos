//! `/api/v1/ws-echo` against a booted engine.
//!
//! The upgrade path's own proof. The gateway's splice is covered by unit tests
//! in `lucidos-gateway::proxy`, and this covers the other end: that the route
//! is reachable through the engine's whole middleware stack, and that a socket
//! opened on it actually carries bytes.
//!
//! Frames are built by hand rather than with a WebSocket library. The two are
//! trivial at this size, and a dependency added for one test would be linked
//! into every build of this crate.

use crate::support::{base_url, http_client};
use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A masked client text frame, the only shape a client may send.
fn client_text_frame(text: &str) -> Vec<u8> {
    let payload = text.as_bytes();
    assert!(payload.len() < 126, "test payloads stay in the short form");
    let mask = [0x37u8, 0xfa, 0x21, 0x3d];
    let mut frame = vec![0x81, 0x80 | payload.len() as u8];
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    frame
}

/// Read one unmasked server text frame and return its payload.
async fn read_server_text<S>(socket: &mut S) -> String
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut head = [0u8; 2];
    socket.read_exact(&mut head).await.expect("frame header");
    assert_eq!(head[0], 0x81, "expected a final text frame");
    assert_eq!(head[1] & 0x80, 0, "a server frame is never masked");
    let len = (head[1] & 0x7f) as usize;
    let mut payload = vec![0u8; len];
    socket.read_exact(&mut payload).await.expect("frame body");
    String::from_utf8(payload).expect("utf8 payload")
}

/// The engine upgrades, and what goes in comes back out.
///
/// A `101` alone would prove the route resolves. The round-trip is what proves
/// the socket is live, which is the question the endpoint exists to answer.
#[tokio::test]
async fn ws_echo_upgrades_and_returns_what_it_is_sent() {
    let key = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
    let response = http_client()
        .get(format!("{}/api/v1/ws-echo", base_url()))
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", &key)
        .send()
        .await
        .expect("upgrade request failed");

    assert_eq!(
        response.status(),
        reqwest::StatusCode::SWITCHING_PROTOCOLS,
        "the engine must accept the upgrade"
    );
    let accept = response
        .headers()
        .get("sec-websocket-accept")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    assert!(accept.is_some(), "the handshake answer must be present");

    let mut socket = response.upgrade().await.expect("upgrade the connection");
    socket
        .write_all(&client_text_frame("through the socket"))
        .await
        .expect("send");
    assert_eq!(read_server_text(&mut socket).await, "through the socket");
}
