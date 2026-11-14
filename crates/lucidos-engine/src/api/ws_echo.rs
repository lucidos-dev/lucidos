//! `GET /api/v1/ws-echo`: a WebSocket that echoes what you send it.
//!
//! The diagnostic for the upgrade path, and the WebSocket analogue of
//! `/api/v1/health`. It answers one question: does an upgrade survive
//! everything between a client and this engine?
//!
//! That path has several hops and each can drop an upgrade on its own. The
//! gateway proxies it (ADR 0151), Tailscale Serve may front the gateway, and a
//! PWA service worker sits in the browser. A `101` here means every hop
//! carried it. A failure names the hop that did not, with no voice session to
//! debug at the same time.
//!
//! Authenticated like every other `/api/v1` route, and it reaches nothing:
//! frames go back to the caller that sent them and nowhere else.

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::Response,
    routing::get,
    Router,
};

use super::AppState;

/// The largest message the echo accepts.
///
/// An echo doubles whatever it is given, so an unbounded one lets an
/// authenticated caller size the engine's write buffer for free. Diagnostics
/// are tens of bytes, and the cap is generous against that.
///
/// Enforced on the SOCKET, not in the handler, so an oversized message is
/// refused as it arrives. axum's own default is 16 MiB, which is what a
/// handler-side check would buffer before rejecting.
const MAX_ECHO_BYTES: usize = 64 * 1024;

/// `GET /api/v1/ws-echo`: upgrade, then echo every frame back.
async fn ws_echo(upgrade: WebSocketUpgrade) -> Response {
    upgrade
        .max_message_size(MAX_ECHO_BYTES)
        .on_upgrade(echo_until_closed)
}

/// Echo text and binary frames until the socket closes.
///
/// Ping and pong are left to axum, which answers a ping itself. Anything past
/// the size cap arrives as an error and ends the loop, rather than being
/// truncated: a truncated echo would read as a transport fault, which is the
/// one thing this endpoint exists to rule out.
async fn echo_until_closed(mut socket: WebSocket) {
    while let Some(Ok(frame)) = socket.recv().await {
        let reply = match frame {
            Message::Text(t) => Message::Text(t),
            Message::Binary(b) => Message::Binary(b),
            Message::Close(_) => break,
            // Already answered by axum.
            Message::Ping(_) | Message::Pong(_) => continue,
        };
        if socket.send(reply).await.is_err() {
            break;
        }
    }
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/ws-echo", get(ws_echo))
}
