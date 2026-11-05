//! E2E for the chat turn end's *read decision* (ADR 0417), on the mock model.
//!
//! What it pins, against a real engine:
//!
//! * a turn that ends without deciding gets one forced decision, recorded
//!   before its `ResponseGenerated`, and the reply stays the draft
//! * the forced call records its cost as a `read_decision` capture
//! * a reply that carries its own decision beside its text ends the turn in
//!   one round, with no forced call

use crate::support::{base_url, db_url, poll_thread_summary_by_marker, unique_marker, user_client};
use lucidos_engine::llm::mock::{
    MOCK_READ_BESIDE_REPLY_SENTINEL, MOCK_READ_YES_SENTINEL, MOCK_RESPONSE,
};
use uuid::Uuid;

/// Send `message` as a new chat turn and wait for its `ResponseGenerated`.
async fn finished_turn(pool: &sqlx::PgPool, label: &str, message: &str) -> Uuid {
    let marker = unique_marker(label);
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({ "message": format!("{marker} {message}"), "mode": "human" }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status(), 200);
    let thread_id = poll_thread_summary_by_marker(pool, &marker, 25)
        .await
        .thread_id;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while sequences(pool, thread_id, "ResponseGenerated")
        .await
        .is_empty()
    {
        assert!(std::time::Instant::now() < deadline, "the turn never ended");
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    thread_id
}

async fn sequences(pool: &sqlx::PgPool, thread_id: Uuid, event_type: &str) -> Vec<i64> {
    sqlx::query_scalar(
        "SELECT sequence FROM events \
          WHERE aggregate = 'thread' AND aggregate_id = $1 AND event_type = $2 \
          ORDER BY sequence",
    )
    .bind(thread_id.to_string())
    .bind(event_type)
    .fetch_all(pool)
    .await
    .expect("DB query failed")
}

async fn captures(pool: &sqlx::PgPool, thread_id: Uuid, clause: &str) -> i64 {
    sqlx::query_scalar(&format!(
        "SELECT count(*) FROM events \
          WHERE aggregate = 'thread' AND aggregate_id = $1 \
            AND event_type = 'ContextCaptured' AND {clause}"
    ))
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    .expect("DB query failed")
}

async fn reply_text(pool: &sqlx::PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar(
        "SELECT payload->>'text' FROM events \
          WHERE aggregate = 'thread' AND aggregate_id = $1 \
            AND event_type = 'ResponseGenerated'",
    )
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    .expect("DB query failed")
}

async fn connect() -> sqlx::PgPool {
    sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database")
}

#[tokio::test]
async fn an_undecided_turn_is_decided_before_its_response_and_keeps_its_reply() {
    let pool = connect().await;
    let thread_id = finished_turn(&pool, "api-read-decision-no", "tell me a pangram").await;

    let decided = sequences(&pool, thread_id, "ThreadReadNotRequested").await;
    let replied = sequences(&pool, thread_id, "ResponseGenerated").await;
    assert_eq!(decided.len(), 1, "exactly one decision per turn");
    assert!(
        sequences(&pool, thread_id, "ThreadReadRequested")
            .await
            .is_empty(),
        "the mock's forced answer is no"
    );
    assert!(decided[0] < replied[0], "decided before the response");
    assert_eq!(reply_text(&pool, thread_id).await, MOCK_RESPONSE);
    assert_eq!(
        captures(&pool, thread_id, "payload->>'purpose' = 'read_decision'").await,
        1,
        "the forced call records what it cost"
    );
}

#[tokio::test]
async fn a_forced_yes_lists_the_thread_for_review() {
    let pool = connect().await;
    let thread_id = finished_turn(
        &pool,
        "api-read-decision-yes",
        &format!("research it {MOCK_READ_YES_SENTINEL}"),
    )
    .await;
    assert_eq!(
        sequences(&pool, thread_id, "ThreadReadRequested")
            .await
            .len(),
        1
    );
    let read_requested: bool =
        sqlx::query_scalar("SELECT read_requested FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("DB query failed");
    assert!(read_requested);
}

#[tokio::test]
async fn a_reply_beside_its_decision_ends_the_turn_in_one_round() {
    let pool = connect().await;
    let thread_id = finished_turn(
        &pool,
        "api-read-decision-beside",
        MOCK_READ_BESIDE_REPLY_SENTINEL,
    )
    .await;

    assert_eq!(
        captures(&pool, thread_id, "payload->>'producer' = 'main_llm'").await,
        1,
        "no second round after the decision"
    );
    assert_eq!(
        captures(&pool, thread_id, "payload->>'purpose' = 'read_decision'").await,
        0,
        "the turn decided, so nothing is forced"
    );
    assert_eq!(
        sequences(&pool, thread_id, "ThreadReadRequested")
            .await
            .len(),
        1
    );
    assert_eq!(reply_text(&pool, thread_id).await, MOCK_RESPONSE);
}
