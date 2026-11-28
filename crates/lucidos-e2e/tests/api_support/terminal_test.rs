use crate::support::{
    base_url, browser_handshake, home_thread_id, user_client, ws_client_frame,
    ws_read_server_frame, WS_BINARY, WS_CLOSE, WS_TEXT,
};
use lucidos_engine::core::prefs;
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

/// The switch is workspace-wide, so the tests that flip it take turns.
static SWITCH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn switch_url() -> String {
    format!(
        "{}/api/v1/preferences?key={}",
        base_url(),
        prefs::TERMINAL_ENABLED.key()
    )
}

async fn set_terminal_enabled(client: &reqwest::Client, on: Option<bool>) {
    let body: Value = match on {
        Some(on) => client
            .put(switch_url())
            .json(&json!({ "value": on.to_string() }))
            .send(),
        None => client.delete(switch_url()).send(),
    }
    .await
    .expect("terminal switch write failed")
    .json()
    .await
    .expect("terminal switch reply is not JSON");
    let unset = on.is_none()
        && body["error"]
            .as_str()
            .is_some_and(|e| e.ends_with("not found"));
    assert!(body["success"] == true || unset, "{body}");
}

async fn open(client: &reqwest::Client, thread_id: Uuid) -> reqwest::Response {
    client
        .post(format!(
            "{}/api/v1/threads/{thread_id}/terminal",
            base_url()
        ))
        .json(&json!({ "cols": 80, "rows": 24 }))
        .send()
        .await
        .expect("open request failed")
}

#[tokio::test]
async fn a_terminal_is_refused_while_the_switch_is_off() {
    let _switch = SWITCH.lock().await;
    let client = user_client().await;
    set_terminal_enabled(&client, None).await;
    let response = open(&client, home_thread_id().await).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body: Value = response.json().await.expect("refusal is JSON");
    assert_eq!(body["reason"], "terminal-disabled", "{body}");
}

#[tokio::test]
async fn an_app_document_cannot_turn_the_terminal_on() {
    let _switch = SWITCH.lock().await;
    let client = user_client().await;
    let response = client
        .put(switch_url())
        .header("sec-fetch-site", "same-origin")
        .header("referer", format!("{}/app/habit-tracker/", base_url()))
        .json(&json!({ "value": "true" }))
        .send()
        .await
        .expect("switch request failed");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    set_terminal_enabled(&client, None).await;
}

/// Opens a shell on the home thread, runs one command through the socket,
/// kills the shell and reads the exit frame.
async fn round_trip(client: &reqwest::Client) -> (Value, String, Value) {
    let home = home_thread_id().await;
    let opened: Value = open(client, home)
        .await
        .json()
        .await
        .expect("open reply is JSON");
    let terminal_id = opened["terminal_id"].as_str().expect("a terminal id");
    let ticket = opened["ticket"].as_str().expect("a socket ticket");
    let path = format!("/api/v1/threads/{home}/terminal/{terminal_id}/socket?ticket={ticket}");
    let response = browser_handshake(client, &path, &base_url())
        .send()
        .await
        .expect("upgrade request failed");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = response.upgrade().await.expect("upgrade the connection");
    let hello = br#"{"type":"hello","cols":80,"rows":24}"#;
    socket
        .write_all(&ws_client_frame(WS_TEXT, hello))
        .await
        .expect("send hello");
    socket
        .write_all(&ws_client_frame(WS_BINARY, b"echo term''inal-e2e\n"))
        .await
        .expect("send input");

    let mut output = String::new();
    while !output.contains("terminal-e2e") {
        let (opcode, payload) = ws_read_server_frame(&mut socket).await;
        assert_ne!(opcode, WS_CLOSE, "the socket closed early: {output:?}");
        if opcode == WS_BINARY {
            output.push_str(&String::from_utf8_lossy(&payload));
        }
    }

    let killed = client
        .delete(format!(
            "{}/api/v1/threads/{home}/terminal/{terminal_id}",
            base_url()
        ))
        .send()
        .await
        .expect("kill request failed");
    assert!(killed.status().is_success(), "{}", killed.status());

    loop {
        let (opcode, payload) = ws_read_server_frame(&mut socket).await;
        assert_ne!(opcode, WS_CLOSE, "the socket closed before an exit frame");
        if opcode != WS_TEXT {
            continue;
        }
        let frame: Value = serde_json::from_slice(&payload).expect("a JSON frame");
        if frame["type"] == "exit" {
            return (opened, output, frame);
        }
    }
}

#[tokio::test]
async fn a_shell_runs_a_command_through_its_socket_and_reports_its_kill() {
    let _switch = SWITCH.lock().await;
    let client = user_client().await;
    set_terminal_enabled(&client, Some(true)).await;
    let steps = tokio::spawn({
        let client = client.clone();
        async move { tokio::time::timeout(Duration::from_secs(30), round_trip(&client)).await }
    })
    .await;
    set_terminal_enabled(&client, None).await;
    let (opened, output, exit) = steps
        .expect("the terminal round trip panicked")
        .expect("the terminal round trip timed out");

    assert_eq!(opened["expires_in_secs"], 30, "{opened}");
    assert!(output.contains("terminal-e2e"), "{output:?}");
    assert_eq!(exit["cause"], "killed", "{exit}");
}
