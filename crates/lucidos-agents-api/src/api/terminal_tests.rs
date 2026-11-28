use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, Uri};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;
use crate::api::gateway_stamp::test_support::stamped_device_headers;
use crate::api::permission_requests::tests::{agent_headers, permission_test_state};
use crate::core::prefs;
use crate::engine::terminal::{CatchUp, Output};
use crate::test_support::{
    boot_mock_engine, in_engine_child, seed_chat_thread, seed_preference, setup_test_db,
    teardown_test_db,
};

const PHONE: &str = "owners-phone";
const LAPTOP: &str = "owners-laptop";

fn shell_headers(device_id: &str) -> HeaderMap {
    let mut headers = stamped_device_headers(device_id);
    headers.insert("sec-fetch-site", "same-origin".parse().unwrap());
    headers.insert("referer", "http://localhost/".parse().unwrap());
    headers
}

async fn send(
    state: &AppState,
    method: &str,
    uri: &str,
    headers: HeaderMap,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(if body.is_null() {
            String::new()
        } else {
            body.to_string()
        }))
        .unwrap();
    request.headers_mut().extend(headers);
    let response = router()
        .with_state(state.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn opened_count(state: &AppState, thread_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'TerminalOpened'",
    )
    .bind(thread_id)
    .fetch_one(&state.pool)
    .await
    .unwrap()
}

async fn exit_cause(state: &AppState, thread_id: Uuid) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let cause: Option<String> = sqlx::query_scalar(
            "SELECT payload->>'cause' FROM events \
             WHERE thread_id = $1 AND event_type = 'TerminalExited'",
        )
        .bind(thread_id)
        .fetch_optional(&state.pool)
        .await
        .unwrap();
        if cause.is_some() {
            return cause;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}

fn socket_uri(thread_id: Uuid, terminal_id: Uuid, ticket: &str) -> Uri {
    format!("/threads/{thread_id}/terminal/{terminal_id}/socket?ticket={ticket}")
        .parse()
        .unwrap()
}

async fn read_until(attachment: &mut crate::engine::terminal::Attachment, needle: &str) -> String {
    let mut seen = match &attachment.catch_up {
        CatchUp::Bytes { data, .. } | CatchUp::Reset { data, .. } => data.clone(),
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !String::from_utf8_lossy(&seen).contains(needle) {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, attachment.output.recv()).await {
            Ok(Ok(Output::Bytes { data, .. })) => seen.extend_from_slice(&data),
            Ok(Ok(Output::Resized { .. } | Output::Attachments { .. })) => {}
            _ => break,
        }
    }
    String::from_utf8_lossy(&seen).into_owned()
}

