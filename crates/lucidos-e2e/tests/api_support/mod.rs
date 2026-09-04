//! Shared harness for E2E API integration tests.

use std::fs;
use std::path::{Path, PathBuf};

/// Minimal valid PNG (signature + IHDR start). Enough for the engine's
/// magic-byte sniff to recognize image/png; not a real renderable image.
pub fn png_bytes() -> Vec<u8> {
    vec![
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00,
    ]
}

/// Encode an in-memory JPEG of given dimensions — needed by tests that
/// exercise the preview downscale path against a real-sized image, since
/// `png_bytes()` produces a 1×1 image that's already below the preview cap.
pub fn encoded_jpeg(width: u32, height: u32) -> Vec<u8> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ColorType, ImageEncoder};
    let pixels = vec![128u8; (width * height * 3) as usize];
    let mut buf: Vec<u8> = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, 70)
        .write_image(&pixels, width, height, ColorType::Rgb8.into())
        .expect("encode jpeg");
    buf
}

pub fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize()
        .iter()
        .fold(String::with_capacity(64), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{:02x}", b);
            acc
        })
}

fn read_ports() -> (u16, String) {
    let workspace = workspace_path();
    let ports_file = workspace.join(".lucidos/ports");
    let content = fs::read_to_string(&ports_file).unwrap_or_else(|e| {
        panic!(
            "Cannot read ports file at {}: {}. Is the workspace running?",
            ports_file.display(),
            e
        )
    });
    let mut port: Option<u16> = None;
    let mut proto: Option<String> = None;
    for line in content.lines() {
        if let Some(val) = line.strip_prefix("API_PORT=") {
            port = Some(val.trim().parse().expect("Invalid API_PORT"));
        } else if let Some(val) = line.strip_prefix("PROTO=") {
            proto = Some(val.trim().to_string());
        }
    }
    let port = port.unwrap_or_else(|| panic!("API_PORT not found in {}", ports_file.display()));
    (port, proto.unwrap_or_else(|| "https".to_string()))
}

/// Serializes every test that touches the workspace's `backup.key`.
///
/// Two files contend for it and the contention is not obvious from either:
/// `backup_key_test` deletes the key and asserts a read-only reveal does not
/// mint one, while `backup_schedule_test` enables a schedule, and
/// `PUT /backup/schedule` calls `crypto::ensure_key` whenever the new schedule
/// is active. Run in parallel, the second silently re-mints the key the first
/// just removed, and the failure surfaces in the innocent test.
///
/// Lives here rather than in either test file because a lock only one side
/// takes is not a lock.
pub fn backup_key_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));
    &LOCK
}

/// Serializes every test that temporarily widens the engine's thread-queue
/// capacity policy to guarantee itself an admission slot.
///
/// The policy is a single shared setting, and the widen is a read-modify-write:
/// each test reads the current policy, raises the limits it needs, fires, then
/// PUTs the value it read back. Two of those overlapping is not a slower test,
/// it is a leak. The second reader saves the FIRST one's widened policy as "the
/// original", and restores that after the first has already put the real one
/// back, so the whole rest of the suite runs on 512 concurrent slots and every
/// admission-refusal assertion silently stops testing anything.
///
/// Lives here rather than in either test file because a lock only one side
/// takes is not a lock.
pub fn capacity_policy_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));
    &LOCK
}

