//! E2E test for the `lucidos` CLI against a running Lucidos workspace.
//!
//! Verifies the headline use case from the spec: an `events emit` from a
//! Claude-Code-shaped subprocess (worktree subdir, `LUCIDOS_WORKSPACE` set) is
//! observable via `events query` immediately afterwards.

use crate::support::{unique_marker, workspace_path};
use std::process::Command;

fn lucidos_bin() -> std::path::PathBuf {
    // Integration tests live at target/<profile>/deps/<test-binary>. The
    // sibling `lucidos` CLI binary sits at target/<profile>/lucidos. We can
    // find it by walking up two directories from the current test executable.
    let test_exe = std::env::current_exe().expect("current_exe");
    let target_profile_dir = test_exe
        .parent()
        .and_then(|p| p.parent())
        .expect("test exe must live under target/<profile>/deps/");
    let bin = target_profile_dir.join("lucidos");
    assert!(
        bin.exists(),
        "lucidos CLI binary not found at {}. Run `cargo build -p lucidos-cli` first.",
        bin.display()
    );
    bin
}

/// The CLI, aimed at the e2e workspace the way an engine-spawned session is.
///
/// Every CLI call in the suite goes through here. A suite run inside a
/// coding-agent session inherits that session's own `LUCIDOS_API_BASE_URL` and
/// origin token. The CLI honours both over the workspace it is handed. Left in
/// place, the call reaches the PARENT engine, and a token-less test carries a
/// real token. Clearing them here leaves the CLI resolving the target from
/// `<workspace>/.lucidos/ports`, speaking for nobody.
pub(crate) fn lucidos_cmd() -> Command {
    let mut cmd = Command::new(lucidos_bin());
    cmd.env("LUCIDOS_WORKSPACE", workspace_path())
        .env_remove("LUCIDOS_API_BASE_URL")
        .env_remove("LUCIDOS_AGENT_ORIGIN_TOKEN");
    cmd
}

#[test]
fn emit_then_query_round_trip_against_running_workspace() {
    let event_type = "LucidosCliE2eEmitted";
    let summary = unique_marker("cli-e2e");

    let payload = serde_json::json!({ "marker": summary }).to_string();

    // Emit
    let emit = lucidos_cmd()
        .args([
            "events",
            "emit",
            event_type,
            "--summary",
            &summary,
            "--payload",
        ])
        .arg(&payload)
        .output()
        .expect("lucidos events emit should run");
    assert!(
        emit.status.success(),
        "emit failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&emit.stdout),
        String::from_utf8_lossy(&emit.stderr)
    );

    // Query and find our marker
    let query = lucidos_cmd()
        .args(["events", "query", "--type", event_type, "--limit", "10"])
        .output()
        .expect("lucidos events query should run");
    assert!(
        query.status.success(),
        "query failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&query.stdout),
        String::from_utf8_lossy(&query.stderr)
    );

    let body: serde_json::Value =
        serde_json::from_slice(&query.stdout).expect("query stdout must be JSON");
    let arr = body.as_array().expect("query response must be an array");

    let found = arr.iter().any(|ev| {
        ev["payload"]["summary"].as_str() == Some(summary.as_str())
            && ev["payload"]["marker"].as_str() == Some(summary.as_str())
    });
    assert!(
        found,
        "Did not find emitted event with marker {summary} in query response: {}",
        serde_json::to_string_pretty(&body).unwrap()
    );
}

#[test]
fn emit_rejects_missing_summary() {
    let out = lucidos_cmd()
        .args([
            "events",
            "emit",
            "LucidosCliE2eRejected",
            "--payload",
            "{\"foo\": 1}",
        ])
        .output()
        .expect("lucidos should run");
    assert!(!out.status.success(), "emit without summary must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("summary"),
        "stderr should mention summary: {stderr}"
    );
}

