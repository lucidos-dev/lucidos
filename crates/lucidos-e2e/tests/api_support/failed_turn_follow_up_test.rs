//! E2E: a follow-up typed into a chat turn that then FAILS still gets a turn.
//!
//! The follow-up is injected into the live turn. The turn's model call then
//! errors, so the loop exits before reading it. The turn tail drains it as an
//! orphan. An `Err` returns no `ProcessResult` to carry it back to the caller,
//! so the engine must re-submit it.

use crate::support::{base_url, db_url, poll_thread_summary_by_marker, unique_marker, user_client};
use lucidos_engine::llm::mock::{
    MOCK_FAIL_AFTER_MS_SENTINEL, MOCK_RESPONSE, MOCK_SCRIPTED_FAILURE,
};
use uuid::Uuid;

/// Long enough for the follow-up to land inside the live turn, on a loaded
/// host too. The test checks that it did, rather than trusting the number.
const FAIL_AFTER_MS: u64 = 8_000;

async fn sequence_of(pool: &sqlx::PgPool, event_id: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT sequence FROM events WHERE id::text = $1")
        .bind(event_id)
        .fetch_optional(pool)
        .await
        .expect("events query failed")
}

/// The sequence and text of the first `event_type` anchored on `anchor`.
async fn anchored(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    event_type: &str,
    anchor: &str,
) -> Option<(i64, Option<String>)> {
    sqlx::query_as(
        "SELECT sequence, coalesce(payload->>'text', payload->>'error') FROM events \
          WHERE thread_id = $1 AND event_type = $2 \
            AND payload->>'request_event_id' = $3 \
          ORDER BY sequence LIMIT 1",
    )
    .bind(thread_id)
    .bind(event_type)
    .bind(anchor)
    .fetch_optional(pool)
    .await
    .expect("events query failed")
}

async fn wait_for_anchored(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    event_type: &str,
    anchor: &str,
    what: &str,
) -> (i64, Option<String>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if let Some(found) = anchored(pool, thread_id, event_type, anchor).await {
            return found;
        }
        assert!(std::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

#[tokio::test]
async fn a_follow_up_typed_into_a_failing_turn_gets_its_own_turn() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let url = format!("{}/api/v1/chat/stream", base_url());
    let marker = unique_marker("api-failed-turn-follow-up");

    let opening_id = Uuid::new_v4().to_string();
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "message": format!("{marker} {MOCK_FAIL_AFTER_MS_SENTINEL} {FAIL_AFTER_MS}"),
            "mode": "human",
            "event_id": opening_id,
        }))
        .send()
        .await
        .expect("opening chat request failed");
    assert_eq!(resp.status(), 200);
    let thread_id = poll_thread_summary_by_marker(&pool, &marker, 25)
        .await
        .thread_id;

    let follow_up_id = Uuid::new_v4().to_string();
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "message": format!("and one more thing {marker}"),
            "mode": "human",
            "thread_id": thread_id.to_string(),
            "event_id": follow_up_id,
        }))
        .send()
        .await
        .expect("follow-up chat request failed");
    assert_eq!(resp.status(), 200);

    let (failed_at, error) = wait_for_anchored(
        &pool,
        thread_id,
        "ResponseFailed",
        &opening_id,
        "the opening turn never failed",
    )
    .await;
    let error = error.unwrap_or_default();
    assert!(
        error.contains(MOCK_SCRIPTED_FAILURE),
        "the turn failed for its scripted reason, got: {error}"
    );
    let follow_up_at = sequence_of(&pool, &follow_up_id)
        .await
        .expect("the follow-up was persisted when its POST returned 200");
    assert!(
        follow_up_at < failed_at,
        "the follow-up must land inside the live turn for this test to say anything \
         (follow-up at {follow_up_at}, failure at {failed_at})"
    );

    let (_, reply) = wait_for_anchored(
        &pool,
        thread_id,
        "ResponseGenerated",
        &follow_up_id,
        "the follow-up typed into the failed turn was never answered",
    )
    .await;
    assert_eq!(reply.as_deref(), Some(MOCK_RESPONSE));
}