/// Holds the e2e workspace's working tree still for the one test that asserts
/// on a snapshot of the whole tree.
///
/// A command checkpoint is a pair of images of the ENTIRE working tree, taken
/// either side of a guarded command, and
/// `command_safety_test::a_command_destroying_only_ignored_content_leaves_no_undo_card`
/// asserts the pair comes out empty. Its own command only destroys a gitignored
/// path, so it contributes nothing, but the pair also captures whatever ELSE
/// lands in the tree between the two snapshots. The apply tests merge files
/// into `data/apps/` there, and on 2026-08-07 both full-suite runs caught
/// `style-a.css` + `style-b.css` from `app_coding_agent_concurrent_apply` in the
/// window: the guard read them as the command's own effects and emitted exactly
/// the card the test exists to prove absent.
///
/// Read/write rather than a plain mutex, so the tree writers keep running
/// concurrently with EACH OTHER and only the exclusive windows are exclusive.
/// Writers take `read()`.
///
/// TWO KINDS of holder take `write()`, and the second is easy to miss.
///
/// The snapshot is the first. The second is **any test that MERGES**. The
/// engine refuses to merge into a tree with uncommitted changes. A `read()`
/// holder may be part-way through creating a file it has not committed. That
/// refusal reached `app_coding_agent_concurrent_apply` as `Cannot merge: the
/// repository has uncommitted changes`, on one full run out of two. Merging
/// needs the tree QUIET, not merely un-snapshotted.
///
/// A merging test still takes ONE guard for its whole window, so requests it
/// fires inside that window overlap exactly as before. What `write()` costs is
/// the concurrency BETWEEN tests, which was the unsound part. It costs nothing
/// WITHIN a test, which is what those tests are about.
///
/// An apply the test expects to be REFUSED merges nothing and needs no guard.
///
/// **Every writer has to take it, so this is an obligation on new tests too.**
/// A lock the checkpoint test holds against only SOME writers still lets the
/// rest recreate the card. The writers today are the three apply tests, the two
/// trigger tests (their script files appearing and being removed), the
/// file-edit tests, the CLI data-write test, and the app-seeding helper. If you
/// add a test that creates, edits or deletes a non-ignored file under the e2e
/// workspace, take a `read()` guard across that mutation, or a `write()` one if
/// the mutation is a merge. Writes under
/// `.lucidos/` and `data/blobs/` need nothing: the workspace gitignores both, so
/// no snapshot ever sees them.
///
/// Only the moment a file APPEARS or DISAPPEARS needs the guard. A file present
/// across both images cancels out of the diff, so committing it later, or
/// reading it, is free.
///
/// Lives here rather than in either test file because a lock only one side
/// takes is not a lock.
pub fn workspace_tree_lock() -> &'static tokio::sync::RwLock<()> {
    static LOCK: std::sync::LazyLock<tokio::sync::RwLock<()>> =
        std::sync::LazyLock::new(|| tokio::sync::RwLock::new(()));
    &LOCK
}

pub fn workspace_path() -> PathBuf {
    if let Ok(ws) = std::env::var("E2E_WORKSPACE") {
        PathBuf::from(ws)
    } else {
        let home = std::env::var("HOME").expect("HOME not set");
        PathBuf::from(home).join("workspaces/e2e-test")
    }
}

/// Read the postgres port for the E2E workspace from its docker container.
pub fn db_url() -> String {
    let ws = workspace_path();
    let container =
        std::env::var("LUCIDOS_SHARED_PG_CONTAINER").unwrap_or_else(|_| "lucidos-pg-shared".into());
    let db_slug = ws
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("workspace")
        .chars()
        .fold(String::new(), |mut out, ch| {
            if ch.is_ascii_alphanumeric() {
                out.push(ch.to_ascii_lowercase());
            } else if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
            out
        })
        .trim_matches('-')
        .to_string();
    let db = format!(
        "lucidos_{}",
        if db_slug.is_empty() {
            "workspace"
        } else {
            &db_slug
        }
    );

    // Get the host port from docker
    let port_output = std::process::Command::new("docker")
        .args(["port", &container, "5432"])
        .output()
        .expect("Failed to get docker port");
    let port_line = String::from_utf8_lossy(&port_output.stdout)
        .trim()
        .to_string();
    // Format: "0.0.0.0:5438" or "[::]:5438" — take the last port number
    let port = port_line
        .lines()
        .next()
        .and_then(|l| l.rsplit(':').next())
        .expect("Could not parse docker port");
    format!("postgres://lucidos:lucidos@localhost:{}/{}", port, db)
}

pub fn base_url() -> String {
    let (port, proto) = read_ports();
    format!("{}://localhost:{}", proto, port)
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build HTTP client")
}

/// Put `device_id` in the `devices` table, so a header naming it is evidence.
///
/// `api::actor::require_user_actor` suppresses an id that names no row, so an
/// unregistered id is a header rather than an identity and the request is
/// refused. A test that sends its OWN id (rather than taking [`user_client`])
/// registers it here first. `POST /devices/register` is the one bootstrap the
/// mutating gate exempts, so the bare client is the right caller for it.
///
/// An upsert, so calling it repeatedly is free and emits no second
/// `DeviceRegistered`.
pub async fn register_device(device_id: &str) {
    let resp = http_client()
        .post(format!("{}/api/v1/devices/register", base_url()))
        .json(&serde_json::json!({
            "device_id": device_id,
            "user_agent": "lucidos-e2e/1",
        }))
        .send()
        .await
        .expect("device registration request failed");
    assert!(
        resp.status().is_success(),
        "device registration returned {}",
        resp.status()
    );
}

/// Device id this suite registers to stand in for the user's own client.
/// Stable across tests: registration is an upsert, and the `DeviceRegistered`
/// event only fires on the genuine first insert.
pub const E2E_DEVICE_ID: &str = "e2e-api-client";

