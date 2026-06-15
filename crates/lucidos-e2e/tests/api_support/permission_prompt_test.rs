//! E2E for POST /api/v1/internal/permission-prompt — the endpoint invoked by
//! lucidos-cli's MCP permission server when CC asks for a tool-call decision.
//!
//! Drives the endpoint with HTTP only — no MCP subprocess required. The
//! handler:
//!   1. Registers a oneshot in `Engine.pending_mcp_consent` keyed by request_id.
//!   2. Emits `CodingAgentPermissionRequest` (persisted) with that request_id.
//!   3. Blocks until POST /api/v1/mcp/consent resolves the oneshot, then emits
//!      `CodingAgentPermissionResolved` and returns `{ allowed, reason? }`.

use crate::support::{base_url, db_url, http_client, seed_cc_thread_summary};
use serde_json::json;
use sqlx::PgPool;
use std::sync::LazyLock;
use tokio::sync::{Mutex, MutexGuard};
use uuid::Uuid;

/// Serialize tests that snapshot/modify the global `~/.lucidos/cc-allowed-tools`
/// file. Cargo runs integration tests in parallel within a binary, so without
/// this lock concurrent snapshot+restore cycles would race and clobber each
/// other's restore. Acquire at the top of any test that calls
/// `read_cc_allowed_tools` / `restore_cc_allowed_tools`; hold for the full
/// snapshot → mutate → assert → restore cycle.
static CC_ALLOWED_TOOLS_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

async fn lock_cc_allowed_tools() -> MutexGuard<'static, ()> {
    CC_ALLOWED_TOOLS_LOCK.lock().await
}

#[tokio::test]
async fn permission_prompt_rejects_invalid_thread_id() {
    let client = http_client();
    let resp = client
        .post(format!("{}/api/v1/internal/permission-prompt", base_url()))
        .json(&json!({
            "thread_id": "not-a-uuid",
            "tool_use_id": "tu_1",
            "tool_name": "Edit",
            "input": {}
        }))
        .send()
        .await
        .expect("request failed");
    assert_eq!(resp.status().as_u16(), 400, "non-UUID thread_id must 400");
}

#[tokio::test]
async fn permission_prompt_resolves_when_consent_posted() {
    let client = http_client();
    let pool = PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "running").await;

    // Issue the prompt request — it blocks on the oneshot. Spawn so we can
    // poll for the persisted request event in parallel.
    let prompt_task = {
        let client = client.clone();
        tokio::spawn(async move {
            client
                .post(format!("{}/api/v1/internal/permission-prompt", base_url()))
                .json(&json!({
                    "thread_id": thread_id.to_string(),
                    "tool_use_id": "tu_perm_1",
                    "tool_name": "Edit",
                    "input": { "file_path": "/tmp/foo.md" }
                }))
                .send()
                .await
                .expect("prompt request failed")
                .json::<serde_json::Value>()
                .await
                .expect("invalid JSON body")
        })
    };

    let request_id =
        wait_for_permission_request(&pool, thread_id, std::time::Duration::from_secs(10)).await;

    let consent = client
        .post(format!("{}/api/v1/mcp/consent", base_url()))
        .json(&json!({ "request_id": request_id, "allowed": true }))
        .send()
        .await
        .expect("consent request failed");
    assert_eq!(consent.status().as_u16(), 200, "consent should 200");

    let body = prompt_task.await.expect("prompt task panicked");
    assert_eq!(body["allowed"], true, "response must reflect allowed=true");

    let resolved_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE thread_id = $1 AND event_type = 'CodingAgentPermissionResolved' \
           AND (payload->>'allowed')::boolean = TRUE",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap_or(0);
    assert_eq!(
        resolved_count, 1,
        "exactly one CodingAgentPermissionResolved with allowed=true must be persisted"
    );
}

