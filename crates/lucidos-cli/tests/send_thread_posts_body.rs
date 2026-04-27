//! Integration tests: `lucidos send-thread` POSTs the expected JSON body shape
//! to the target workspace's `/api/chat/stream`, with caller_* defaulted from env.

use std::sync::{Arc, Mutex};

// Multi-thread runtime: the test blocks on `Command::status()` (synchronous)
// while the spawned axum task needs progress on a separate worker — single-thread
// would deadlock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_thread_posts_caller_fields_in_body() {
    let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    let cap = captured.clone();
    let app = axum::Router::new().route(
        "/api/chat/stream",
        axum::routing::post(move |body: axum::Json<serde_json::Value>| {
            let cap = cap.clone();
            async move {
                *cap.lock().unwrap() = Some(body.0);
                axum::Json(serde_json::json!({"event_id": "00000000-0000-0000-0000-000000000000"}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });

    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("caller");
    let target = tmp.path().join("dev");
    std::fs::create_dir_all(caller.join(".lucidos")).unwrap();
    std::fs::create_dir_all(target.join(".lucidos")).unwrap();
    std::fs::write(caller.join(".lucidos/ports"), "API_PORT=1\nVITE_PORT=2\n").unwrap();
    std::fs::write(
        target.join(".lucidos/ports"),
        format!("API_PORT={}\nVITE_PORT={}\n", addr.port(), addr.port()),
    ).unwrap();

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let thread_id = uuid::Uuid::new_v4().to_string();
    let event_id = uuid::Uuid::new_v4().to_string();
    let status = std::process::Command::new(bin)
        .args(["send-thread", "--to", "dev", "--message", "do the thing", "--title", "Test", "--insecure-http"])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", &thread_id)
        .env("LUCIDOS_EVENT_ID", &event_id)
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status().expect("spawn cli");
    assert!(status.success());

    let body = captured.lock().unwrap().clone().expect("server received body");
    assert_eq!(body["message"], "do the thing");
    assert_eq!(body["title"], "Test");
    assert_eq!(body["mode"], "agent");
    assert_eq!(body["caller_workspace"], "caller");
    assert_eq!(body["caller_thread_id"], thread_id);
    assert_eq!(body["caller_event_id"], event_id);
    assert!(body.get("parent_thread_id").is_none());
    assert!(body.get("spawning_event_id").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_thread_with_parent_posts_parent_fields_in_body() {
    let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    let cap = captured.clone();
    let app = axum::Router::new().route(
        "/api/chat/stream",
        axum::routing::post(move |body: axum::Json<serde_json::Value>| {
            let cap = cap.clone();
            async move {
                *cap.lock().unwrap() = Some(body.0);
                axum::Json(serde_json::json!({"event_id": "00000000-0000-0000-0000-000000000000"}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });

    // For --parent, target == caller. Single workspace.
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    std::fs::create_dir_all(caller.join(".lucidos")).unwrap();
    std::fs::write(
        caller.join(".lucidos/ports"),
        format!("API_PORT={}\nVITE_PORT={}\n", addr.port(), addr.port()),
    ).unwrap();

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let thread_id = uuid::Uuid::new_v4().to_string();
    let event_id = uuid::Uuid::new_v4().to_string();
    let status = std::process::Command::new(bin)
        .args(["send-thread", "--parent", "--to", "dev", "--cc", "--message", "spawn cc subtask", "--title", "Sub", "--insecure-http"])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", &thread_id)
        .env("LUCIDOS_EVENT_ID", &event_id)
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status().expect("spawn cli");
    assert!(status.success());

    let body = captured.lock().unwrap().clone().expect("server received body");
    assert_eq!(body["message"], "spawn cc subtask");
    assert_eq!(body["title"], "Sub");
    assert_eq!(body["mode"], "agent");
    assert_eq!(body["use_claude_code"], true);
    assert_eq!(body["parent_thread_id"], thread_id);
    assert_eq!(body["spawning_event_id"], event_id);
    assert!(body.get("caller_workspace").is_none());
    assert!(body.get("caller_thread_id").is_none());
    assert!(body.get("caller_event_id").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_thread_with_parent_rejects_different_target() {
    // --parent + --to different-workspace must error before sending anything.
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("caller");
    let other = tmp.path().join("other");
    std::fs::create_dir_all(caller.join(".lucidos")).unwrap();
    std::fs::create_dir_all(other.join(".lucidos")).unwrap();
    std::fs::write(caller.join(".lucidos/ports"), "API_PORT=1\nVITE_PORT=2\n").unwrap();
    std::fs::write(other.join(".lucidos/ports"), "API_PORT=2\nVITE_PORT=3\n").unwrap();

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = std::process::Command::new(bin)
        .args(["send-thread", "--parent", "--to", "other", "--message", "x", "--insecure-http"])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status().expect("spawn cli");
    assert!(!status.success(), "CLI must error on --parent with mismatched target");
}