/// An HTTP client that speaks for the user, the way the browser does: every
/// request carries `x-lucidos-device-id` for a device that is registered in
/// this workspace.
///
/// Use this for anything posting `mode: "human"`. The engine refuses a human
/// claim it has no evidence for (`api::chat::human_mode_is_attributed`), and
/// this suite IS a legitimate external client, so the honest way to keep it
/// working is to be a registered one rather than to weaken the gate. Tests that
/// deliberately exercise the refusal keep using the bare [`http_client`].
///
/// **A fresh client per call, deliberately.** Caching one in a `static` looks
/// like the obvious saving (registration is an upsert, so every call after the
/// first is a no-op write) and it is a trap: a `reqwest::Client` owns a
/// connection pool bound to the runtime that built it, each `#[tokio::test]`
/// gets its OWN runtime, and the first test to finish takes the pool's dispatch
/// task down with it. A later test reusing the cached client then fails with
/// `hyper::Error(User(DispatchGone), "runtime dropped the dispatch task")`, on a
/// different test each run depending on scheduling. The registration POST is a
/// single indexed upsert against a local engine, which is not the cost worth
/// optimising here.
pub async fn user_client() -> reqwest::Client {
    register_device(E2E_DEVICE_ID).await;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "x-lucidos-device-id",
        reqwest::header::HeaderValue::from_static(E2E_DEVICE_ID),
    );
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .default_headers(headers)
        .build()
        .expect("Failed to build device-attributed HTTP client")
}

/// Run a git command in the e2e workspace, asserting success.
pub fn git(args: &[&str]) -> std::process::Output {
    git_in(&workspace_path(), args)
}

/// Run a git command in an arbitrary directory, asserting success.
///
/// Retries on `.git/index.lock` collisions — `#[tokio::test]`s run
/// concurrently in one process, and two tests touching the same workspace
/// (e.g. both `app_coding_agent_*` tests against `e2e-test`) race on the
/// workspace's git index. The lock is short-lived; a few short backoffs
/// drain it without changing what's actually under test.
pub fn git_in(dir: &Path, args: &[&str]) -> std::process::Output {
    let mut attempt = 0u32;
    loop {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|e| panic!("git {} failed: {}", args.join(" "), e));
        if output.status.success() {
            return output;
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let is_lock_collision = stderr.contains("index.lock")
            && (stderr.contains("File exists") || stderr.contains("Unable to create"));
        if is_lock_collision && attempt < 20 {
            attempt += 1;
            std::thread::sleep(std::time::Duration::from_millis(150));
            continue;
        }
        panic!(
            "git {} in {} failed: {}",
            args.join(" "),
            dir.display(),
            stderr
        );
    }
}

pub fn unique_marker(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("{}-{}", prefix, ts)
}

/// Seed (or upsert) a CC-classified `thread_summaries` row. Required before
/// emitting CC-only `ThreadEvent`s (`ChangeProposed`,
/// `CodingAgentPermissionRequest`, …) — the lifecycle classifier rejects them
/// when the thread isn't classified as CC.
pub async fn seed_cc_thread_summary(pool: &sqlx::PgPool, thread_id: uuid::Uuid, status: &str) {
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, source, is_coding_agent, created_at, last_activity, message_count, status) \
         VALUES ($1, 'claude_code', TRUE, NOW(), NOW(), 0, $2) \
         ON CONFLICT (thread_id) DO UPDATE SET source = 'claude_code', is_coding_agent = TRUE, status = EXCLUDED.status"
    )
    .bind(thread_id)
    .bind(status)
    .execute(pool)
    .await
    .expect("failed to seed thread_summaries");
}

/// Seed an app coding-agent thread row. Sets `coding_agent_kind='app'` and
/// `coding_agent_folder=<workspace>/data/apps/<app_id>/` so
/// `load_apply_kind_context` dispatches through the App branch.
pub async fn seed_app_cc_thread_summary(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    app_id: &str,
    status: &str,
) {
    let folder = workspace_path()
        .join("data/apps")
        .join(app_id)
        .to_string_lossy()
        .into_owned();
    sqlx::query(
        "INSERT INTO thread_summaries \
         (thread_id, source, is_coding_agent, created_at, last_activity, message_count, status, \
          coding_agent_kind, coding_agent_folder) \
         VALUES ($1, 'claude_code', TRUE, NOW(), NOW(), 0, $2, 'app', $3) \
         ON CONFLICT (thread_id) DO UPDATE SET \
             source = 'claude_code', \
             is_coding_agent = TRUE, \
             status = EXCLUDED.status, \
             coding_agent_kind = 'app', \
             coding_agent_folder = EXCLUDED.coding_agent_folder",
    )
    .bind(thread_id)
    .bind(status)
    .bind(&folder)
    .execute(pool)
    .await
    .expect("failed to seed app thread_summaries");
}