#[tokio::test]
async fn while_the_switch_is_off_nothing_starts_a_shell_or_mints_a_ticket() {
    if !in_engine_child(
        "api::terminal::tests::while_the_switch_is_off_nothing_starts_a_shell_or_mints_a_ticket",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    let thread_id = seed_chat_thread(&state.engine.event_bus, "hello").await;
    let open_uri = format!("/threads/{thread_id}/terminal");
    let ticket_uri = format!("/threads/{thread_id}/terminal/{}/ticket", Uuid::new_v4());

    for switch in [None, Some("false")] {
        if let Some(value) = switch {
            seed_preference(&state.pool, prefs::TERMINAL_ENABLED.key(), value)
                .await
                .unwrap();
        }
        let (status, body) = send(&state, "POST", &open_uri, shell_headers(PHONE), json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{switch:?}: {body}");
        assert_eq!(body["reason"], lifecycle::DISABLED_REASON, "{body}");
        let (status, body) =
            send(&state, "POST", &ticket_uri, shell_headers(PHONE), json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{switch:?}: {body}");
        assert_eq!(body["reason"], lifecycle::DISABLED_REASON, "{body}");
        let (status, body) =
            send(&state, "GET", &open_uri, shell_headers(PHONE), Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, json!({ "enabled": false, "terminal": null }));
        let refusal = super::super::terminal_socket::admit(
            &state,
            &shell_headers(PHONE),
            &socket_uri(thread_id, Uuid::new_v4(), "any"),
            thread_id,
            Uuid::new_v4(),
        )
        .await
        .err()
        .expect("the handshake is refused while the switch is off");
        assert_eq!(refusal.status, StatusCode::FORBIDDEN);
    }
    assert_eq!(opened_count(&state, thread_id).await, 0);
    assert!(!workspace.path().join(".lucidos/terminals").exists());
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_app_page_an_unnamed_page_and_an_agent_cannot_open_a_terminal() {
    if !in_engine_child(
        "api::terminal::tests::an_app_page_an_unnamed_page_and_an_agent_cannot_open_a_terminal",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    seed_preference(&state.pool, prefs::TERMINAL_ENABLED.key(), "true")
        .await
        .unwrap();
    let thread_id = seed_chat_thread(&state.engine.event_bus, "hello").await;
    let open_uri = format!("/threads/{thread_id}/terminal");

    let mut app_page = shell_headers(PHONE);
    app_page.insert(
        "referer",
        "http://localhost/app/habit-tracker/".parse().unwrap(),
    );
    let mut unnamed_page = shell_headers(PHONE);
    unnamed_page.remove("referer");
    for (who, headers) in [
        ("an app page", app_page),
        ("an unnamed page", unnamed_page),
        ("an agent", agent_headers(Some(thread_id))),
    ] {
        let (status, body) = send(&state, "POST", &open_uri, headers, json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}: {body}");
    }
    assert_eq!(opened_count(&state, thread_id).await, 0);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_chat_thread_shell_starts_in_data_and_its_ticket_opens_one_socket() {
    if !in_engine_child(
        "api::terminal::tests::a_chat_thread_shell_starts_in_data_and_its_ticket_opens_one_socket",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    seed_preference(&state.pool, prefs::TERMINAL_ENABLED.key(), "true")
        .await
        .unwrap();
    let data = workspace.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let thread_id = seed_chat_thread(&state.engine.event_bus, "hello").await;
    let open_uri = format!("/threads/{thread_id}/terminal");

    let (status, opened) = send(
        &state,
        "POST",
        &open_uri,
        shell_headers(PHONE),
        json!({ "cols": 100, "rows": 30 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    let terminal_id: Uuid = opened["terminal_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        (opened["cols"].as_u64(), opened["rows"].as_u64()),
        (Some(100), Some(30))
    );
    let first = opened["ticket"].as_str().unwrap().to_owned();

    let refusal = super::super::terminal_socket::admit(
        &state,
        &shell_headers(LAPTOP),
        &socket_uri(thread_id, terminal_id, &first),
        thread_id,
        terminal_id,
    )
    .await
    .err()
    .expect("a header naming another device refuses the ticket");
    assert_eq!(refusal.status, StatusCode::FORBIDDEN);

    let ticket_uri = format!("/threads/{thread_id}/terminal/{terminal_id}/ticket");
    let (status, minted) = send(&state, "POST", &ticket_uri, shell_headers(PHONE), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    let second = socket_uri(thread_id, terminal_id, minted["ticket"].as_str().unwrap());
    let terminal = super::super::terminal_socket::admit(
        &state,
        &shell_headers(PHONE),
        &second,
        thread_id,
        terminal_id,
    )
    .await
    .unwrap_or_else(|_| panic!("the minting device's ticket opens the socket"));
    assert!(
        super::super::terminal_socket::admit(
            &state,
            &shell_headers(PHONE),
            &second,
            thread_id,
            terminal_id
        )
        .await
        .is_err(),
        "a ticket spends once"
    );

    let mut attachment = terminal.attach(None);
    terminal.write(b"cd . && pwd\n").unwrap();
    let canonical = data.canonicalize().unwrap();
    let screen = read_until(&mut attachment, &canonical.display().to_string()).await;
    assert!(
        screen.contains(&canonical.display().to_string()),
        "the shell starts in data/: {screen:?}"
    );

    let (status, view) = send(&state, "GET", &open_uri, shell_headers(PHONE), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["enabled"], true);
    assert_eq!(view["terminal"]["terminal_id"], terminal_id.to_string());
    assert_eq!(view["terminal"]["attachments"], 1);
    drop(attachment);

    let stale = format!("/threads/{thread_id}/terminal/{}", Uuid::new_v4());
    let (status, _) = send(&state, "DELETE", &stale, shell_headers(PHONE), Value::Null).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a kill names the live terminal"
    );

    sqlx::query("UPDATE preferences SET value = 'false' WHERE key = $1")
        .bind(prefs::TERMINAL_ENABLED.key())
        .execute(&state.pool)
        .await
        .unwrap();
    let kill = format!("/threads/{thread_id}/terminal/{terminal_id}");
    let (status, body) = send(&state, "DELETE", &kill, shell_headers(PHONE), Value::Null).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "a kill works with the switch off: {body}"
    );
    assert_eq!(
        exit_cause(&state, thread_id).await.as_deref(),
        Some("killed")
    );
    teardown_test_db(&db).await;
}

async fn suggestions(state: &AppState, thread_id: Uuid) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT payload FROM events \
         WHERE thread_id = $1 AND event_type = 'TerminalCommandSuggested' ORDER BY created",
    )
    .bind(thread_id)
    .fetch_all(&state.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn an_agent_posts_a_suggestion_on_its_own_thread_even_with_the_switch_off() {
    if !in_engine_child(
        "api::terminal::tests::an_agent_posts_a_suggestion_on_its_own_thread_even_with_the_switch_off",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    let thread_id = seed_chat_thread(&state.engine.event_bus, "hello").await;

    let (status, body) = send(
        &state,
        "POST",
        "/terminal-suggestions",
        agent_headers(Some(thread_id)),
        json!({ "command": "git status --short", "label": "  See what changed  " }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let posted = suggestions(&state, thread_id).await;
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0]["command"], "git status --short");
    assert_eq!(posted[0]["label"], "See what changed");
    assert_eq!(posted[0]["suggestion_id"], body["suggestion_id"]);

    for (who, headers, payload, expected) in [
        (
            "a device",
            shell_headers(PHONE),
            json!({ "command": "ls" }),
            StatusCode::FORBIDDEN,
        ),
        (
            "an agent with no thread",
            agent_headers(None),
            json!({ "command": "ls" }),
            StatusCode::FORBIDDEN,
        ),
        (
            "a line break",
            agent_headers(Some(thread_id)),
            json!({ "command": "ls\nrm -rf ~" }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "an empty command",
            agent_headers(Some(thread_id)),
            json!({ "command": "  " }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "a control character in the label",
            agent_headers(Some(thread_id)),
            json!({ "command": "ls", "label": "a\u{1b}[2Kb" }),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, body) = send(&state, "POST", "/terminal-suggestions", headers, payload).await;
        assert_eq!(status, expected, "{who}: {body}");
    }
    assert_eq!(suggestions(&state, thread_id).await.len(), 1);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_type_route_refuses_the_switch_off_an_app_page_an_agent_and_a_busy_shell() {
    if !in_engine_child(
        "api::terminal::tests::the_type_route_refuses_the_switch_off_an_app_page_an_agent_and_a_busy_shell",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    std::fs::create_dir_all(workspace.path().join("data")).unwrap();
    seed_preference(&state.pool, prefs::TERMINAL_ENABLED.key(), "true")
        .await
        .unwrap();
    let thread_id = seed_chat_thread(&state.engine.event_bus, "hello").await;
    let (status, opened) = send(
        &state,
        "POST",
        &format!("/threads/{thread_id}/terminal"),
        shell_headers(PHONE),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    let terminal_id: Uuid = opened["terminal_id"].as_str().unwrap().parse().unwrap();
    let type_uri = format!("/threads/{thread_id}/terminal/{terminal_id}/type");
    let terminal = named_terminal(&state, thread_id, terminal_id)
        .await
        .unwrap();
    let mut view = terminal.attach(None);
    terminal.write(b"echo re''ady\n").unwrap();
    read_until(&mut view, "ready").await;

    let mut app_page = shell_headers(PHONE);
    app_page.insert(
        "referer",
        "http://localhost/app/habit-tracker/".parse().unwrap(),
    );
    for (who, headers) in [
        ("an app page", app_page),
        ("an agent", agent_headers(Some(thread_id))),
    ] {
        let (status, body) = send(
            &state,
            "POST",
            &type_uri,
            headers,
            json!({ "command": "ls" }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}: {body}");
    }
    let (status, body) = send(
        &state,
        "POST",
        &type_uri,
        shell_headers(PHONE),
        json!({ "command": "ls\r" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let marker = workspace.path().join("typed-not-run");
    let (status, body) = send(
        &state,
        "POST",
        &type_uri,
        shell_headers(LAPTOP),
        json!({ "command": format!("touch {}", marker.display()) }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "any paired device types: {body}"
    );
    read_until(&mut view, "typed-not-run").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!marker.exists(), "the typed command ran without Enter");

    terminal.write(b"\x15sleep 30\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while terminal.shell_in_foreground() {
        assert!(Instant::now() < deadline, "sleep never took the foreground");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (status, body) = send(
        &state,
        "POST",
        &type_uri,
        shell_headers(PHONE),
        json!({ "command": "ls" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["reason"], suggestion::BUSY_REASON);

    sqlx::query("UPDATE preferences SET value = 'false' WHERE key = $1")
        .bind(prefs::TERMINAL_ENABLED.key())
        .execute(&state.pool)
        .await
        .unwrap();
    let (status, body) = send(
        &state,
        "POST",
        &type_uri,
        shell_headers(PHONE),
        json!({ "command": "ls" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["reason"], lifecycle::DISABLED_REASON);
    drop(view);
    state
        .engine
        .terminals
        .end(thread_id, TerminalExitCause::Killed)
        .await;
    teardown_test_db(&db).await;
}
