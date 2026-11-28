use axum::http::HeaderMap;
use lucidos_gateway_stamp::workspace_operation::{PROTOCOL_HEADER, PROTOCOL_VERSION};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<(HeaderMap, Value)>>>;

async fn source() -> (u16, Seen) {
    source_with_status(axum::http::StatusCode::OK).await
}

async fn source_with_status(status: axum::http::StatusCode) -> (u16, Seen) {
    source_with_protocol(status, Some(PROTOCOL_VERSION)).await
}

async fn source_with_protocol(
    status: axum::http::StatusCode,
    protocol: Option<&'static str>,
) -> (u16, Seen) {
    let seen: Seen = Default::default();
    let record = seen.clone();
    let app = axum::Router::new().route(
        "/api/v1/workspace-operations",
        axum::routing::post(
            move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
                let record = record.clone();
                async move {
                    record.lock().unwrap().push((headers, body.clone()));
                    let reply = if body["operation"]["kind"] == "spawn" {
                        json!({"thread_id":body["operation"]["body"]["thread_id"]})
                    } else if body["operation"]["request"]["kind"] == "thread-liveness" {
                        json!({"state":"working", "status":"running"})
                    } else {
                        json!({"from":"target", "kind":body["operation"]["request"]["kind"]})
                    };
                    use axum::response::IntoResponse;
                    let mut response = (status, axum::Json(reply)).into_response();
                    if let Some(protocol) = protocol {
                        response
                            .headers_mut()
                            .insert(PROTOCOL_HEADER, protocol.parse().unwrap());
                    }
                    response
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, seen)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_or_unsupported_source_protocol_requires_an_upgrade_without_fallback() {
    for protocol in [None, Some("999")] {
        let (port, seen) = source_with_protocol(axum::http::StatusCode::OK, protocol).await;
        let dir = tempfile::tempdir().unwrap();
        let output = cli(port, dir.path())
            .args([
                "threads",
                "messages",
                "--to",
                "target@cloud",
                "--thread",
                "thread-1",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("Upgrade the caller's source workspace engine"));
        assert_eq!(
            seen.lock()
                .unwrap()
                .iter()
                .filter(|(_, request)| request["operation"]["request"]["kind"] == "thread-messages")
                .count(),
            1
        );
    }
}

fn cli(port: u16, dir: &std::path::Path) -> std::process::Command {
    let workspace = dir.join("source");
    std::fs::create_dir_all(workspace.join(".lucidos")).unwrap();
    std::fs::write(
        workspace.join(".lucidos/ports"),
        format!("API_PORT={port}\nPROTO=http\n"),
    )
    .unwrap();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_lucidos"));
    command
        .env_clear()
        .env("HOME", dir)
        .env("LUCIDOS_PRIVATE_DIR", dir.join("private"))
        .env("LUCIDOS_WORKSPACE", workspace)
        .env("LUCIDOS_WORKSPACES_ROOT", dir.join("no-target-directories"))
        .env("LUCIDOS_API_BASE_URL", format!("http://127.0.0.1:{port}"))
        .env("LUCIDOS_AGENT_ORIGIN_TOKEN", "source-origin-token")
        .env("LUCIDOS_THREAD_ID", "00000000-0000-0000-0000-000000000001");
    command
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_and_discovery_use_the_source_engine_and_keep_stdout_json() {
    let (port, seen) = source().await;
    let dir = tempfile::tempdir().unwrap();
    for (args, kind) in [
        (
            vec!["workspaces", "targets", "--via", "routing-gateway"],
            "workspace-targets",
        ),
        (
            vec![
                "threads",
                "messages",
                "--via",
                "routing-gateway",
                "--to",
                "target@cloud",
                "--thread",
                "thread-1",
            ],
            "thread-messages",
        ),
        (
            vec![
                "events",
                "query",
                "--via",
                "routing-gateway",
                "--to",
                "target@cloud",
                "--thread-id",
                "thread-1",
                "--type",
                "MessageReceived",
                "--limit",
                "7",
            ],
            "events",
        ),
        (
            vec![
                "intelligence-ladder",
                "--via",
                "routing-gateway",
                "--to",
                "target@cloud",
                "--thread",
                "thread-1",
                "--json",
            ],
            "intelligence-ladder",
        ),
    ] {
        let output = cli(port, dir.path()).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["kind"], kind);
        if kind == "events" || kind == "thread-messages" {
            assert!(String::from_utf8_lossy(&output.stderr).contains("# thread: working"));
        }
    }
    let seen = seen.lock().unwrap();
    assert!(seen.len() >= 6);
    for (headers, request) in seen.iter() {
        assert_eq!(
            headers["x-lucidos-agent-origin-token"],
            "source-origin-token"
        );
        assert_eq!(request["via"], "routing-gateway");
        assert_eq!(request["operation"]["kind"], "read");
    }
    let event_request = seen
        .iter()
        .find(|(_, body)| body["operation"]["request"]["kind"] == "events")
        .unwrap();
    assert_eq!(
        event_request.1["operation"]["request"]["thread_id"],
        "thread-1"
    );
    assert_eq!(
        event_request.1["operation"]["request"]["query"]["limit"],
        "7"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_routed_spawn_never_resolves_a_target_directory() {
    let (port, seen) = source().await;
    let dir = tempfile::tempdir().unwrap();
    let output = cli(port, dir.path())
        .args([
            "spawn-thread",
            "--via",
            "routing-gateway",
            "--to",
            "target@cloud",
            "--message",
            "Start a task",
            "--title",
            "[draft] Routed task",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1["operation"]["kind"], "spawn");
    assert_eq!(seen[0].1["to"], "target@cloud");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["title"], "[draft] Routed task");
    assert_eq!(result["via"], "routing-gateway");
    assert_eq!(result["to"], "target@cloud");
    assert_eq!(
        result["thread_id"],
        seen[0].1["operation"]["body"]["thread_id"]
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("thread:"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_routed_spawn_refusal_is_not_retried_or_posted_directly() {
    let (port, seen) = source_with_status(axum::http::StatusCode::FORBIDDEN).await;
    let dir = tempfile::tempdir().unwrap();
    let output = cli(port, dir.path())
        .args([
            "spawn-thread",
            "--to",
            "target@cloud",
            "--message",
            "Start a task",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("403"));
    assert!(output.stdout.is_empty());
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_writes_and_ambient_remote_ladders_fail_before_any_request() {
    let (port, seen) = source().await;
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec![
            "events",
            "emit",
            "AnalysisCompleted",
            "--payload",
            "{}",
            "--summary",
            "test",
            "--via",
            "routing-gateway",
            "--to",
            "target@cloud",
        ],
        vec![
            "threads",
            "archive",
            "--thread",
            "thread-1",
            "--via",
            "routing-gateway",
            "--to",
            "target@cloud",
        ],
        vec![
            "intelligence-ladder",
            "--via",
            "routing-gateway",
            "--to",
            "target@cloud",
        ],
    ] {
        let output = cli(port, dir.path()).args(args).output().unwrap();
        assert!(!output.status.success());
    }
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changes_apply_prints_the_deferred_build_outcome() {
    let reply = json!({
        "status": "applied",
        "version_build": "deferred",
        "deferred_coding_threads": 2,
        "message": "Applied. The new version waits for 2 coding threads to finish."
    });
    let served = reply.clone();
    let app = axum::Router::new().route(
        "/api/v1/changes/:id/apply",
        axum::routing::post(move || {
            let served = served.clone();
            async move { axum::Json(served) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let output = cli(port, dir.path())
        .args(["changes", "apply", "11111111-1111-1111-1111-111111111111"])
        .output()
        .unwrap();
    server.abort();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        reply
    );
}
