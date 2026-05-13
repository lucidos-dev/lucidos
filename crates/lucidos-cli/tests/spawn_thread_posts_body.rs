//! Integration tests: `lucidos spawn-thread` POSTs the expected JSON body shape
//! to the target workspace's `/api/chat/stream`, with caller_* defaulted from
//! env, and prints a `[title](thread:ws/uuid)` markdown link on stdout.

use std::path::Path;
use std::sync::{Arc, Mutex};

type Captured = Arc<Mutex<Option<serde_json::Value>>>;

/// Spawn an axum server on a random port that captures the JSON body of any
/// POST to `/api/chat/stream` and replies with a stub `event_id`. Returns the
/// port and the shared slot the test inspects after running the CLI.
async fn start_capture_server() -> (u16, Captured) {
    let captured: Captured = Arc::new(Mutex::new(None));
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
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    (port, captured)
}

/// Write a minimal `<dir>/.lucidos/ports` file pointing at `port` so the CLI
/// can resolve the workspace and find the local engine endpoint.
fn write_ports_file(dir: &Path, port: u16) {
    std::fs::create_dir_all(dir.join(".lucidos")).unwrap();
    std::fs::write(
        dir.join(".lucidos/ports"),
        format!("API_PORT={port}\nVITE_PORT={port}\n"),
    )
    .unwrap();
}

