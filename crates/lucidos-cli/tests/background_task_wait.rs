//! Integration tests: `lucidos background-task wait` asks the engine's wait
//! route for the calling thread, forwards `--max-secs`, and prints the answer.
//! `run --poll` sends `poll: true`.

use std::path::Path;
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<String>>>;

/// An engine stand-in that records each request's path, query and body, and
/// answers the wait route as a still-running task.
async fn start_engine_stub() -> (u16, Seen) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let wait_seen = seen.clone();
    let run_seen = seen.clone();
    let app = axum::Router::new()
        .route(
            "/api/v1/threads/:thread/background-tasks/:task/wait",
            axum::routing::get(move |uri: axum::http::Uri| {
                let seen = wait_seen.clone();
                async move {
                    seen.lock().unwrap().push(uri.to_string());
                    axum::Json(serde_json::json!({"finished": false, "stdout": "half"}))
                }
            }),
        )
        .route(
            "/api/v1/threads/:thread/background-tasks",
            axum::routing::post(move |body: axum::Json<serde_json::Value>| {
                let seen = run_seen.clone();
                async move {
                    seen.lock().unwrap().push(body.0.to_string());
                    axum::Json(serde_json::json!({"status": "polled", "task_id": "t1"}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, seen)
}

fn write_ports_file(dir: &Path, port: u16) {
    std::fs::create_dir_all(dir.join(".lucidos")).unwrap();
    std::fs::write(
        dir.join(".lucidos/ports"),
        format!("API_PORT={port}\nVITE_PORT={port}\nPROTO=http\n"),
    )
    .unwrap();
}

/// Runs the CLI with a cleared environment. A session's own
/// `LUCIDOS_API_BASE_URL` would otherwise aim it at a real engine.
fn run_cli(ws: &Path, thread_id: &str, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_lucidos"))
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", ws)
        .env("LUCIDOS_WORKSPACE", ws)
        .env("LUCIDOS_THREAD_ID", thread_id)
        .current_dir(ws)
        .output()
        .expect("spawn cli")
}

// Multi-thread runtime: the CLI call blocks this thread while the stub needs
// a worker of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_asks_the_threads_wait_route_and_prints_the_answer() {
    let (port, seen) = start_engine_stub().await;
    let tmp = tempfile::tempdir().unwrap();
    write_ports_file(tmp.path(), port);
    let thread_id = uuid::Uuid::new_v4().to_string();

    let output = run_cli(
        tmp.path(),
        &thread_id,
        &["background-task", "wait", "t1", "--max-secs", "60"],
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("half"));

    let output = run_cli(tmp.path(), &thread_id, &["background-task", "wait", "t1"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        [
            format!("/api/v1/threads/{thread_id}/background-tasks/t1/wait?max_secs=60"),
            format!("/api/v1/threads/{thread_id}/background-tasks/t1/wait"),
        ],
        "no --max-secs leaves the cap to the engine"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_poll_asks_for_no_event_wait() {
    let (port, seen) = start_engine_stub().await;
    let tmp = tempfile::tempdir().unwrap();
    write_ports_file(tmp.path(), port);

    let output = run_cli(
        tmp.path(),
        &uuid::Uuid::new_v4().to_string(),
        &["background-task", "run", "--poll", "--", "make"],
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: serde_json::Value =
        serde_json::from_str(&seen.lock().unwrap()[0]).expect("a JSON body");
    assert_eq!(body["poll"], true);
    assert_eq!(body["command"], "make");
}
