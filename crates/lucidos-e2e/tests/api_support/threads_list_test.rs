//! Coverage for `GET /api/v1/threads/list` and `GET /api/v1/threads/count` —
//! the script/trigger/LLM-tool surface for querying thread summaries.
//!
//! The strategy seeds rows directly into `thread_summaries` with a unique
//! marker title, then asserts the new endpoints surface them with the right
//! shape and the active filter behaves correctly. Going through the chat
//! endpoint would have to wait for an LLM round-trip and would race other
//! parallel tests; seeding the projection row is the contract surface the
//! endpoints actually read.

use crate::support::{base_url, db_url, http_client, unique_marker};
use lucidos_engine::engine::thread_lifecycle::ThreadStatus;

/// Insert a `thread_summaries` row with controlled `status` / `source` /
/// title for assertion. Returns the seeded `thread_id`. Takes
/// `ThreadStatus` (not a bare string) so a future variant rename breaks
/// this test at compile time instead of producing silently empty filters.
async fn seed_summary_row(
    pool: &sqlx::PgPool,
    title: &str,
    source: &str,
    status: ThreadStatus,
) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries \
            (thread_id, title, first_message, source, initiator, \
             created_at, last_activity, message_count, status) \
         VALUES ($1, $2, $2, $3, 'user', NOW(), NOW(), 1, $4)",
    )
    .bind(id)
    .bind(title)
    .bind(source)
    .bind(status.as_str())
    .execute(pool)
    .await
    .expect("seed thread_summaries");
    id
}

/// Look up our seeded summaries inside the response by title prefix —
/// other parallel tests' rows must not pollute the assertions.
fn rows_with_title_prefix<'a>(
    body: &'a serde_json::Value,
    prefix: &str,
) -> Vec<&'a serde_json::Value> {
    body.as_array()
        .expect("list response must be a JSON array")
        .iter()
        .filter(|r| {
            r["title"]
                .as_str()
                .map(|t| t.starts_with(prefix))
                .unwrap_or(false)
        })
        .collect()
}

#[tokio::test]
async fn list_returns_seeded_threads_with_full_shape() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-list-shape");

    let _idle =
        seed_summary_row(&pool, &format!("{marker}-idle"), "chat", ThreadStatus::Idle).await;
    let _running = seed_summary_row(
        &pool,
        &format!("{marker}-running"),
        "chat",
        ThreadStatus::Running,
    )
    .await;

    let url = format!("{}/api/v1/threads/list?limit=1000", base_url());
    let resp = client.get(&url).send().await.expect("list request");
    assert_eq!(resp.status(), 200, "list endpoint should return 200");
    let body: serde_json::Value = resp.json().await.expect("invalid JSON");

    let ours = rows_with_title_prefix(&body, &marker);
    assert_eq!(ours.len(), 2, "both seeded rows should appear");

    // Spot-check the full ThreadSummary shape — every field the projection
    // populates must serialise into the wire response.
    let row = ours
        .iter()
        .find(|r| r["title"].as_str() == Some(&format!("{marker}-running")))
        .expect("running row must be present");
    for field in [
        "thread_id",
        "title",
        "channel",
        "initiator",
        "created_at",
        "last_activity",
        "message_count",
        "section",
        "status",
        "active_children_count",
        "total_children_count",
        "coding_agent_has_diff",
        "coding_agent_proposed",
        "state",
        "compose_text",
        "compose_images",
    ] {
        assert!(
            row.get(field).is_some(),
            "ThreadSummary wire shape missing field `{field}` — got {row}"
        );
    }
    assert_eq!(row["status"].as_str(), Some("running"));
    assert_eq!(row["channel"].as_str(), Some("chat"));
}

#[tokio::test]
async fn list_active_filter_excludes_idle_and_failed() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-list-active");

    let _idle =
        seed_summary_row(&pool, &format!("{marker}-idle"), "chat", ThreadStatus::Idle).await;
    let _failed = seed_summary_row(
        &pool,
        &format!("{marker}-failed"),
        "chat",
        ThreadStatus::Failed,
    )
    .await;
    let _running = seed_summary_row(
        &pool,
        &format!("{marker}-running"),
        "chat",
        ThreadStatus::Running,
    )
    .await;
    let _waiting_for_user = seed_summary_row(
        &pool,
        &format!("{marker}-wait-user"),
        "claude_code",
        ThreadStatus::WaitingForUserAnswer,
    )
    .await;
    // `waiting` (CC proposed changes — loop has stopped) is NOT active per
    // the canonical definition.
    let _waiting = seed_summary_row(
        &pool,
        &format!("{marker}-waiting"),
        "claude_code",
        ThreadStatus::Waiting,
    )
    .await;

    let url = format!("{}/api/v1/threads/list?active=true&limit=1000", base_url());
    let body: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("active=true request")
        .json()
        .await
        .expect("invalid JSON");

    let ours = rows_with_title_prefix(&body, &marker);
    let titles: Vec<&str> = ours.iter().filter_map(|r| r["title"].as_str()).collect();
    assert!(
        titles.contains(&format!("{marker}-running").as_str()),
        "running row must appear in active list, got {titles:?}"
    );
    assert!(
        titles.contains(&format!("{marker}-wait-user").as_str()),
        "waiting_for_user_answer row must appear in active list, got {titles:?}"
    );
    assert!(
        !titles.contains(&format!("{marker}-idle").as_str()),
        "idle row must NOT appear in active list, got {titles:?}"
    );
    assert!(
        !titles.contains(&format!("{marker}-failed").as_str()),
        "failed row must NOT appear in active list, got {titles:?}"
    );
    assert!(
        !titles.contains(&format!("{marker}-waiting").as_str()),
        "waiting (post-CC-idled with proposed changes) must NOT appear in active list \
         — the loop has paused; got {titles:?}"
    );
}

