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