/// Regression for the bug where `harden.md` Phase 0 read a filesystem marker
/// no current code wrote (post-DB-migration), causing CC to falsely report
/// `ALREADY_HARDENED` after engine restart interruption. The skill now reads
/// state via `lucidos hardened query`; this round-trip proves the CLI surface
/// reports `MISSING → FRESH → STALE` correctly against the real engine.
#[test]
fn hardened_query_round_trip() {
    // Throwaway git repo so we don't pollute the e2e workspace's hardened_branches
    // rows for actual branches. Unique branch name keeps state isolated across runs.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    let branch = format!("cli-test-{}", unique_marker("hardened"));

    run_git(repo, &["init", "-q", "-b", "main"]);
    run_git(repo, &["config", "user.email", "test@example.com"]);
    run_git(repo, &["config", "user.name", "test"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    run_git(repo, &["add", "."]);
    run_git(repo, &["commit", "-q", "-m", "first"]);
    run_git(repo, &["checkout", "-q", "-b", &branch]);

    // 1) MISSING — nothing recorded yet for this (repo, branch).
    let out = lucidos_cmd()
        .args(["hardened", "query"])
        .current_dir(repo)
        .output()
        .expect("lucidos hardened query should run");
    assert!(
        out.status.success(),
        "query failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "MISSING",
        "fresh branch must report MISSING"
    );

    // 2) Mark — should record current HEAD.
    let mark = lucidos_cmd()
        .args(["hardened", "mark"])
        .current_dir(repo)
        .output()
        .expect("lucidos hardened mark should run");
    assert!(
        mark.status.success(),
        "mark failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&mark.stdout),
        String::from_utf8_lossy(&mark.stderr)
    );

    // 3) FRESH — HEAD still matches the just-recorded SHA.
    let out = lucidos_cmd()
        .args(["hardened", "query"])
        .current_dir(repo)
        .output()
        .expect("lucidos hardened query should run");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "FRESH",
        "after mark, query must report FRESH"
    );

    // 4) STALE — advance HEAD; recorded SHA no longer matches.
    std::fs::write(repo.join("b.txt"), "b").unwrap();
    run_git(repo, &["add", "."]);
    run_git(repo, &["commit", "-q", "-m", "second"]);

    let out = lucidos_cmd()
        .args(["hardened", "query"])
        .current_dir(repo)
        .output()
        .expect("lucidos hardened query should run");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "STALE",
        "after new commit, query must report STALE"
    );
}

