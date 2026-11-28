use std::fs;
use std::path::{Path, PathBuf};

pub fn png_bytes() -> Vec<u8> {
    vec![
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00,
    ]
}

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

fn api_port() -> u16 {
    let workspace = workspace_path();
    let ports_file = workspace.join(".lucidos/ports");
    let content = fs::read_to_string(&ports_file).unwrap_or_else(|e| {
        panic!(
            "Cannot read ports file at {}: {}. Is the workspace running?",
            ports_file.display(),
            e
        )
    });
    content
        .lines()
        .find_map(|line| line.strip_prefix("API_PORT="))
        .map(|val| val.trim().parse().expect("Invalid API_PORT"))
        .unwrap_or_else(|| panic!("API_PORT not found in {}", ports_file.display()))
}

pub fn backup_key_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));
    &LOCK
}

pub fn capacity_policy_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));
    &LOCK
}

pub async fn capacity_policy(client: &reqwest::Client) -> serde_json::Value {
    let body: serde_json::Value = client
        .get(format!("{}/api/v1/thread-queue", base_url()))
        .send()
        .await
        .expect("GET /thread-queue failed")
        .json()
        .await
        .expect("Invalid JSON");
    body["policy"].clone()
}

async fn set_capacity_policy(client: &reqwest::Client, policy: &serde_json::Value) {
    let resp = client
        .put(format!("{}/api/v1/thread-queue/policy", base_url()))
        .json(policy)
        .send()
        .await
        .expect("PUT /thread-queue/policy failed");
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    assert_eq!(status, 200, "policy PUT rejected: {body}");
}

pub async fn with_pool_room<T: Send + 'static>(
    client: &reqwest::Client,
    work: impl std::future::Future<Output = T> + Send + 'static,
) -> T {
    let _cap = capacity_policy_lock().lock().await;
    with_pool_room_held(client, work).await
}

pub async fn with_pool_room_held<T: Send + 'static>(
    client: &reqwest::Client,
    work: impl std::future::Future<Output = T> + Send + 'static,
) -> T {
    let original_policy = capacity_policy(client).await;
    let mut roomy_policy = original_policy.clone();
    roomy_policy["max_concurrent_total"] = serde_json::json!(512);
    roomy_policy["max_concurrent_cron"] = serde_json::json!(512);
    set_capacity_policy(client, &roomy_policy).await;
    let outcome = tokio::spawn(work).await;
    set_capacity_policy(client, &original_policy).await;
    match outcome {
        Ok(value) => value,
        Err(join) => std::panic::resume_unwind(join.into_panic()),
    }
}

pub fn workspace_tree_lock() -> &'static tokio::sync::RwLock<()> {
    static LOCK: std::sync::LazyLock<tokio::sync::RwLock<()>> =
        std::sync::LazyLock::new(|| tokio::sync::RwLock::new(()));
    &LOCK
}

pub async fn write_data_fixture(
    client: &reqwest::Client,
    rel: &str,
    body: &str,
) -> Result<(), String> {
    let resp = client
        .put(format!("{}/api/v1/data/{}", base_url(), rel))
        .header("Content-Type", "text/plain")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| format!("PUT /api/v1/data/{rel} failed: {e}"))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    Err(format!("PUT /api/v1/data/{rel} returned {status}: {body}"))
}

pub async fn remove_data_fixtures(client: &reqwest::Client, dir: &str, files: &[String]) {
    let _tree = workspace_tree_lock().read().await;
    for rel in files {
        let _ = client
            .delete(format!("{}/api/v1/data/{}", base_url(), rel))
            .send()
            .await;
    }
    remove_empty_dirs(&workspace_path().join("data").join(dir));
}

fn remove_empty_dirs(dir: &Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                remove_empty_dirs(&entry.path());
            }
        }
    }
    let _ = fs::remove_dir(dir);
}

pub fn apis_json_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));
    &LOCK
}

pub fn workspace_path() -> PathBuf {
    if let Ok(ws) = std::env::var("E2E_WORKSPACE") {
        return PathBuf::from(ws);
    }
    let root = std::env::var("LUCIDOS_E2E_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").expect("HOME not set");
            PathBuf::from(home).join(".lucidos/e2e-root")
        });
    root.join("workspaces/e2e-test")
}