/// Concurrent identical permission requests must surface as a single card to
/// the user. CC can fire several `tools/call` for the same logical action in
/// one assistant turn (parallel tool_use blocks, or sequential retries after
/// a denial). Without dedup, each one renders its own `PermissionCard` — the
/// "infinite loop of file-access prompts" the user reported.
///
/// Verifies:
///   - exactly ONE `CodingAgentPermissionRequest` is persisted across N
///     concurrent identical requests
///   - a single consent answers ALL of them
///   - exactly ONE `CodingAgentPermissionResolved` is persisted
#[tokio::test]
async fn permission_prompt_deduplicates_concurrent_identical_requests() {
    let client = http_client();
    let pool = PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "running").await;

    // Fire 3 concurrent identical permission_prompt requests. Each gets its
    // own tool_use_id (CC mints those per call) but the (thread_id, tool_name,
    // input) triple is identical — the engine must dedup.
    let body = json!({
        "thread_id": thread_id.to_string(),
        "tool_use_id": "tu_dup_1",
        "tool_name": "Edit",
        "input": { "file_path": "/tmp/dedup-target.md", "old_string": "x", "new_string": "y" }
    });
    let mut tasks = Vec::new();
    for i in 0..3 {
        let client = client.clone();
        let mut body = body.clone();
        body["tool_use_id"] = json!(format!("tu_dup_{}", i + 1));
        tasks.push(tokio::spawn(async move {
            client
                .post(format!("{}/api/v1/internal/permission-prompt", base_url()))
                .json(&body)
                .send()
                .await
                .expect("prompt request failed")
                .json::<serde_json::Value>()
                .await
                .expect("invalid JSON body")
        }));
    }

    // Wait for the canonical request event to appear, then briefly let the
    // other two duplicates settle so any erroneous extra events would persist.
    let request_id =
        wait_for_permission_request(&pool, thread_id, std::time::Duration::from_secs(10)).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let request_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE thread_id = $1 AND event_type = 'CodingAgentPermissionRequest'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap_or(0);
    assert_eq!(
        request_count, 1,
        "concurrent identical permission requests must produce exactly ONE request event \
         (got {request_count}); duplicates flood the user with cards"
    );

    let consent = client
        .post(format!("{}/api/v1/mcp/consent", base_url()))
        .json(&json!({ "request_id": request_id, "allowed": true }))
        .send()
        .await
        .expect("consent request failed");
    assert_eq!(consent.status().as_u16(), 200, "consent should 200");

    for task in tasks {
        let body = task.await.expect("prompt task panicked");
        assert_eq!(
            body["allowed"], true,
            "every duplicate request must receive the same allow answer"
        );
    }

    let resolved_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE thread_id = $1 AND event_type = 'CodingAgentPermissionResolved'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap_or(0);
    assert_eq!(
        resolved_count, 1,
        "exactly one CodingAgentPermissionResolved must be persisted, not one per duplicate"
    );

    sqlx::query("DELETE FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .ok();
}

/// `persist_scope: "narrow"` on a Skill prompt → engine appends
/// `Skill(<plugin>:*)` to ~/.lucidos/cc-allowed-tools and a second identical
/// click is a no-op (no duplicate line).
#[tokio::test]
async fn permission_prompt_persists_narrow_skill_pattern() {
    let _lock = lock_cc_allowed_tools().await;
    let client = http_client();
    let pool = PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    // Unique plugin name per run so concurrent / repeat runs never collide.
    let plugin = format!("test-{}", Uuid::new_v4().simple());
    let pattern = format!("Skill({}:*)", plugin);
    let snapshot = read_cc_allowed_tools(&client).await;

    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "running").await;

    persist_via_consent(
        &client,
        &pool,
        thread_id,
        "Skill",
        json!({ "skill": format!("{}:demo", plugin) }),
        "narrow",
    )
    .await;

    let after_first = read_cc_allowed_tools(&client).await;
    assert!(
        line_present(&after_first, &pattern),
        "after first allow, file must contain {pattern}; got:\n{after_first}"
    );

    // Repeat — must not duplicate the line.
    let thread_id2 = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id2, "running").await;
    persist_via_consent(
        &client,
        &pool,
        thread_id2,
        "Skill",
        json!({ "skill": format!("{}:demo", plugin) }),
        "narrow",
    )
    .await;
    let after_second = read_cc_allowed_tools(&client).await;
    assert_eq!(
        count_line(&after_second, &pattern),
        1,
        "duplicate Always-allow click must not double-write the pattern; file:\n{after_second}"
    );

    restore_cc_allowed_tools(&client, &snapshot).await;
}

/// `persist_scope: "broad"` on a Bash prompt → engine appends bare `Bash`.
#[tokio::test]
async fn permission_prompt_persists_broad_bash_pattern() {
    let _lock = lock_cc_allowed_tools().await;
    let client = http_client();
    let pool = PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    // Use a unique sentinel inside the input so we never accidentally match
    // a pre-existing canonical request from the e2e workspace.
    let sentinel = format!("echo {}", Uuid::new_v4().simple());
    let snapshot = read_cc_allowed_tools(&client).await;

    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "running").await;

    persist_via_consent(
        &client,
        &pool,
        thread_id,
        "Bash",
        json!({ "command": sentinel }),
        "broad",
    )
    .await;

    let after = read_cc_allowed_tools(&client).await;
    assert!(
        line_present(&after, "Bash"),
        "broad Bash always-allow must append bare 'Bash'; got:\n{after}"
    );

    restore_cc_allowed_tools(&client, &snapshot).await;
}