/// `lucidos changes list` round-trip: the CLI GETs `/api/v1/changes` and
/// echoes the payload verbatim. This is the command the nightly pipeline
/// repeatedly guessed (and got "unrecognized subcommand 'list'") when looking
/// for the pending change id before `apply`. Asserting the shape proves the
/// subcommand exists and surfaces the `pending` array a caller reads ids from.
#[test]
fn changes_list_returns_expected_shape() {
    let out = lucidos_cmd()
        .args(["changes", "list"])
        .output()
        .expect("lucidos changes list should run");
    assert!(
        out.status.success(),
        "changes list failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let body: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("changes list stdout must be JSON");
    assert!(
        body["pending"].is_array(),
        "response must carry a `pending` array (the id source for `apply`): {}",
        serde_json::to_string_pretty(&body).unwrap()
    );
    assert!(
        body["applied"].is_array(),
        "response must carry an `applied` array: {}",
        serde_json::to_string_pretty(&body).unwrap()
    );
    assert!(
        body["total_pending"].is_u64(),
        "response must carry a numeric `total_pending`: {}",
        serde_json::to_string_pretty(&body).unwrap()
    );
}

fn run_git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git {} failed: {}", args.join(" "), e));
    assert!(
        out.status.success(),
        "git {} in {} failed: {}",
        args.join(" "),
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `lucidos notify` round-trip: CLI POSTs `/api/v1/notifications`, the engine
/// produces a `NotificationCreated` event AND a notification row that
/// `GET /api/v1/notifications` can return. Mirrors the
/// `emit_then_query_round_trip_against_running_workspace` style.
#[tokio::test]
async fn notify_creates_inbox_notification_against_running_workspace() {
    // Unique title so we can find this exact notification regardless of any
    // background notifications the e2e workspace produces (auto-cleanup,
    // backup-failure, etc.).
    let title = unique_marker("cli-notify-title");
    let message = unique_marker("cli-notify-message");

    let out = lucidos_cmd()
        .args(["notify", "--title", &title, "--message", &message])
        .output()
        .expect("lucidos notify should run");
    assert!(
        out.status.success(),
        "notify failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let body: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("notify stdout must be JSON");
    assert_eq!(body["success"], serde_json::json!(true));
    let returned_id = body["notification_id"]
        .as_str()
        .expect("notification_id must be a string");
    let returned_id = uuid::Uuid::parse_str(returned_id).expect("notification_id must be a UUID");

    // Verify via the public list endpoint that the notification is in the
    // inbox. The CLI's returned id is the source of truth — match by id, not
    // by title scan, so a colliding marker can never produce a false positive.
    let client = crate::support::http_client();
    let url = format!(
        "{}/api/v1/notifications?limit=100",
        crate::support::base_url()
    );
    let resp: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("GET /api/v1/notifications failed")
        .json()
        .await
        .expect("notifications response must be JSON");

    let items = resp["notifications"]
        .as_array()
        .expect("notifications must be an array");
    let found = items
        .iter()
        .find(|n| n["id"].as_str() == Some(returned_id.to_string().as_str()))
        .unwrap_or_else(|| {
            panic!(
                "notification {returned_id} not found in inbox; got: {}",
                serde_json::to_string_pretty(&resp).unwrap()
            )
        });
    assert_eq!(found["title"].as_str(), Some(title.as_str()));
    assert_eq!(found["message"].as_str(), Some(message.as_str()));
}

#[test]
fn notify_rejects_missing_title() {
    let out = lucidos_cmd()
        .args(["notify", "--message", "no title here"])
        .output()
        .expect("lucidos should run");
    assert!(!out.status.success(), "notify without --title must fail");
}

#[test]
fn notify_rejects_missing_message() {
    let out = lucidos_cmd()
        .args(["notify", "--title", "no body here"])
        .output()
        .expect("lucidos should run");
    assert!(!out.status.success(), "notify without --message must fail");
}

/// Server-side validation: an empty title makes it past clap (it accepts the
/// flag with an empty value) and must be rejected by the engine with 400.
#[test]
fn notify_rejects_empty_title_at_engine() {
    let out = lucidos_cmd()
        .args(["notify", "--title", "", "--message", "msg"])
        .output()
        .expect("lucidos should run");
    assert!(
        !out.status.success(),
        "notify with empty --title must fail; stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Direct-POST regression for the whitespace-title bypass: the engine must
/// trim before deciding, so a title of `"   "` is rejected as a 400 even if
/// some future client fails to normalize. Goes through the HTTP route
/// directly because the CLI does its own trim-check before posting.
#[tokio::test]
async fn notify_endpoint_rejects_whitespace_only_title() {
    let client = crate::support::user_client().await;
    let url = format!("{}/api/v1/notifications", crate::support::base_url());
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "title": "   ",
            "message": "ok",
        }))
        .send()
        .await
        .expect("POST /api/v1/notifications failed");
    assert_eq!(
        resp.status().as_u16(),
        400,
        "whitespace-only title must be rejected with 400; body={}",
        resp.text().await.unwrap_or_default()
    );
}

/// `lucidos data write` must ANNOUNCE the write, not just land the bytes.
///
/// It used to `std::fs::write` the file directly: no `DataFileWritten`, no
/// `Artifact*`, no git commit, no memory index entry. The frontend's artifact
/// cache is refreshed by those events, so a freshly written artifact was
/// unknown to it, and the chat link this very command prints then failed to
/// resolve and reloaded the whole workspace on click. The write now goes
/// through `PUT /api/v1/data/*path` (ADR 0032), and this proves the events
/// really reach the store, which the CLI's own stub-server test cannot.
#[test]
fn data_write_announces_the_artifact_against_a_running_workspace() {
    let ws = workspace_path();
    let marker = unique_marker("cli-data-write");
    let rel = format!("artifacts/cli-e2e/{marker}.md");

    let src = std::env::temp_dir().join(format!("{marker}.md"));
    std::fs::write(&src, b"# announced\n").expect("write source file");

    // The write lands a new file in the shared working tree, which a command
    // checkpoint images whole; see `workspace_tree_lock`. `blocking_read` is
    // the right call here and only here: this is a plain `#[test]`, so there is
    // no async runtime to park.
    let _tree = crate::support::workspace_tree_lock().blocking_read();
    let out = lucidos_cmd()
        .args(["data", "write", &rel, "--from"])
        .arg(&src)
        .output()
        .expect("lucidos data write should run");
    let _ = std::fs::remove_file(&src);
    assert!(
        out.status.success(),
        "data write failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // stdout is the ready-to-paste chat link, bare store path, no scheme.
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("[{marker}.md]({rel})")
    );

    // The bytes landed in the PARENT workspace, via the engine.
    let landed = ws.join("data").join(&rel);
    assert!(
        landed.exists(),
        "expected the file at {}, but it does not exist",
        landed.display()
    );

    // The entity event fired, which is what refreshes the artifact list, feeds
    // the memory index, and lets an `on_event: ArtifactCreated` trigger see it.
    let artifact_rel = rel.strip_prefix("artifacts/").expect("artifacts-rooted");
    assert!(
        query_system_event_mentions("ArtifactCreated", "artifact_path", artifact_rel),
        "no ArtifactCreated carrying {artifact_rel}"
    );

    // And the API-origin audit event alongside it, carrying the store path.
    assert!(
        query_system_event_mentions("DataFileWritten", "path", &rel),
        "no DataFileWritten carrying {rel}"
    );
}

/// True when a recent SYSTEM event of `event_type` carries `field == value`.
///
/// A system event's stored payload is the serde-tagged envelope
/// (`{"type": "ArtifactCreated", "data": {…}}`), so the variant's own fields
/// live under `data`. That is unlike a domain event from `events emit`, whose
/// payload is the caller's object flat at the top level (see
/// `emit_then_query_round_trip_against_running_workspace`, which reads
/// `payload.summary` directly).
fn query_system_event_mentions(event_type: &str, field: &str, value: &str) -> bool {
    let out = lucidos_cmd()
        .args(["events", "query", "--type", event_type, "--limit", "50"])
        .output()
        .expect("lucidos events query should run");
    assert!(
        out.status.success(),
        "query {event_type} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("query stdout must be JSON");
    body.as_array()
        .expect("query response must be an array")
        .iter()
        .any(|ev| ev["payload"]["data"][field].as_str() == Some(value))
}

/// One request, captured whole: the port to aim at, and the raw text it sent.
///
/// Answers 200 with a JSON body so the CLI's own success path runs. Reads the
/// headers, then the body its `Content-Length` announces, so the client is
/// never reset mid-write.
fn spawn_capturing_server() -> (u16, std::sync::mpsc::Receiver<String>) {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
    let port = listener.local_addr().expect("local_addr").port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept the CLI's connection");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .expect("set a read timeout");
        let mut raw: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        while !request_is_complete(&raw) {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => raw.extend_from_slice(&buf[..n]),
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&raw).into_owned());
        let body = b"{\"success\":true}";
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(body);
    });
    (port, rx)
}