#[tokio::test]
async fn list_source_filter_narrows_results() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-list-source");

    let _chat =
        seed_summary_row(&pool, &format!("{marker}-chat"), "chat", ThreadStatus::Idle).await;
    let _cc = seed_summary_row(
        &pool,
        &format!("{marker}-cc"),
        "claude_code",
        ThreadStatus::Idle,
    )
    .await;
    let _trig = seed_summary_row(
        &pool,
        &format!("{marker}-trig"),
        "trigger",
        ThreadStatus::Idle,
    )
    .await;

    let url = format!(
        "{}/api/v1/threads/list?source=chat,trigger&limit=1000",
        base_url()
    );
    let body: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("source request")
        .json()
        .await
        .expect("invalid JSON");

    let ours = rows_with_title_prefix(&body, &marker);
    let channels: Vec<&str> = ours.iter().filter_map(|r| r["channel"].as_str()).collect();
    assert!(
        channels.contains(&"chat"),
        "chat row missing from source=chat,trigger filter: {channels:?}"
    );
    assert!(
        channels.contains(&"trigger"),
        "trigger row missing from source=chat,trigger filter: {channels:?}"
    );
    assert!(
        channels.iter().all(|c| *c != "claude_code"),
        "claude_code row leaked through source=chat,trigger filter: {channels:?}"
    );
}

#[tokio::test]
async fn count_matches_list_length_under_same_filter() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-count");

    let _running = seed_summary_row(
        &pool,
        &format!("{marker}-r1"),
        "chat",
        ThreadStatus::Running,
    )
    .await;
    let _wait_user = seed_summary_row(
        &pool,
        &format!("{marker}-r2"),
        "claude_code",
        ThreadStatus::WaitingForUserAnswer,
    )
    .await;
    let _idle = seed_summary_row(&pool, &format!("{marker}-i"), "chat", ThreadStatus::Idle).await;

    // count and list must agree for the same filter set. Run against the
    // global active filter (no source narrowing) since other tests may also
    // have active rows — we only assert count >= our seeded count, AND that
    // count and list lengths agree.
    let list_url = format!("{}/api/v1/threads/list?active=true&limit=1000", base_url());
    let count_url = format!("{}/api/v1/threads/count?active=true", base_url());

    let list_body: serde_json::Value = client
        .get(&list_url)
        .send()
        .await
        .expect("list req")
        .json()
        .await
        .expect("invalid JSON");
    let count_body: serde_json::Value = client
        .get(&count_url)
        .send()
        .await
        .expect("count req")
        .json()
        .await
        .expect("invalid JSON");

    let list_len = list_body.as_array().expect("list is array").len() as i64;
    let count_val = count_body["count"].as_i64().expect("count is integer");

    // Both views must agree — the count endpoint is just a thinner version
    // of the same SQL filter. Other parallel tests may push the absolute
    // number around, but list.len() and count.count must match within the
    // same instant... we don't get atomicity across two HTTP calls, but the
    // margin should be tiny. Allow ±5 to absorb parallel test noise.
    assert!(
        (list_len - count_val).abs() <= 5,
        "list length {list_len} and count {count_val} disagree by more than 5 — \
         filters likely diverge"
    );

    // Our seeded active rows must be counted.
    let ours = rows_with_title_prefix(&list_body, &marker);
    assert_eq!(
        ours.len(),
        2,
        "expected both seeded active rows to appear (got {})",
        ours.len()
    );
}

#[tokio::test]
async fn list_limit_clamps_to_max_1000() {
    // The handler clamps `limit` to 1..=1000. Passing 999999 must NOT 500;
    // it should silently clamp and return success.
    let client = http_client();
    let url = format!("{}/api/v1/threads/list?limit=999999", base_url());
    let resp = client.get(&url).send().await.expect("oversized limit req");
    assert_eq!(
        resp.status(),
        200,
        "oversized limit must be clamped to 1000 server-side, not error"
    );
}

#[tokio::test]
async fn get_by_id_returns_the_seeded_thread_summary() {
    // `GET /api/v1/threads/:thread_id` — the by-id complement to `/list`, used
    // by the message-route popover to resolve a cross-workspace Origin's thread
    // name from the source workspace's engine.
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let marker = unique_marker("api-threads-get-by-id");

    let id = seed_summary_row(
        &pool,
        &format!("{marker}-title"),
        "chat",
        ThreadStatus::Idle,
    )
    .await;

    let url = format!("{}/api/v1/threads/{}", base_url(), id);
    let resp = client.get(&url).send().await.expect("get-by-id request");
    assert_eq!(resp.status(), 200, "by-id endpoint should return 200");
    let body: serde_json::Value = resp.json().await.expect("invalid JSON");

    assert_eq!(body["thread_id"].as_str(), Some(id.to_string().as_str()));
    assert_eq!(
        body["title"].as_str(),
        Some(format!("{marker}-title").as_str())
    );
}

#[tokio::test]
async fn get_by_id_returns_404_for_unknown_thread() {
    let client = http_client();
    let unknown = uuid::Uuid::new_v4();
    let url = format!("{}/api/v1/threads/{}", base_url(), unknown);
    let resp = client.get(&url).send().await.expect("unknown-id request");
    assert_eq!(
        resp.status(),
        404,
        "an unknown thread id must 404, not return an empty 200"
    );
}