/// `persist_scope: "broad"` on `Edit` (and Write/NotebookEdit) must NOT write
/// the file: bare `Edit` in `--allowedTools` is silently ignored by CC's
/// `acceptEdits` routing for out-of-cwd paths, so persisting it would mislead
/// the user into thinking the prompt won't recur. Engine refuses at the
/// `derive_allow_pattern` boundary; UI hides the corresponding button.
#[tokio::test]
async fn permission_prompt_broad_persist_for_edit_does_not_write_file() {
    let _lock = lock_cc_allowed_tools().await;
    let client = http_client();
    let pool = PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let snapshot = read_cc_allowed_tools(&client).await;
    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "running").await;

    persist_via_consent(
        &client,
        &pool,
        thread_id,
        "Edit",
        json!({
            "file_path": format!("/tmp/edit-broad-test-{}.md", Uuid::new_v4().simple()),
            "old_string": "x",
            "new_string": "y"
        }),
        "broad",
    )
    .await;

    let after = read_cc_allowed_tools(&client).await;
    // snapshot == after is the load-bearing assertion: the engine must leave
    // the file untouched. A standalone `!line_present(after, "Edit")` would
    // be wrong because the user's pre-existing file may already contain
    // `Edit` (from a prior install or manual edit) — what matters is that
    // *this* call didn't append.
    assert_eq!(
        snapshot, after,
        "broad persist on Edit must NOT modify cc-allowed-tools — bare entry is a no-op for acceptEdits-routed tools"
    );
}

/// Without `persist_scope`, the consent endpoint must not touch the file.
/// Regression guard for the existing Allow-once behavior.
#[tokio::test]
async fn permission_prompt_without_persist_scope_does_not_write_file() {
    let _lock = lock_cc_allowed_tools().await;
    let client = http_client();
    let pool = PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let plugin = format!("test-{}", Uuid::new_v4().simple());
    let pattern = format!("Skill({}:*)", plugin);
    let before = read_cc_allowed_tools(&client).await;

    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "running").await;

    // Drive the prompt → consent flow without persist_scope.
    let prompt_task = {
        let client = client.clone();
        let plugin = plugin.clone();
        tokio::spawn(async move {
            client
                .post(format!("{}/api/v1/internal/permission-prompt", base_url()))
                .json(&json!({
                    "thread_id": thread_id.to_string(),
                    "tool_use_id": format!("tu_{}", Uuid::new_v4()),
                    "tool_name": "Skill",
                    "input": { "skill": format!("{}:demo", plugin) }
                }))
                .send()
                .await
                .expect("prompt request failed")
                .json::<serde_json::Value>()
                .await
                .expect("invalid JSON body")
        })
    };

    let request_id =
        wait_for_permission_request(&pool, thread_id, std::time::Duration::from_secs(10)).await;

    let consent = client
        .post(format!("{}/api/v1/mcp/consent", base_url()))
        .json(&json!({ "request_id": request_id, "allowed": true }))
        .send()
        .await
        .expect("consent request failed");
    assert_eq!(consent.status().as_u16(), 200);
    let _ = prompt_task.await.expect("prompt task panicked");

    let after = read_cc_allowed_tools(&client).await;
    assert_eq!(
        before, after,
        "Allow-once (no persist_scope) must leave cc-allowed-tools unchanged"
    );
    assert!(
        !line_present(&after, &pattern),
        "no persist_scope must not append {pattern}"
    );
}

/// Read the current ~/.lucidos/cc-allowed-tools via the settings endpoint.
async fn read_cc_allowed_tools(client: &reqwest::Client) -> String {
    let resp = client
        .get(format!("{}/api/v1/cc-allowed-tools", base_url()))
        .send()
        .await
        .expect("GET cc-allowed-tools failed");
    assert_eq!(resp.status().as_u16(), 200, "GET cc-allowed-tools must 200");
    let body: serde_json::Value = resp.json().await.expect("invalid JSON");
    body["contents"].as_str().unwrap_or("").to_string()
}

/// Restore the settings file to a previously-snapshotted state.
async fn restore_cc_allowed_tools(client: &reqwest::Client, contents: &str) {
    let resp = client
        .put(format!("{}/api/v1/cc-allowed-tools", base_url()))
        .json(&json!({ "contents": contents }))
        .send()
        .await
        .expect("PUT cc-allowed-tools failed");
    assert!(
        resp.status().is_success(),
        "PUT cc-allowed-tools must succeed during restore"
    );
}

