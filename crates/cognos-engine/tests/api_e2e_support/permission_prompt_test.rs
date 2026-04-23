//! E2E for POST /api/internal/permission-prompt — the endpoint invoked by
//! cognos-cli's MCP permission server when CC asks for a tool-call decision.
//!
//! Drives the endpoint with HTTP only — no MCP subprocess required. The
//! handler:
//!   1. Registers a oneshot in `Engine.pending_mcp_consent` keyed by request_id.
//!   2. Emits `CodingAgentPermissionRequest` (persisted) with that request_id.
//!   3. Blocks until POST /api/mcp/consent resolves the oneshot, then emits
//!      `CodingAgentPermissionResolved` and returns `{ allowed, reason? }`.

use crate::support::{base_url, db_url, http_client};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[tokio::test]
#[ignore]
async fn permission_prompt_rejects_invalid_thread_id() {
    let client = http_client();
    let resp = client
        .post(format!("{}/api/internal/permission-prompt", base_url()))
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
#[ignore]
async fn permission_prompt_resolves_when_consent_posted() {
    let client = http_client();
    let pool = PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    // Seed a thread_summaries row so the projection's UPDATE finds the thread.
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, source, is_cc, created_at, last_activity, message_count, status) \
         VALUES ($1, 'claude_code', TRUE, NOW(), NOW(), 0, 'running') \
         ON CONFLICT (thread_id) DO UPDATE SET status = 'running'"
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("failed to upsert thread_summaries");

    // Issue the prompt request — it blocks on the oneshot. Spawn so we can
    // poll for the persisted request event in parallel.
    let prompt_task = {
        let client = client.clone();
        let thread_id = thread_id;
        tokio::spawn(async move {
            client
                .post(format!("{}/api/internal/permission-prompt", base_url()))
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
        .post(format!("{}/api/mcp/consent", base_url()))
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
#[ignore]
async fn permission_prompt_deduplicates_concurrent_identical_requests() {
    let client = http_client();
    let pool = PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, source, is_cc, created_at, last_activity, message_count, status) \
         VALUES ($1, 'claude_code', TRUE, NOW(), NOW(), 0, 'running') \
         ON CONFLICT (thread_id) DO UPDATE SET status = 'running'"
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("failed to upsert thread_summaries");

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
                .post(format!("{}/api/internal/permission-prompt", base_url()))
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
        .post(format!("{}/api/mcp/consent", base_url()))
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
