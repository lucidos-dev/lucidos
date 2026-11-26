use crate::support::{base_url, browser_handshake, http_client};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn client_text_frame(text: &str) -> Vec<u8> {
    let payload = text.as_bytes();
    assert!(payload.len() < 126, "test payloads stay in the short form");
    let mask = [0x37u8, 0xfa, 0x21, 0x3d];
    let mut frame = vec![0x81, 0x80 | payload.len() as u8];
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    frame
}

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

#[tokio::test]
async fn ws_echo_upgrades_and_returns_what_it_is_sent() {
    let response = browser_handshake(&http_client(), "/api/v1/ws-echo", &base_url())
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

#[tokio::test]
async fn ws_echo_refuses_a_handshake_from_another_page() {
    let response = browser_handshake(&http_client(), "/api/v1/ws-echo", "http://localhost:1")
        .send()
        .await
        .expect("upgrade request failed");

    assert_eq!(
        response.status(),
        reqwest::StatusCode::FORBIDDEN,
        "a foreign page must not reach a socket on this engine"
    );
}