pub fn db_url() -> String {
    let ws = workspace_path();
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
    let password = lucidos_local_token::read_named(lucidos_local_token::SHARED_PG_PASSWORD_FILE)
        .expect("the shared cluster's password file, which setup_postgres mints");
    format!(
        "postgres://lucidos:{password}@localhost:{}/{}",
        db_port(),
        db
    )
}

pub const WS_TEXT: u8 = 0x1;
pub const WS_BINARY: u8 = 0x2;
pub const WS_CLOSE: u8 = 0x8;

/// A final, masked client frame, as a browser sends it.
pub fn ws_client_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() < 126, "test payloads stay in the short form");
    let mask = [0x37u8, 0xfa, 0x21, 0x3d];
    let mut frame = vec![0x80 | opcode, 0x80 | payload.len() as u8];
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    frame
}

/// The next final server frame: its opcode and payload.
pub async fn ws_read_server_frame<S>(socket: &mut S) -> (u8, Vec<u8>)
where
    S: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut head = [0u8; 2];
    socket.read_exact(&mut head).await.expect("frame header");
    assert_eq!(head[0] & 0x80, 0x80, "the server sends final frames");
    assert_eq!(head[1] & 0x80, 0, "a server frame is never masked");
    let len = match head[1] & 0x7f {
        126 => u64::from(socket.read_u16().await.expect("16-bit length")),
        127 => socket.read_u64().await.expect("64-bit length"),
        short => u64::from(short),
    };
    let mut payload = vec![0u8; len as usize];
    socket.read_exact(&mut payload).await.expect("frame body");
    (head[0] & 0x0f, payload)
}

pub async fn home_thread_id() -> uuid::Uuid {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let home = sqlx::query_scalar("SELECT thread_id FROM thread_summaries WHERE is_home")
        .fetch_one(&pool)
        .await
        .expect("the workspace has a home thread");
    pool.close().await;
    home
}

fn db_port() -> String {
    if let Ok(port) = std::env::var("LUCIDOS_EXTERNAL_PG_PORT") {
        return port;
    }
    let container =
        std::env::var("LUCIDOS_SHARED_PG_CONTAINER").unwrap_or_else(|_| "lucidos-pg-shared".into());
    let port_output = std::process::Command::new("docker")
        .args(["port", &container, "5432"])
        .output()
        .expect("Failed to get docker port");
    let port_line = String::from_utf8_lossy(&port_output.stdout)
        .trim()
        .to_string();
    port_line
        .lines()
        .next()
        .and_then(|l| l.rsplit(':').next())
        .expect("Could not parse docker port")
        .to_string()
}

pub fn base_url() -> String {
    format!("http://localhost:{}", api_port())
}

pub fn http_client() -> reqwest::Client {
    lucidos_local_token::client::client_builder()
        .build()
        .expect("Failed to build HTTP client")
}

pub fn browser_handshake(
    client: &reqwest::Client,
    path: &str,
    origin: &str,
) -> reqwest::RequestBuilder {
    client
        .get(format!("{}{path}", base_url()))
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", b64(&[0u8; 16]))
        .header("origin", origin)
}

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

pub const E2E_DEVICE_ID: &str = "e2e-api-client";

fn harness_stamp_key() -> Option<(lucidos_gateway_stamp::StampKey, String)> {
    let text = fs::read_to_string(workspace_path().join(".lucidos/e2e-stamp-key")).ok()?;
    let mut lines = text.lines();
    let key = lucidos_gateway_stamp::StampKey::read_handover(lines.next()?.as_bytes())?;
    Some((key, lines.next()?.to_string()))
}

pub fn device_stamp(device_id: &str) -> Option<String> {
    let (key, workspace_id) = harness_stamp_key()?;
    Some(key.sign_now(lucidos_gateway_stamp::Claim::Device {
        workspace_id: &workspace_id,
        device_id,
    }))
}

pub fn as_stamped_device(
    builder: reqwest::RequestBuilder,
    device_id: &str,
) -> reqwest::RequestBuilder {
    let builder = builder.header("x-lucidos-device-id", device_id);
    match device_stamp(device_id) {
        Some(stamp) => builder.header(lucidos_gateway_stamp::HEADER_GATEWAY_STAMP, stamp),
        None => builder,
    }
}