/// True once `raw` holds the headers and the body they announce.
fn request_is_complete(raw: &[u8]) -> bool {
    let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&raw[..end]).to_lowercase();
    let announced = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    raw.len() >= end + 4 + announced
}

/// The captured request's headers, as the engine's resolver reads them.
fn captured_headers(request: &str) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    for line in request.lines().skip(1) {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if let (Ok(name), Ok(value)) = (
            axum::http::HeaderName::from_bytes(name.trim().as_bytes()),
            axum::http::HeaderValue::from_str(value.trim()),
        ) {
            headers.insert(name, value);
        }
    }
    headers
}

/// The CLI's hand-copied origin-token names, proven against the engine's own.
///
/// `crates/lucidos-cli/src/http.rs` restates `ENV_AGENT_ORIGIN_TOKEN` and
/// `HEADER_AGENT_ORIGIN_TOKEN` because the CLI cannot depend on the engine
/// crate. Rename one side alone and every `lucidos` call from a coding-agent
/// session quietly loses its Agent attribution, with every test still green.
///
/// The token here is minted through the engine library, so it is signed rather
/// than shaped. The binary under test is the real CLI, spawned with the env var
/// the ENGINE names. A stub server captures what it sent, and the engine's own
/// `build_message_origin` reads those headers back.
///
/// It aims at the stub, not the workspace. The running engine's HMAC key is per
/// startup and lives in its memory, so no outside process can mint a token it
/// would verify. `follow_up_test.rs` records the same limit.
#[test]
fn the_cli_carries_the_origin_token_the_engine_minted() {
    use lucidos_engine::api::actor::{
        build_message_origin, init_agent_origin_secret, mint_agent_origin_token,
        ENV_AGENT_ORIGIN_TOKEN, HEADER_AGENT_ORIGIN_TOKEN,
    };
    use lucidos_engine::engine::thread_events::{ActorMode, MessageOrigin};

    let source_thread = uuid::Uuid::new_v4();
    // First writer wins, so a secret another test installed signs this one too,
    // and the verify below reads whichever is installed.
    init_agent_origin_secret("lucidos-cli-origin-mirror".to_string());
    let token = mint_agent_origin_token(Some(source_thread), 0, None)
        .expect("a secret is installed, so a token mints");

    let (port, captured) = spawn_capturing_server();
    let summary = unique_marker("cli-origin");
    let out = Command::new(lucidos_bin())
        .env("LUCIDOS_WORKSPACE", workspace_path())
        .env("LUCIDOS_API_BASE_URL", format!("http://127.0.0.1:{port}"))
        .env(ENV_AGENT_ORIGIN_TOKEN, &token)
        .args([
            "events",
            "emit",
            "LucidosCliOriginProbed",
            "--payload",
            "{}",
            "--summary",
            &summary,
        ])
        .output()
        .expect("lucidos events emit should run");
    assert!(
        out.status.success(),
        "emit against the capturing server failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let request = captured
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the CLI must reach the capturing server");
    assert!(
        request.to_lowercase().contains(HEADER_AGENT_ORIGIN_TOKEN),
        "the CLI sent no {HEADER_AGENT_ORIGIN_TOKEN} header, so the engine would \
         stamp its mutations as somebody else. Request was:\n{request}"
    );

    let origin = build_message_origin(
        &captured_headers(&request),
        ActorMode::Human,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    match origin {
        Some(MessageOrigin::Api {
            mode,
            source_thread_id,
            ..
        }) => {
            assert_eq!(
                mode,
                ActorMode::Agent,
                "a token-bearing CLI call must stamp as the agent, never as the user"
            );
            assert_eq!(
                source_thread_id,
                Some(source_thread),
                "the spawning thread must survive the hop through the CLI"
            );
        }
        other => panic!("expected Api {{ mode: Agent }}, got {other:?}; request was:\n{request}"),
    }
}