/// Seed (or upsert) a chat-classified `thread_summaries` row. Required
/// before emitting `UserQuestionAsked` on a chat thread — the lifecycle
/// validator accepts the variant on chat threads now that the chat agent's
/// `ask_user_question` tool also raises it, but the row must exist with
/// `source = 'chat'` so the classifier resolves to `ThreadType::Chat`.
pub async fn seed_chat_thread_summary(pool: &sqlx::PgPool, thread_id: uuid::Uuid, status: &str) {
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, source, is_coding_agent, created_at, last_activity, message_count, status) \
         VALUES ($1, 'chat', FALSE, NOW(), NOW(), 0, $2) \
         ON CONFLICT (thread_id) DO UPDATE SET source = 'chat', is_coding_agent = FALSE, status = EXCLUDED.status"
    )
    .bind(thread_id)
    .bind(status)
    .execute(pool)
    .await
    .expect("failed to seed chat thread_summaries");
}

/// Poll `/api/v1/threads` until `archive` is non-empty, returning the parsed response.
/// Times out after `max_secs` with a panic.
pub async fn poll_threads_until_archive(
    client: &reqwest::Client,
    max_secs: u64,
) -> serde_json::Value {
    let url = format!("{}/api/v1/threads", base_url());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(max_secs);
    loop {
        let body: serde_json::Value = client
            .get(&url)
            .send()
            .await
            .expect("Threads request failed")
            .json()
            .await
            .expect("Invalid JSON");
        let archive = body["archive"].as_array().unwrap();
        if !archive.is_empty() {
            return body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Timed out waiting for thread to appear in archive after {}s",
            max_secs
        );
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

/// `POST /api/v1/repositories` for the given path with a `unique_marker(label)`
/// name, asserting `201 Created` and returning the new repo id.
pub async fn register_repo(client: &reqwest::Client, path: &Path, label: &str) -> String {
    let body = serde_json::json!({
        "name": unique_marker(label),
        "path": path.to_str().unwrap(),
        "description": format!("{} test repo", label),
    });
    let resp = client
        .post(format!("{}/api/v1/repositories", base_url()))
        .json(&body)
        .send()
        .await
        .expect("Register repo failed");
    assert_eq!(resp.status().as_u16(), 201, "Expected 201 Created");
    let repo: serde_json::Value = resp.json().await.expect("Invalid JSON");
    repo["id"].as_str().unwrap().to_string()
}

/// Count events of one type on a thread.
///
/// Most callers assert this is ZERO, so a query error must never come back as a
/// count. Swallowing one into `0` turns a "nothing was emitted" assertion into a
/// pass the moment the DB hiccups. Here rather than in either test file, so the
/// two cannot grow different failure behaviour for the same question.
pub async fn count_events_of_type(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    event_type: &str,
) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = $2",
    )
    .bind(thread_id)
    .bind(event_type)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("counting {event_type} on thread {thread_id} failed: {e}"))
}

/// Subset of `thread_summaries` columns the API tests poll for.
pub struct ThreadSummaryRow {
    pub thread_id: uuid::Uuid,
    pub parent_thread_id: Option<uuid::Uuid>,
    pub initiator: String,
}

/// Poll thread_summaries for the row whose first_message contains `marker`.
/// Tests sending chat messages with a unique marker use this to find their
/// own thread without racing parallel tests on `archive[0]` order.
pub async fn poll_thread_summary_by_marker(
    pool: &sqlx::PgPool,
    marker: &str,
    max_secs: u64,
) -> ThreadSummaryRow {
    let pattern = format!("%{}%", marker);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(max_secs);
    loop {
        let row: Option<(uuid::Uuid, Option<uuid::Uuid>, String)> = sqlx::query_as(
            "SELECT thread_id, parent_thread_id, initiator FROM thread_summaries \
             WHERE first_message LIKE $1 LIMIT 1",
        )
        .bind(&pattern)
        .fetch_optional(pool)
        .await
        .expect("DB query failed");
        if let Some((thread_id, parent_thread_id, initiator)) = row {
            return ThreadSummaryRow {
                thread_id,
                parent_thread_id,
                initiator,
            };
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Test thread for marker {marker} did not appear in thread_summaries within {max_secs}s",
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}
