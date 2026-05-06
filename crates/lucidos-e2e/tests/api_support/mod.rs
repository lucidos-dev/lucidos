//! Shared harness for E2E API integration tests.

use std::fs;
use std::path::{Path, PathBuf};

fn read_api_port() -> u16 {
    let workspace = workspace_path();
    let ports_file = workspace.join(".lucidos/ports");
    let content = fs::read_to_string(&ports_file).unwrap_or_else(|e| {
        panic!(
            "Cannot read ports file at {}: {}. Is the workspace running?",
            ports_file.display(),
            e
        )
    });
    for line in content.lines() {
        if let Some(val) = line.strip_prefix("API_PORT=") {
            return val.trim().parse().expect("Invalid API_PORT");
        }
    }
    panic!("API_PORT not found in {}", ports_file.display());
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
    let ws_str = ws.to_str().expect("workspace path not valid UTF-8");
    // Container name uses cksum of workspace path
    let output = std::process::Command::new("bash")
        .args([
            "-c",
            &format!("echo -n '{}' | cksum | cut -d' ' -f1", ws_str),
        ])
        .output()
        .expect("Failed to run cksum");
    let cksum = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let container = format!("lucidos-pg-{}", cksum);

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
    format!("postgres://lucidos:lucidos@localhost:{}/lucidos", port)
}

/// HTTPS with self-signed cert
pub fn base_url() -> String {
    format!("https://localhost:{}", read_api_port())
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
pub fn git_in(dir: &Path, args: &[&str]) -> std::process::Output {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git {} failed: {}", args.join(" "), e));
    assert!(
        output.status.success(),
        "git {} in {} failed: {}",
        args.join(" "),
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output
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
/// emitting CC-only `ThreadEvent`s (`ChangeProposed`, `UserQuestionAsked`,
/// `CodingAgentPermissionRequest`, …) — the lifecycle classifier rejects them
/// when the thread isn't classified as CC.
pub async fn seed_cc_thread_summary(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    status: &str,
) {
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, source, is_cc, created_at, last_activity, message_count, status) \
         VALUES ($1, 'claude_code', TRUE, NOW(), NOW(), 0, $2) \
         ON CONFLICT (thread_id) DO UPDATE SET source = 'claude_code', is_cc = TRUE, status = EXCLUDED.status"
    )
    .bind(thread_id)
    .bind(status)
    .execute(pool)
    .await
    .expect("failed to seed thread_summaries");
}

/// Poll `/api/threads` until `history` is non-empty, returning the parsed response.
/// Times out after `max_secs` with a panic.
pub async fn poll_threads_until_history(
    client: &reqwest::Client,
    max_secs: u64,
) -> serde_json::Value {
    let url = format!("{}/api/threads", base_url());
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
        let history = body["history"].as_array().unwrap();
        if !history.is_empty() {
            return body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Timed out waiting for thread to appear in history after {}s",
            max_secs
        );
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

/// `POST /api/repositories` for the given path with a `unique_marker(label)`
/// name, asserting `201 Created` and returning the new repo id.
pub async fn register_repo(client: &reqwest::Client, path: &Path, label: &str) -> String {
    let body = serde_json::json!({
        "name": unique_marker(label),
        "path": path.to_str().unwrap(),
        "description": format!("{} test repo", label),
    });
    let resp = client
        .post(format!("{}/api/repositories", base_url()))
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
/// own thread without racing parallel tests on `history[0]` order.
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
            return ThreadSummaryRow { thread_id, parent_thread_id, initiator };
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Test thread for marker {marker} did not appear in thread_summaries within {max_secs}s",
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}