pub fn as_stamped_workspace_call(
    builder: reqwest::RequestBuilder,
    caller_workspace: &str,
    verdict: &lucidos_engine::engine::thread_events::SpawnVerdict,
) -> reqwest::RequestBuilder {
    let Some((key, workspace_id)) = harness_stamp_key() else {
        return builder;
    };
    let verdict = verdict.to_header();
    let stamp = key.sign_now(lucidos_gateway_stamp::Claim::WorkspaceCall {
        target_workspace: &workspace_id,
        caller_workspace,
        caller_thread_id: "",
        verdict: &verdict,
    });
    builder
        .header(lucidos_gateway_stamp::HEADER_GATEWAY_STAMP, stamp)
        .header(lucidos_gateway_stamp::HEADER_CALLER_VERDICT, verdict)
}

fn device_headers(stamped: bool) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "x-lucidos-device-id",
        reqwest::header::HeaderValue::from_static(E2E_DEVICE_ID),
    );
    if let Some(stamp) = device_stamp(E2E_DEVICE_ID).filter(|_| stamped) {
        headers.insert(
            lucidos_gateway_stamp::HEADER_GATEWAY_STAMP,
            reqwest::header::HeaderValue::from_str(&stamp).expect("a header-safe stamp"),
        );
    }
    headers
}

fn device_client(stamped: bool) -> reqwest::Client {
    lucidos_local_token::client::client_builder()
        .default_headers(device_headers(stamped))
        .build()
        .expect("Failed to build device-attributed HTTP client")
}

pub async fn user_client() -> reqwest::Client {
    register_device(E2E_DEVICE_ID).await;
    device_client(true)
}

pub async fn unattested_user_client() -> reqwest::Client {
    register_device(E2E_DEVICE_ID).await;
    device_client(false)
}

pub fn local_process_client() -> reqwest::Client {
    let token = lucidos_local_token::read().unwrap_or_else(|| {
        panic!(
            "no machine-local token at {:?}; the engine mints one at boot",
            lucidos_local_token::path()
        )
    });
    let mut value = reqwest::header::HeaderValue::from_str(&token).expect("local token header");
    value.set_sensitive(true);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(lucidos_local_token::HEADER_LOCAL_TOKEN, value);
    lucidos_local_token::client::client_builder()
        .default_headers(headers)
        .build()
        .expect("Failed to build local-process HTTP client")
}

pub fn git(args: &[&str]) -> std::process::Output {
    git_in(&workspace_path(), args)
}

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
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let prev = LAST
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |last| {
            Some(now.max(last + 1))
        })
        .unwrap();
    format!("{}-{}", prefix, now.max(prev + 1))
}

#[test]
fn unique_marker_never_repeats_inside_one_millisecond() {
    let markers: std::collections::HashSet<String> =
        (0..1000).map(|_| unique_marker("same")).collect();
    assert_eq!(markers.len(), 1000);
}

pub fn e2e_font_slug(prefix: &str) -> String {
    let slug = format!("{prefix}-{:016x}", uuid::Uuid::new_v4().as_u64_pair().0);
    assert!(
        slug.len() <= lucidos_engine::core::workspace_fonts::MAX_SLUG_LEN,
        "{slug} is longer than a font directory name may be"
    );
    slug
}

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

pub struct ThreadSummaryRow {
    pub thread_id: uuid::Uuid,
    pub parent_thread_id: Option<uuid::Uuid>,
    pub initiator: String,
}

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

pub fn commit_on_new_branch(branch: &str, file: &str, contents: &str) {
    let wt_tmp = tempfile::tempdir().unwrap();
    let wt_dir = wt_tmp.path();
    git(&[
        "worktree",
        "add",
        wt_dir.to_str().unwrap(),
        "-b",
        branch,
        "main",
    ]);
    let path = wt_dir.join(file);
    std::fs::create_dir_all(path.parent().expect("a file has a parent")).unwrap();
    std::fs::write(path, contents).unwrap();
    git_in(wt_dir, &["add", file]);
    git_in(wt_dir, &["commit", "-m", &format!("e2e: {file}")]);
    git(&["worktree", "remove", "--force", wt_dir.to_str().unwrap()]);
}

pub async fn insert_session_started(pool: &sqlx::PgPool, thread_id: uuid::Uuid, branch: &str) {
    let payload = serde_json::json!({
        "coding_agent": "claude-code",
        "session_id": format!("sess-{thread_id}"),
        "branch": branch,
        "coding_agent_kind": "app",
        "channel": "claude_code",
    });
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2::text, 'SessionStarted', $3, NOW(), $2)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(thread_id)
    .bind(payload)
    .execute(pool)
    .await
    .expect("failed to insert SessionStarted");
}