// Multi-thread runtime: the test blocks on `Command::output()` (synchronous)
// while the spawned axum task needs progress on a separate worker — single-thread
// would deadlock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_posts_caller_fields_in_body() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("caller");
    let target = tmp.path().join("dev");
    write_ports_file(&caller, 1);
    write_ports_file(&target, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let thread_id = uuid::Uuid::new_v4().to_string();
    let event_id = uuid::Uuid::new_v4().to_string();
    let output = std::process::Command::new(bin)
        .args(["spawn-thread", "--to", "dev", "--message", "do the thing", "--title", "Test", "--insecure-http"])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", &thread_id)
        .env("LUCIDOS_EVENT_ID", &event_id)
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output().expect("spawn cli");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    let body = captured.lock().unwrap().clone().expect("server received body");
    assert_eq!(body["message"], "do the thing");
    assert_eq!(body["title"], "Test");
    assert_eq!(body["mode"], "agent");
    assert_eq!(body["caller_workspace"], "caller");
    assert_eq!(body["caller_thread_id"], thread_id);
    assert_eq!(body["caller_event_id"], event_id);
    assert!(body.get("parent_thread_id").is_none());
    assert!(body.get("spawning_event_id").is_none());
    assert!(body.get("repo_id").is_none(), "no --repo and no $LUCIDOS_REPO ⇒ repo_id absent");

    // The body must carry the same thread_id the CLI prints in the markdown
    // link — that's the whole point of generating it client-side.
    let body_thread_id = body["thread_id"].as_str().expect("thread_id in body");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.trim(),
        format!("[Test](thread:dev/{})", body_thread_id),
        "stdout must be the spawned-thread markdown link"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_with_parent_posts_parent_fields_in_body() {
    let (port, captured) = start_capture_server().await;

    // For --parent, target == caller. Single workspace.
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let thread_id = uuid::Uuid::new_v4().to_string();
    let event_id = uuid::Uuid::new_v4().to_string();
    let status = std::process::Command::new(bin)
        .args(["spawn-thread", "--parent", "--to", "dev", "--cc", "--message", "spawn cc subtask", "--title", "Sub", "--insecure-http"])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", &thread_id)
        .env("LUCIDOS_EVENT_ID", &event_id)
        .env_remove("LUCIDOS_REPO")
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
async fn spawn_thread_with_parent_rejects_different_target() {
    // --parent + --to different-workspace must error before sending anything —
    // no capture server needed because the CLI exits before any HTTP call.
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("caller");
    let other = tmp.path().join("other");
    write_ports_file(&caller, 1);
    write_ports_file(&other, 2);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = std::process::Command::new(bin)
        .args(["spawn-thread", "--parent", "--to", "other", "--message", "x", "--insecure-http"])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status().expect("spawn cli");
    assert!(!status.success(), "CLI must error on --parent with mismatched target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_explicit_repo_flag_lands_in_body() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("caller");
    let target = tmp.path().join("work");
    write_ports_file(&caller, 1);
    write_ports_file(&target, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = std::process::Command::new(bin)
        .args([
            "spawn-thread", "--to", "work", "--cc",
            "--repo", "user-acquisition",
            "--message", "fix bug", "--title", "Fix", "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        // Explicit --repo must beat $LUCIDOS_REPO.
        .env("LUCIDOS_REPO", "lucidos")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status().expect("spawn cli");
    assert!(status.success());

    let body = captured.lock().unwrap().clone().expect("server received body");
    assert_eq!(body["repo_id"], "user-acquisition", "explicit --repo wins over env var");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_defaults_repo_from_lucidos_repo_env() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("work");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = std::process::Command::new(bin)
        .args([
            "spawn-thread", "--parent", "--to", "work", "--cc",
            "--message", "sidequest", "--title", "Side", "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        // No --repo flag — must default from env.
        .env("LUCIDOS_REPO", "user-acquisition")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status().expect("spawn cli");
    assert!(status.success());

    let body = captured.lock().unwrap().clone().expect("server received body");
    assert_eq!(body["repo_id"], "user-acquisition", "$LUCIDOS_REPO is the default");
}

/// `--relation top` on a same-workspace target must produce the
/// fire-and-forget body shape (caller_* fields, no parent_*). Today's
/// `--parent` flag couples "same-workspace" with "callback"; the new
/// `--relation` flag splits the two so `--to <same-ws> --relation top`
/// becomes a valid in-engine "spawn a top-level thread" expression
/// without triggering the callback wiring.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_workspace_relation_top_omits_parent_fields() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let thread_id = uuid::Uuid::new_v4().to_string();
    let event_id = uuid::Uuid::new_v4().to_string();
    let status = std::process::Command::new(bin)
        .args([
            "spawn-thread", "--to", "dev", "--relation", "top",
            "--message", "fire and forget", "--title", "Top", "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", &thread_id)
        .env("LUCIDOS_EVENT_ID", &event_id)
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status().expect("spawn cli");
    assert!(status.success());

    let body = captured.lock().unwrap().clone().expect("server received body");
    assert!(body.get("parent_thread_id").is_none(), "top must NOT emit parent_thread_id");
    assert!(body.get("spawning_event_id").is_none(), "top must NOT emit spawning_event_id");
    assert_eq!(body["caller_workspace"], "dev");
    assert_eq!(body["caller_thread_id"], thread_id);
    assert_eq!(body["caller_event_id"], event_id);
}

/// `--relation sub` requires a same-workspace target — callbacks across
/// workspaces aren't wired. Mirrors the existing `--parent` cross-workspace
/// rejection but expressed through the new flag.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_workspace_relation_sub_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("caller");
    let other = tmp.path().join("other");
    write_ports_file(&caller, 1);
    write_ports_file(&other, 2);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = std::process::Command::new(bin)
        .args([
            "spawn-thread", "--to", "other", "--relation", "sub",
            "--message", "x", "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output().expect("spawn cli");
    assert!(!output.status.success(), "CLI must error on --relation sub with cross-workspace target");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("relation") && (stderr.contains("same-workspace") || stderr.contains("$LUCIDOS_WORKSPACE")),
        "stderr must explain that sub requires same-workspace, got: {}",
        stderr
    );
}

/// `--parent` is the deprecated alias for `--relation sub`. It must still
/// work for one release so existing recipes / scripts don't break, and
/// it must print a deprecation warning to stderr so callers know to
/// migrate. The HTTP body must look identical to a `--relation sub` call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parent_flag_still_works_with_deprecation_warning() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let thread_id = uuid::Uuid::new_v4().to_string();
    let event_id = uuid::Uuid::new_v4().to_string();
    let output = std::process::Command::new(bin)
        .args([
            "spawn-thread", "--parent", "--to", "dev", "--cc",
            "--message", "compat", "--title", "Compat", "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", &thread_id)
        .env("LUCIDOS_EVENT_ID", &event_id)
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output().expect("spawn cli");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    let body = captured.lock().unwrap().clone().expect("server received body");
    assert_eq!(body["parent_thread_id"], thread_id, "--parent must still set parent_thread_id");
    assert_eq!(body["spawning_event_id"], event_id, "--parent must still set spawning_event_id");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--parent") && stderr.contains("deprecated"),
        "must warn about --parent being deprecated, got stderr: {}",
        stderr
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_empty_repo_flag_overrides_env_to_workspace_default() {
    // `--repo ""` is the explicit "use the target workspace's default repo"
    // escape hatch — it must drop the env-var default.
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("work");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = std::process::Command::new(bin)
        .args([
            "spawn-thread", "--parent", "--to", "work", "--cc",
            "--repo", "",
            "--message", "default repo", "--title", "Def", "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_REPO", "user-acquisition")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status().expect("spawn cli");
    assert!(status.success());

    let body = captured.lock().unwrap().clone().expect("server received body");
    assert!(body.get("repo_id").is_none(), "--repo \"\" must drop the env-var default");
}
