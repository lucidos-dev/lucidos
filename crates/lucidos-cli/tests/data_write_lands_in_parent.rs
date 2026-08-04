//! `lucidos data write` resolves the PARENT workspace and hands the content to
//! that workspace's engine.
//!
//! Originally this asserted the file landed at `<parent>/data/artifacts/...`
//! after a direct `std::fs::write`. That write announced nothing: no
//! `DataFileWritten`, no `Artifact*`, no git commit, so a file written this way
//! was invisible to the Files panel, the memory index and `on_event` triggers,
//! and the chat link the command prints reloaded the whole workspace on click
//! (the artifact rewriter could not resolve a path the cache had never heard
//! of). The write now goes through the engine's `PUT /api/v1/data/*path`, the
//! announced write path (ADR 0032).
//!
//! So the parent-resolution invariant is asserted where it now lives: the
//! REQUEST. The port comes from the parent's `.lucidos/ports` and the path is
//! the normalized store path, neither of which a worktree-rooted resolution
//! could produce.

use std::fs;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::routing::put;
use axum::Router;

const LUCIDOS: &str = env!("CARGO_BIN_EXE_lucidos");

/// One captured `PUT /api/v1/data/*path`.
struct Captured {
    path: String,
    body: Vec<u8>,
}

/// A stub engine that answers the data-write route. Returns the bound port and
/// a receiver the test drains after running the CLI. `status` is what the stub
/// answers with, so the failure path can be driven too.
fn spawn_stub_engine(status: StatusCode) -> (u16, Receiver<Captured>) {
    let (tx, rx) = mpsc::channel();
    let tx = Arc::new(Mutex::new(tx));
    let (port_tx, port_rx) = mpsc::channel();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async move {
            let app = Router::new()
                .route("/api/v1/data/*path", put(capture))
                .with_state((tx, status));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            port_tx
                .send(listener.local_addr().expect("addr").port())
                .expect("send port");
            axum::serve(listener, app).await.expect("serve");
        });
    });

    let port = port_rx.recv().expect("stub engine must bind");
    (port, rx)
}

