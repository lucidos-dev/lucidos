use crate::support::{
    base_url, browser_handshake, http_client, ws_client_frame, ws_read_server_frame, WS_TEXT,
};
use tokio::io::AsyncWriteExt;

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
        .write_all(&ws_client_frame(WS_TEXT, b"through the socket"))
        .await
        .expect("send");
    assert_eq!(
        ws_read_server_frame(&mut socket).await,
        (WS_TEXT, b"through the socket".to_vec())
    );
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