/// GET/PUT round-trip of `~/.lucidos/agent-allowed-commands` via the settings
/// endpoints that back the Settings → Permissions → Lucidos Agent permissions
/// list editor. Mirrors the `cc-allowed-tools` settings API.
#[tokio::test]
async fn agent_allowed_commands_settings_roundtrip() {
    let client = http_client();
    // Snapshot, write a known body with a unique sentinel, read it back, restore.
    let snapshot = read_agent_allowed_commands(&client).await;
    let sentinel = format!("Bash(e2e-{}:*)", Uuid::new_v4().simple());
    let body = format!(
        "# Lucidos Agent command allowlist — one pattern per line.\n{sentinel}\nPython\n"
    );

    let put = client
        .put(format!("{}/api/v1/agent-allowed-commands", base_url()))
        .json(&json!({ "contents": body }))
        .send()
        .await
        .expect("PUT agent-allowed-commands failed");
    assert!(put.status().is_success(), "PUT must succeed");

    let after = read_agent_allowed_commands(&client).await;
    assert_eq!(after, body, "GET must return exactly what was PUT");
    assert!(
        line_present(&after, &sentinel),
        "round-tripped file must contain the sentinel pattern; got:\n{after}"
    );

    // Restore so the e2e workspace's allowlist is left as we found it.
    let restore = client
        .put(format!("{}/api/v1/agent-allowed-commands", base_url()))
        .json(&json!({ "contents": snapshot }))
        .send()
        .await
        .expect("PUT agent-allowed-commands (restore) failed");
    assert!(restore.status().is_success(), "restore PUT must succeed");
}

/// Read the current ~/.lucidos/agent-allowed-commands via the settings endpoint.
async fn read_agent_allowed_commands(client: &reqwest::Client) -> String {
    let resp = client
        .get(format!("{}/api/v1/agent-allowed-commands", base_url()))
        .send()
        .await
        .expect("GET agent-allowed-commands failed");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "GET agent-allowed-commands must 200"
    );
    let body: serde_json::Value = resp.json().await.expect("invalid JSON");
    body["contents"].as_str().unwrap_or("").to_string()
}

fn line_present(contents: &str, pattern: &str) -> bool {
    count_line(contents, pattern) > 0
}

fn count_line(contents: &str, pattern: &str) -> usize {
    contents
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && *l == pattern)
        .count()
}

/// End-to-end: open a permission_prompt, wait for the canonical request event,
/// then resolve via /api/v1/mcp/consent with the given persist_scope.
async fn persist_via_consent(
    client: &reqwest::Client,
    pool: &PgPool,
    thread_id: Uuid,
    tool_name: &str,
    input: serde_json::Value,
    scope: &str,
) {
    let prompt_task = {
        let client = client.clone();
        let tool_name = tool_name.to_string();
        let input = input.clone();
        tokio::spawn(async move {
            client
                .post(format!("{}/api/v1/internal/permission-prompt", base_url()))
                .json(&json!({
                    "thread_id": thread_id.to_string(),
                    "tool_use_id": format!("tu_{}", Uuid::new_v4()),
                    "tool_name": tool_name,
                    "input": input
                }))
                .send()
                .await
                .expect("prompt request failed")
                .json::<serde_json::Value>()
                .await
                .expect("invalid JSON body")
        })
    };

    let request_id =
        wait_for_permission_request(pool, thread_id, std::time::Duration::from_secs(10)).await;

    let consent = client
        .post(format!("{}/api/v1/mcp/consent", base_url()))
        .json(&json!({
            "request_id": request_id,
            "allowed": true,
            "persist_scope": scope,
        }))
        .send()
        .await
        .expect("consent request failed");
    assert_eq!(
        consent.status().as_u16(),
        200,
        "consent with persist_scope={scope} must 200"
    );

    let body = prompt_task.await.expect("prompt task panicked");
    assert_eq!(body["allowed"], true);
}

/// Poll the events table until a `CodingAgentPermissionRequest` for `thread_id`
/// appears, returning the typed `request_id`. Panics on timeout.
async fn wait_for_permission_request(
    pool: &PgPool,
    thread_id: Uuid,
    timeout: std::time::Duration,
) -> String {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT payload->>'request_id' FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentPermissionRequest' \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(pool)
        .await
        .expect("DB query failed");
        if let Some((request_id,)) = row {
            return request_id;
        }
        if std::time::Instant::now() >= deadline {
            panic!("CodingAgentPermissionRequest never persisted");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
