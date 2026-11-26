use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::Response,
    routing::get,
    Router,
};

use super::AppState;

/// INVARIANT: The largest message the echo accepts.
///
/// An echo doubles whatever it is given, so an unbounded one lets an
/// authenticated caller size the engine's write buffer for free. Diagnostics
/// are tens of bytes, and the cap is generous against that.
///
/// Enforced on the SOCKET, not in the handler, so an oversized message is
/// refused as it arrives. axum's own default is 16 MiB, which is what a
/// handler-side check would buffer before rejecting.
const MAX_ECHO_BYTES: usize = 64 * 1024;

async fn ws_echo(upgrade: WebSocketUpgrade) -> Response {
    upgrade
        .max_message_size(MAX_ECHO_BYTES)
        .on_upgrade(echo_until_closed)
}

async fn echo_until_closed(mut socket: WebSocket) {
    while let Some(Ok(frame)) = socket.recv().await {
        let reply = match frame {
            Message::Text(t) => Message::Text(t),
            Message::Binary(b) => Message::Binary(b),
            Message::Close(_) => break,
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
