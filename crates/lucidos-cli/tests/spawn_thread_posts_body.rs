use std::path::Path;
use std::sync::{Arc, Mutex};

type Captured = Arc<Mutex<Option<serde_json::Value>>>;

fn cli(bin: &str) -> std::process::Command {
    let mut command = std::process::Command::new(bin);
    for inherited in [
        "LUCIDOS_AGENT_ORIGIN_TOKEN",
        "LUCIDOS_API_BASE_URL",
        "LUCIDOS_THREAD_ID",
        "LUCIDOS_EVENT_ID",
    ] {
        command.env_remove(inherited);
    }
    command
}

#[test]
fn human_mode_from_agent_fails_before_workspace_resolution() {
    let tmp = tempfile::tempdir().unwrap();
    let output = cli(env!("CARGO_BIN_EXE_lucidos"))
        .args(["spawn-thread", "--to"])
        .arg(tmp.path().join("missing-workspace"))
        .args(["--message", "Start a task", "--mode", "human"])
        .env("LUCIDOS_AGENT_ORIGIN_TOKEN", "test-origin-token")
        .output()
        .expect("spawn cli");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--mode human cannot be used from an agent subprocess"),
        "{stderr}"
    );
    assert!(stderr.contains("Omit --mode"), "{stderr}");
    assert!(!stderr.contains("HTTP"), "{stderr}");
}

async fn start_capture_server() -> (u16, Captured) {
    let captured: Captured = Arc::new(Mutex::new(None));
    let cap = captured.clone();
    let app = axum::Router::new().route(
        "/api/v1/chat/stream",
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
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, captured)
}