async fn capture(
    State((tx, status)): State<(Arc<Mutex<Sender<Captured>>>, StatusCode)>,
    AxumPath(path): AxumPath<String>,
    body: Bytes,
) -> (StatusCode, String) {
    tx.lock()
        .expect("lock")
        .send(Captured {
            path,
            body: body.to_vec(),
        })
        .expect("record request");
    if status.is_success() {
        (status, r#"{"success":true}"#.to_string())
    } else {
        (status, r#"{"error":"disk on fire"}"#.to_string())
    }
}

fn write_ports(workspace: &std::path::Path, port: u16) {
    let lucidos = workspace.join(".lucidos");
    fs::create_dir_all(&lucidos).unwrap();
    // `http` so the CLI's blocking client talks plain HTTP to the stub, which
    // serves no TLS. A real engine writes `PROTO=https` here in dev.
    fs::write(
        lucidos.join("ports"),
        format!("API_PORT={}\nVITE_PORT={}\nPROTO=http\n", port, port),
    )
    .unwrap();
}

#[test]
fn write_from_worktree_targets_the_parent_workspace_engine() {
    let (port, requests) = spawn_stub_engine(StatusCode::OK);

    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    write_ports(&workspace, port);

    // Mirror the engine's worktree layout so the walk-up logic gets exercised
    // exactly the way it would at runtime.
    let worktree = workspace.join(".lucidos/worktrees/abc123");
    fs::create_dir_all(&worktree).unwrap();

    let src = tmp.path().join("input.txt");
    fs::write(&src, b"hello world").unwrap();

    let out = Command::new(LUCIDOS)
        .args(["data", "write", "artifacts/ua/test.txt", "--from"])
        .arg(&src)
        .current_dir(&worktree)
        .env_remove("LUCIDOS_WORKSPACE")
        .env_remove("LUCIDOS_API_BASE_URL")
        .output()
        .expect("lucidos binary should run");
    assert!(
        out.status.success(),
        "lucidos data write failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // The write reached the PARENT workspace's engine (its port, from its
    // ports file) at the normalized store path. A worktree-rooted resolution
    // could produce neither.
    let req = requests
        .recv()
        .expect("engine must have received the write");
    assert_eq!(req.path, "artifacts/ua/test.txt");
    assert_eq!(req.body, b"hello world");

    // stdout carries the ready-to-paste clickable chat link: basename label,
    // bare store-path target (no scheme, since a scheme would dead-end on
    // click). Unchanged by the move to HTTP.
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "[test.txt](artifacts/ua/test.txt)"
    );

    // stderr carries the resolved absolute path under the PARENT workspace, so
    // `… 2>/tmp/path` keeps working.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.trim().ends_with("ws/data/artifacts/ua/test.txt"),
        "stderr must name the parent-workspace absolute path, got: {stderr}"
    );
    assert!(
        !stderr.contains("worktrees"),
        "stderr must not name the worktree, got: {stderr}"
    );
}

#[test]
fn write_falls_back_to_env_when_pwd_outside_workspace() {
    let (port, requests) = spawn_stub_engine(StatusCode::OK);

    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    write_ports(&workspace, port);

    // PWD is unrelated to the workspace. Resolution must fall through to env.
    let unrelated = tmp.path().join("elsewhere");
    fs::create_dir_all(&unrelated).unwrap();

    let src = tmp.path().join("input.txt");
    fs::write(&src, b"via env").unwrap();

    let out = Command::new(LUCIDOS)
        .args(["data", "write", "artifacts/x.txt", "--from"])
        .arg(&src)
        .current_dir(&unrelated)
        .env("LUCIDOS_WORKSPACE", &workspace)
        .env_remove("LUCIDOS_API_BASE_URL")
        .output()
        .expect("lucidos binary should run");
    assert!(
        out.status.success(),
        "lucidos data write failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let req = requests
        .recv()
        .expect("engine must have received the write");
    assert_eq!(req.path, "artifacts/x.txt");
    assert_eq!(req.body, b"via env");
}

#[test]
fn write_normalizes_and_encodes_the_request_path() {
    let (port, requests) = spawn_stub_engine(StatusCode::OK);

    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    write_ports(&workspace, port);

    let src = tmp.path().join("input.txt");
    fs::write(&src, b"data").unwrap();

    // A loose name gets the `artifacts/` prefix, and a space in the filename
    // must survive the URL round-trip rather than producing an invalid URL.
    let out = Command::new(LUCIDOS)
        .args(["data", "write", "quarterly report.md", "--from"])
        .arg(&src)
        .current_dir(tmp.path())
        .env("LUCIDOS_WORKSPACE", &workspace)
        .env_remove("LUCIDOS_API_BASE_URL")
        .output()
        .expect("lucidos binary should run");
    assert!(
        out.status.success(),
        "lucidos data write failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Axum percent-decodes, so the engine sees the original name back.
    let req = requests
        .recv()
        .expect("engine must have received the write");
    assert_eq!(req.path, "artifacts/quarterly report.md");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "[quarterly report.md](artifacts/quarterly report.md)"
    );
}

#[test]
fn write_fails_loudly_when_the_engine_rejects_it() {
    let (port, requests) = spawn_stub_engine(StatusCode::INTERNAL_SERVER_ERROR);

    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    write_ports(&workspace, port);

    let src = tmp.path().join("input.txt");
    fs::write(&src, b"doomed").unwrap();

    let out = Command::new(LUCIDOS)
        .args(["data", "write", "artifacts/doomed.txt", "--from"])
        .arg(&src)
        .current_dir(tmp.path())
        .env("LUCIDOS_WORKSPACE", &workspace)
        .env_remove("LUCIDOS_API_BASE_URL")
        .output()
        .expect("lucidos binary should run");

    assert!(
        !out.status.success(),
        "a rejected write must exit non-zero, got success"
    );
    // No chat link for a write that did not land: printing one would hand the
    // agent a link to a file the workspace does not have.
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "",
        "stdout must be empty when the write failed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("500"),
        "stderr must carry the engine's status, got: {stderr}"
    );
    assert!(
        stderr.contains("disk on fire"),
        "stderr must carry the engine's message, got: {stderr}"
    );
    // The request really was attempted (the failure is the engine's answer, not
    // the CLI declining to try).
    assert!(requests.recv().is_ok());
}

#[test]
fn write_reports_an_unreachable_engine_actionably() {
    // Bind then drop to get a port nothing is listening on.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };

    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    write_ports(&workspace, port);

    let src = tmp.path().join("input.txt");
    fs::write(&src, b"nowhere").unwrap();

    let out = Command::new(LUCIDOS)
        .args(["data", "write", "artifacts/nowhere.txt", "--from"])
        .arg(&src)
        .current_dir(tmp.path())
        .env("LUCIDOS_WORKSPACE", &workspace)
        .env_remove("LUCIDOS_API_BASE_URL")
        .output()
        .expect("lucidos binary should run");

    assert!(!out.status.success(), "must exit non-zero with no engine");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("engine running"),
        "must hint at engine status, got: {stderr}"
    );
}

#[test]
fn data_path_prints_resolved_path_with_normalization() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    write_ports(&workspace, 1);

    let worktree = workspace.join(".lucidos/worktrees/foo");
    fs::create_dir_all(&worktree).unwrap();

    // Loose name → prefixed with artifacts/. `data path` is a pure local path
    // helper: it contacts no engine, so the dead port above is fine.
    let out = Command::new(LUCIDOS)
        .args(["data", "path", "report.html"])
        .current_dir(&worktree)
        .env_remove("LUCIDOS_WORKSPACE")
        .env_remove("LUCIDOS_API_BASE_URL")
        .output()
        .expect("lucidos binary should run");
    assert!(out.status.success());
    let printed = String::from_utf8(out.stdout).unwrap();
    let printed = printed.trim();
    // macOS symlinks /var → /private/var, so the child's current_dir() resolves
    // through the symlink. Canonicalize the workspace root and assert the
    // printed path ends with the right suffix relative to it.
    let ws_canon = fs::canonicalize(&workspace).unwrap();
    let expected = ws_canon.join("data/artifacts/report.html");
    assert_eq!(std::path::PathBuf::from(printed), expected);
}
