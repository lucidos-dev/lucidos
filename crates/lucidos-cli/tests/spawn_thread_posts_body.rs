//! Integration tests: `lucidos spawn-thread` POSTs the expected JSON body shape
//! to the target workspace's `/api/v1/chat/stream`, with caller_* defaulted from
//! env, and prints a `[title](thread:ws/uuid)` markdown link on stdout.

use std::path::Path;
use std::sync::{Arc, Mutex};

type Captured = Arc<Mutex<Option<serde_json::Value>>>;

/// Spawn an axum server on a random port that captures the JSON body of any
/// POST to `/api/v1/chat/stream` and replies with a stub `event_id`. Returns the
/// port and the shared slot the test inspects after running the CLI.
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
    assert_eq!(body["use_coding_agent"], true);
    assert_eq!(body["parent_thread_id"], thread_id);
    assert_eq!(body["spawning_event_id"], event_id);
    assert!(body.get("caller_workspace").is_none());
    assert!(body.get("caller_thread_id").is_none());
    assert!(body.get("caller_event_id").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_codex_shortcut_posts_coding_agent_body() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = std::process::Command::new(bin)
        .args(["spawn-thread", "--to", "dev", "--codex", "--message", "spawn codex subtask", "--title", "Codex", "--insecure-http"])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status().expect("spawn cli");
    assert!(status.success());

    let body = captured.lock().unwrap().clone().expect("server received body");
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
    let status = std::process::Command::new(bin)
        .args(["spawn-thread", "--to", "dev", "--coding-agent", "codex", "--message", "spawn codex top", "--title", "Codex Top", "--insecure-http"])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status().expect("spawn cli");
    assert!(status.success());

    let body = captured.lock().unwrap().clone().expect("server received body");
    assert_eq!(body["use_coding_agent"], true);
    assert_eq!(body["coding_agent"], "codex");
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
    let target = tmp.path().join("myws");
    write_ports_file(&caller, 1);
    write_ports_file(&target, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = std::process::Command::new(bin)
        .args([
            "spawn-thread", "--to", "myws", "--cc",
            "--repo", "example-repo",
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
    assert_eq!(body["repo_id"], "example-repo", "explicit --repo wins over env var");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_defaults_repo_from_lucidos_repo_env() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("myws");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = std::process::Command::new(bin)
        .args([
            "spawn-thread", "--parent", "--to", "myws", "--cc",
            "--message", "sidequest", "--title", "Side", "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        // No --repo flag — must default from env.
        .env("LUCIDOS_REPO", "example-repo")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status().expect("spawn cli");
    assert!(status.success());

    let body = captured.lock().unwrap().clone().expect("server received body");
    assert_eq!(body["repo_id"], "example-repo", "$LUCIDOS_REPO is the default");
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

/// `--relation child` requires a same-workspace target — callbacks across
/// workspaces aren't wired. Mirrors the existing `--parent` cross-workspace
/// rejection but expressed through the new flag.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_workspace_relation_child_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("caller");
    let other = tmp.path().join("other");
    write_ports_file(&caller, 1);
    write_ports_file(&other, 2);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = std::process::Command::new(bin)
        .args([
            "spawn-thread", "--to", "other", "--relation", "child",
            "--message", "x", "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output().expect("spawn cli");
    assert!(!output.status.success(), "CLI must error on --relation child with cross-workspace target");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("relation") && (stderr.contains("same-workspace") || stderr.contains("$LUCIDOS_WORKSPACE")),
        "stderr must explain that child requires same-workspace, got: {}",
        stderr
    );
}

/// `--relation sub` is a back-compat alias for `--relation child` — clap's
/// `#[value(alias = "sub")]` keeps older recipes working after the glossary
/// canonicalized on *child thread* (direct descendant) vs *sub-thread*
/// (transitive). The CLI must accept it AND apply the same same-workspace
/// callback semantics, otherwise the alias is a lie.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relation_sub_alias_still_accepted_as_child() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let thread_id = uuid::Uuid::new_v4().to_string();
    let event_id = uuid::Uuid::new_v4().to_string();
    let status = std::process::Command::new(bin)
        .args([
            "spawn-thread", "--to", "dev", "--relation", "sub",
            "--message", "compat alias", "--title", "Alias", "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", &thread_id)
        .env("LUCIDOS_EVENT_ID", &event_id)
        .env_remove("LUCIDOS_REPO")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status().expect("spawn cli");
    assert!(status.success(), "--relation sub must remain accepted as the back-compat alias");

    let body = captured.lock().unwrap().clone().expect("server received body");
    assert_eq!(body["parent_thread_id"], thread_id, "sub alias must wire parent_thread_id like child");
    assert_eq!(body["spawning_event_id"], event_id, "sub alias must wire spawning_event_id like child");
    assert!(body.get("caller_workspace").is_none(), "sub alias must not emit caller_* (those are top-only)");
}

/// `--parent` is the deprecated alias for `--relation child`. It must still
/// work for one release so existing recipes / scripts don't break, and
/// it must print a deprecation warning to stderr so callers know to
/// migrate. The HTTP body must look identical to a `--relation child` call.
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

/// `--folder data/apps/<id> --cc` must POST a `folder` field plus
/// `use_coding_agent` so the engine spawns an *app coding-agent thread* (the
/// same kind `run_coding_agent(folder=…)` produces). Critically, `--folder` must
/// SUPPRESS the `$LUCIDOS_REPO` `repo_id` default: the engine sets that env
/// var on every CC subprocess, and it rejects a request carrying both
/// `repo_id` and `folder`. The original bug was every spawn landing in the
/// engine's `Lucidos` repo because the env default always won.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_with_folder_posts_folder_and_omits_repo() {
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = std::process::Command::new(bin)
        .args([
            "spawn-thread", "--to", "dev", "--relation", "top", "--cc",
            "--folder", "data/apps/habit-tracker",
            "--message", "run a research session", "--title", "Research", "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_EVENT_ID", uuid::Uuid::new_v4().to_string())
        // The engine sets $LUCIDOS_REPO on every subprocess — --folder must
        // win so the body does NOT carry both repo_id and folder.
        .env("LUCIDOS_REPO", "Lucidos")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output().expect("spawn cli");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    let body = captured.lock().unwrap().clone().expect("server received body");
    assert_eq!(body["folder"], "data/apps/habit-tracker");
    assert_eq!(body["use_coding_agent"], true);
    assert!(
        body.get("repo_id").is_none(),
        "--folder must suppress the $LUCIDOS_REPO repo_id default (engine 400s on both)"
    );
}

/// `--folder` and `--repo` describe incompatible worktree targets (app folder
/// vs registered repo). Passing both must error before any HTTP round-trip —
/// no capture server needed because clap rejects the conflict at parse time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_folder_and_repo_are_mutually_exclusive() {
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, 1);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = std::process::Command::new(bin)
        .args([
            "spawn-thread", "--to", "dev", "--relation", "top", "--cc",
            "--folder", "data/apps/foo", "--repo", "Lucidos",
            "--message", "x", "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output().expect("spawn cli");
    assert!(!output.status.success(), "CLI must reject --folder together with --repo");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("folder") && stderr.contains("repo"),
        "stderr must explain the --folder/--repo conflict, got: {}",
        stderr
    );
}

/// `--folder` targets an app coding-agent thread, which only exists for CC
/// sessions. `--folder` without `--cc` must error with a clear message before
/// sending anything — the engine would 400 anyway (it rejects `folder`
/// without `use_coding_agent`), but a client-side check is clearer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_folder_requires_cc() {
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("dev");
    write_ports_file(&caller, 1);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let output = std::process::Command::new(bin)
        .args([
            "spawn-thread", "--to", "dev", "--relation", "top",
            "--folder", "data/apps/foo",
            "--message", "x", "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .output().expect("spawn cli");
    assert!(!output.status.success(), "CLI must reject --folder without --cc");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--folder") && stderr.contains("--cc"),
        "stderr must explain that --folder requires --cc, got: {}",
        stderr
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_thread_empty_repo_flag_overrides_env_to_workspace_default() {
    // `--repo ""` is the explicit "use the target workspace's default repo"
    // escape hatch — it must drop the env-var default.
    let (port, captured) = start_capture_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let caller = tmp.path().join("myws");
    write_ports_file(&caller, port);

    let bin = env!("CARGO_BIN_EXE_lucidos");
    let status = std::process::Command::new(bin)
        .args([
            "spawn-thread", "--parent", "--to", "myws", "--cc",
            "--repo", "",
            "--message", "default repo", "--title", "Def", "--insecure-http",
        ])
        .env("LUCIDOS_WORKSPACE", &caller)
        .env("LUCIDOS_THREAD_ID", uuid::Uuid::new_v4().to_string())
        .env("LUCIDOS_REPO", "example-repo")
        .env("LUCIDOS_WORKSPACES_ROOT", tmp.path())
        .current_dir(&caller)
        .status().expect("spawn cli");
    assert!(status.success());

    let body = captured.lock().unwrap().clone().expect("server received body");
    assert!(body.get("repo_id").is_none(), "--repo \"\" must drop the env-var default");
}