fn write_ports_file(dir: &Path, port: u16) {
    std::fs::create_dir_all(dir.join(".lucidos")).unwrap();
    std::fs::write(
        dir.join(".lucidos/ports"),
        format!("API_PORT={port}\nVITE_PORT={port}\n"),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_to_absolute_path_with_at_sign_posts_caller_fields_in_body() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("caller");
    let target = tmp.path().join("me@example.com/workspaces/dev");
    write_ports_file(&caller, 1);
    write_ports_file(&target, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let thread_id = uuid::Uuid::new_v4().to_string();
    let event_id = uuid::Uuid::new_v4().to_string();
    let output = cli(bin)
        .args(["spawn-thread", "--to"])
        .arg(&target)
        .args([
            "--message",
            "do the thing",
            "--title",
            "Test",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", &thread_id)
        .env("LUCIDOS_EVENT_ID", &event_id)
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output()
        .expect("spawn cli");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let body = captured
        .lock()
        .unwrap()
        .clone()
        .expect("server received body");
    assert_eq!(body["message"], "do the thing");
    assert_eq!(body["title"], "Test");
    assert_eq!(body["mode"], "agent");
    assert_eq!(body["caller_workspace"], "caller");
    assert_eq!(body["caller_thread_id"], thread_id);
    assert_eq!(body["caller_event_id"], event_id);
    assert!(body.get("parent_thread_id").is_none());
    assert!(body.get("spawning_event_id").is_none());
    assert!(
        body.get("repo_id").is_none(),
        "no --repo and no $LUCIDOS_REPO ⇒ repo_id absent"
    );

    let body_thread_id = body["thread_id"].as_str().expect("thread_id in body");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.trim(),
        format!("[Test](thread:dev/{})", body_thread_id),
        "stdout must be the spawned-thread markdown link"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_relation_child_posts_parent_fields_in_body() {
    let (port, captured) = start_capture_server().await;

    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let thread_id = uuid::Uuid::new_v4().to_string();
    let event_id = uuid::Uuid::new_v4().to_string();
    let status = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--cc",
            "--relation",
            "child",
            "--message",
            "spawn cc subtask",
            "--title",
            "Sub",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", &thread_id)
        .env("LUCIDOS_EVENT_ID", &event_id)
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status()
        .expect("spawn cli");
    assert!(status.success());

    let body = captured
        .lock()
        .unwrap()
        .clone()
        .expect("server received body");
    assert_eq!(body["message"], "spawn cc subtask");
    assert_eq!(body["title"], "Sub");
    assert_eq!(body["mode"], "agent");
    assert_eq!(body["use_coding_agent"], true);
    assert_eq!(body["parent_thread_id"], thread_id);
    assert_eq!(body["spawning_event_id"], event_id);
    assert!(body.get("caller_workspace").is_none());
    assert!(body.get("caller_thread_id").is_none());
    assert!(body.get("caller_event_id").is_none());
}

/// A dev server on `[::1]` can share the engine's port, because the engine binds
/// `127.0.0.1` only. Naming `localhost` then reaches that server, which answers
/// the spawn with an empty 404.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_reaches_the_engine_when_another_server_holds_ipv6_loopback() {
    let (port, captured) = start_capture_server().await;
    let Ok(impostor) = tokio::net::TcpListener::bind(("::1", port)).await else {
        eprintln!("skipped: this host cannot bind [::1]:{port}");
        return;
    };
    tokio::spawn(async move {
        let app = axum::Router::new().fallback(|| async { axum::http::StatusCode::NOT_FOUND });
        axum::serve(impostor, app).await.unwrap();
    });

    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);
    let output = cli(env!("CARGO_BIN_EXE_lucidos"))
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--cc",
            "--relation",
            "child",
            "--message",
            "spawn cc subtask",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_EVENT_ID", uuid::Uuid::new_v4().to_string())
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output()
        .expect("spawn cli");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        captured.lock().unwrap().is_some(),
        "the engine got the spawn"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_codex_shortcut_posts_coding_agent_body() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--codex",
            "--message",
            "spawn codex subtask",
            "--title",
            "Codex",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status()
        .expect("spawn cli");
    assert!(status.success());

    let body = captured
        .lock()
        .unwrap()
        .clone()
        .expect("server received body");
    assert_eq!(body["use_coding_agent"], true);
    assert_eq!(body["coding_agent"], "codex");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_coding_agent_flag_posts_codex_body() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("caller");
    let target = tmp.path().join("dev");
    write_ports_file(&caller, 1);
    write_ports_file(&target, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--coding-agent",
            "codex",
            "--message",
            "spawn codex top",
            "--title",
            "Codex Top",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status()
        .expect("spawn cli");
    assert!(status.success());

    let body = captured
        .lock()
        .unwrap()
        .clone()
        .expect("server received body");
    assert_eq!(body["use_coding_agent"], true);
    assert_eq!(body["coding_agent"], "codex");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_explicit_repo_flag_lands_in_body() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("caller");
    let target = tmp.path().join("myws");
    write_ports_file(&caller, 1);
    write_ports_file(&target, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "myws",
            "--cc",
            "--repo",
            "example-repo",
            "--message",
            "fix bug",
            "--title",
            "Fix",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_REPO", "lucidos")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status()
        .expect("spawn cli");
    assert!(status.success());

    let body = captured
        .lock()
        .unwrap()
        .clone()
        .expect("server received body");
    assert_eq!(
        body["repo_id"], "example-repo",
        "explicit --repo wins over env var"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_defaults_repo_from_lucidos_repo_env() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("myws");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "myws",
            "--cc",
            "--message",
            "sidequest",
            "--title",
            "Side",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_REPO", "example-repo")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status()
        .expect("spawn cli");
    assert!(status.success());

    let body = captured
        .lock()
        .unwrap()
        .clone()
        .expect("server received body");
    assert_eq!(
        body["repo_id"], "example-repo",
        "$LUCIDOS_REPO is the default"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_workspace_relation_top_omits_parent_fields() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let thread_id = uuid::Uuid::new_v4().to_string();
    let event_id = uuid::Uuid::new_v4().to_string();
    let status = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--relation",
            "top",
            "--message",
            "fire and forget",
            "--title",
            "Top",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", &thread_id)
        .env("LUCIDOS_EVENT_ID", &event_id)
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status()
        .expect("spawn cli");
    assert!(status.success());

    let body = captured
        .lock()
        .unwrap()
        .clone()
        .expect("server received body");
    assert!(
        body.get("parent_thread_id").is_none(),
        "top must NOT emit parent_thread_id"
    );
    assert!(
        body.get("spawning_event_id").is_none(),
        "top must NOT emit spawning_event_id"
    );
    assert_eq!(body["caller_workspace"], "dev");
    assert_eq!(body["caller_thread_id"], thread_id);
    assert_eq!(body["caller_event_id"], event_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_workspace_relation_child_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("caller");
    let other = tmp.path().join("other");
    write_ports_file(&caller, 1);
    write_ports_file(&other, 2);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "other",
            "--relation",
            "child",
            "--message",
            "x",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output()
        .expect("spawn cli");
    assert!(
        !output.status.success(),
        "CLI must error on --relation child with cross-workspace target"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("relation")
            && (stderr.contains("same-workspace") || stderr.contains("$LUCIDOS_WORKSPACE")),
        "stderr must explain that child requires same-workspace, got: {}",
        stderr
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relation_sub_alias_still_accepted_as_child() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let thread_id = uuid::Uuid::new_v4().to_string();
    let event_id = uuid::Uuid::new_v4().to_string();
    let status = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--relation",
            "sub",
            "--message",
            "compat alias",
            "--title",
            "Alias",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", &thread_id)
        .env("LUCIDOS_EVENT_ID", &event_id)
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status()
        .expect("spawn cli");
    assert!(
        status.success(),
        "--relation sub must remain accepted as the back-compat alias"
    );

    let body = captured
        .lock()
        .unwrap()
        .clone()
        .expect("server received body");
    assert_eq!(
        body["parent_thread_id"], thread_id,
        "sub alias must wire parent_thread_id like child"
    );
    assert_eq!(
        body["spawning_event_id"], event_id,
        "sub alias must wire spawning_event_id like child"
    );
    assert!(
        body.get("caller_workspace").is_none(),
        "sub alias must not emit caller_* (those are top-only)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_with_folder_posts_folder_and_omits_repo() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--relation",
            "top",
            "--cc",
            "--folder",
            "data/apps/habit-tracker",
            "--message",
            "run a research session",
            "--title",
            "Research",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_EVENT_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_REPO", "Lucidos")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output()
        .expect("spawn cli");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let body = captured
        .lock()
        .unwrap()
        .clone()
        .expect("server received body");
    assert_eq!(body["folder"], "data/apps/habit-tracker");
    assert_eq!(body["use_coding_agent"], true);
    assert!(
        body.get("repo_id").is_none(),
        "--folder must suppress the $LUCIDOS_REPO repo_id default (engine 400s on both)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_folder_and_repo_are_mutually_exclusive() {
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, 1);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--relation",
            "top",
            "--cc",
            "--folder",
            "data/apps/foo",
            "--repo",
            "Lucidos",
            "--message",
            "x",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output()
        .expect("spawn cli");
    assert!(
        !output.status.success(),
        "CLI must reject --folder together with --repo"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("folder") && stderr.contains("repo"),
        "stderr must explain the --folder/--repo conflict, got: {}",
        stderr
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_folder_requires_cc() {
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, 1);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--relation",
            "top",
            "--folder",
            "data/apps/foo",
            "--message",
            "x",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output()
        .expect("spawn cli");
    assert!(
        !output.status.success(),
        "CLI must reject --folder without --cc"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--folder") && stderr.contains("--cc"),
        "stderr must explain that --folder requires --cc, got: {}",
        stderr
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_reasoning_effort_lands_in_body() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--coding-agent",
            "claude-code",
            "--reasoning-effort",
            "max",
            "--message",
            "think hard",
            "--title",
            "Ideation",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output()
        .expect("spawn cli");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let body = captured
        .lock()
        .unwrap()
        .clone()
        .expect("server received body");
    assert_eq!(body["reasoning_effort"], "max");
    assert_eq!(body["use_coding_agent"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_omits_reasoning_effort_when_not_asked() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--codex",
            "--message",
            "no pin",
            "--title",
            "Plain",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status()
        .expect("spawn cli");
    assert!(status.success());

    let body = captured
        .lock()
        .unwrap()
        .clone()
        .expect("server received body");
    assert!(
        body.get("reasoning_effort").is_none(),
        "no --reasoning-effort ⇒ the field is absent, not null"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_rejects_an_unknown_reasoning_effort() {
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, 1);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--cc",
            "--reasoning-effort",
            "ludicrous",
            "--message",
            "x",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output()
        .expect("spawn cli");
    assert!(
        !output.status.success(),
        "CLI must reject a level no backend menu offers"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ludicrous") && stderr.contains("xhigh") && stderr.contains("max"),
        "stderr must name the rejected value and the accepted levels, got: {}",
        stderr
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_rejects_the_chat_only_none_level() {
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, 1);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--cc",
            "--reasoning-effort",
            "none",
            "--message",
            "x",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output()
        .expect("spawn cli");
    assert!(
        !output.status.success(),
        "`none` is a chat tier and no coding-agent menu offers it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_reasoning_effort_requires_a_coding_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, 1);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--reasoning-effort",
            "max",
            "--message",
            "x",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output()
        .expect("spawn cli");
    assert!(
        !output.status.success(),
        "CLI must reject --reasoning-effort on a chat-thread spawn"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--reasoning-effort") && stderr.contains("--coding-agent"),
        "stderr must name the flag and what satisfies it, got: {}",
        stderr
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_rejects_an_effort_the_named_model_lacks() {
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, 1);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--coding-agent",
            "codex",
            "--coding-agent-model",
            "gpt-5.5",
            "--reasoning-effort",
            "max",
            "--message",
            "x",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output()
        .expect("spawn cli");
    assert!(
        !output.status.success(),
        "CLI must reject max on a Codex model that does not offer it"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("gpt-5.6-sol"),
        "stderr must name a model that does offer max, got: {}",
        stderr
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_rejects_codex_max_with_no_model_named() {
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, 1);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--codex",
            "--reasoning-effort",
            "max",
            "--message",
            "x",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output()
        .expect("spawn cli");
    assert!(
        !output.status.success(),
        "codex max with no model always drops, so the CLI must refuse it"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--coding-agent-model"),
        "stderr must say what to pass, got: {}",
        stderr
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_allows_codex_xhigh_with_no_model_named() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--codex",
            "--reasoning-effort",
            "xhigh",
            "--message",
            "unrestricted level",
            "--title",
            "XHigh",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output()
        .expect("spawn cli");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let body = captured
        .lock()
        .unwrap()
        .clone()
        .expect("server received body");
    assert_eq!(body["reasoning_effort"], "xhigh");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coding_agent_model_flag_sends_cc_model() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--cc",
            "--coding-agent-model",
            "opus",
            "--message",
            "pick a model",
            "--title",
            "Model",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output()
        .expect("spawn cli");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let body = captured
        .lock()
        .unwrap()
        .clone()
        .expect("server received body");
    assert_eq!(body["cc_model"], "opus");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_empty_repo_flag_overrides_env_to_workspace_default() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("myws");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = cli(bin)
        .args([
            "spawn-thread",
            "--to",
            "myws",
            "--cc",
            "--repo",
            "",
            "--message",
            "default repo",
            "--title",
            "Def",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_REPO", "example-repo")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status()
        .expect("spawn cli");
    assert!(status.success());

    let body = captured
        .lock()
        .unwrap()
        .clone()
        .expect("server received body");
    assert!(
        body.get("repo_id").is_none(),
        "--repo \"\" must drop the env-var default"
    );
}

type Requests = Arc<Mutex<Vec<(String, Option<String>, serde_json::Value)>>>;

async fn start_engine(status: u16, answer: serde_json::Value) -> (u16, Requests) {
    let requests: Requests = Arc::default();
    let record = requests.clone();
    let app = axum::Router::new().fallback(
        move |uri: axum::http::Uri,
              headers: axum::http::HeaderMap,
              body: axum::Json<serde_json::Value>| {
            let record = record.clone();
            let answer = answer.clone();
            async move {
                let asserted = headers
                    .get("x-lucidos-target-workspace")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                record
                    .lock()
                    .unwrap()
                    .push((uri.path().to_string(), asserted, body.0));
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    axum::Json(answer),
                )
            }
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, requests)
}

fn spawn_from_a_thread(tmp: &Path, caller: &Path, own_port: u16) -> std::process::Output {
    cli(env!("CARGO_BIN_EXE_lucidos"))
        .args([
            "spawn-thread",
            "--to",
            "dev",
            "--message",
            "do the thing",
            "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", caller)
        .env("LUCIDOS_AGENT_ORIGIN_TOKEN", "a-thread-token")
        .env(
            "LUCIDOS_API_BASE_URL",
            format!("http://127.0.0.1:{own_port}"),
        )
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp)
        .current_dir(caller)
        .output()
        .expect("spawn cli")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_threads_spawn_into_another_workspace_goes_through_its_own_engine() {
    let (own_port, own) = start_engine(200, serde_json::json!({"event_id": "e"})).await;
    let (target_port, target) = start_engine(200, serde_json::json!({"event_id": "e"})).await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("caller");
    write_ports_file(&caller, own_port);
    write_ports_file(&tmp.path().join("dev"), target_port);

    let output = spawn_from_a_thread(tmp.path(), &caller, own_port);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let own = own.lock().unwrap().clone();
    assert_eq!(own.len(), 1, "{own:?}");
    let (path, asserted, body) = &own[0];
    assert_eq!(path, "/api/v1/workspace-calls/dev/chat/stream");
    assert_eq!(
        asserted.as_deref(),
        Some("caller"),
        "the request is for the caller's own engine"
    );
    assert_eq!(body["message"], "do the thing");
    assert!(target.lock().unwrap().is_empty());
    assert!(String::from_utf8_lossy(&output.stdout).contains("(thread:dev/"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_no_gateway_the_spawn_posts_to_the_target_directly() {
    let (own_port, own) = start_engine(
        409,
        serde_json::json!({"error": "no", "reason": "no_gateway"}),
    )
    .await;
    let (target_port, target) = start_engine(200, serde_json::json!({"event_id": "e"})).await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("caller");
    write_ports_file(&caller, own_port);
    write_ports_file(&tmp.path().join("dev"), target_port);

    let output = spawn_from_a_thread(tmp.path(), &caller, own_port);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(own.lock().unwrap().len(), 1);
    let target = target.lock().unwrap().clone();
    assert_eq!(target.len(), 1, "{target:?}");
    assert_eq!(target[0].0, "/api/v1/chat/stream");
    assert_eq!(target[0].1.as_deref(), Some("dev"));
    assert_eq!(target[0].2["caller_workspace"], "caller");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_refusal_from_the_own_engine_is_the_answer() {
    let (own_port, _) =
        start_engine(403, serde_json::json!({"error": "stamp did not verify"})).await;
    let (target_port, target) = start_engine(200, serde_json::json!({"event_id": "e"})).await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("caller");
    write_ports_file(&caller, own_port);
    write_ports_file(&tmp.path().join("dev"), target_port);

    let output = spawn_from_a_thread(tmp.path(), &caller, own_port);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("stamp did not verify"));
    assert!(target.lock().unwrap().is_empty());
}
