//! E2E for POST /api/claude-code/answer-question. We can't easily spawn a real
//! CC subprocess from a test, so we drive the endpoint by inserting a synthetic
//! `UserQuestionAsked` event directly into the events table for a CC thread.
//! The endpoint should:
//!   - 200 + emit `UserQuestionAnswered` for valid Selected/FreeText/Canceled
//!   - 409 when no pending question exists
//!   - 409 when a question is already answered (idempotency)
//!
//! For the success cases we use `AnswerKind::Canceled`, which short-circuits
//! the resume path (no fresh CC spawn) and just emits `UserQuestionAnswered`.

use crate::support::{base_url, db_url, http_client};
use uuid::Uuid;

async fn insert_user_question_asked(pool: &sqlx::PgPool, thread_id: Uuid, tool_use_id: &str) {
    // Required: thread_summaries row (claude_code source) so the projection
    // doesn't choke on UPDATE…WHERE thread_id = $1.
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, source, is_cc, created_at, last_activity, message_count, status) \
         VALUES ($1, 'claude_code', TRUE, NOW(), NOW(), 0, 'waiting_for_user_answer') \
         ON CONFLICT (thread_id) DO UPDATE SET status = 'waiting_for_user_answer'"
    )
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("failed to upsert thread_summaries");

    // Insert the question event directly. We bypass the bus to keep the test
    // hermetic — the API handler reads back from events, so this is sufficient.
    let payload = serde_json::json!({
        "tool_use_id": tool_use_id,
        "cc_session_id": "sess_e2e",
        "question": "Pick one:",
        "options": [
            { "id": "opt-0", "label": "Yes" },
            { "id": "opt-1", "label": "No" },
        ],
    });
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2::text, 'UserQuestionAsked', $3, NOW(), $2)"
    )
    .bind(Uuid::new_v4())
    .bind(thread_id)
    .bind(payload)
    .execute(pool)
    .await
    .expect("failed to insert UserQuestionAsked");
}

async fn count_answered(pool: &sqlx::PgPool, thread_id: Uuid, tool_use_id: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM events \
         WHERE thread_id = $1 AND event_type = 'UserQuestionAnswered' \
           AND payload->>'tool_use_id' = $2",
    )
    .bind(thread_id)
    .bind(tool_use_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0)
}

#[tokio::test]
#[ignore]
async fn answer_question_canceled_emits_answered_event() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("tu-cancel-{}", &Uuid::new_v4().as_simple().to_string()[..8]);
    insert_user_question_asked(&pool, thread_id, &tool_use_id).await;

    let url = format!("{}/api/claude-code/answer-question", base_url());
    let body = serde_json::json!({
        "thread_id": thread_id.to_string(),
        "tool_use_id": tool_use_id,
        "answer": { "kind": "Canceled" }
    });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "Canceled answer should succeed"
    );

    // Wait briefly for emit (synchronous in handler, but DB roundtrip).
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let answered = count_answered(&pool, thread_id, &tool_use_id).await;
    assert_eq!(
        answered, 1,
        "exactly one UserQuestionAnswered event must exist"
    );
}

#[tokio::test]
#[ignore]
async fn answer_question_missing_returns_409() {
    let client = http_client();
    let url = format!("{}/api/claude-code/answer-question", base_url());
    let body = serde_json::json!({
        "thread_id": Uuid::new_v4().to_string(),
        "tool_use_id": "does-not-exist",
        "answer": { "kind": "Canceled" }
    });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("request failed");
    assert_eq!(resp.status().as_u16(), 409, "missing question should 409");
}

#[tokio::test]
#[ignore]
async fn dismiss_with_pending_question_emits_canceled_answer() {
    // Bug: a CC thread sitting in WaitingForUserAnswer had no Done button
    // and dismissing didn't resolve the question card. Dismiss must auto-cancel
    // any pending question so the card resolves cleanly to "Canceled" and the
    // thread can leave REVIEW.
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    let tool_use_id = format!(
        "tu-dismiss-{}",
        &Uuid::new_v4().as_simple().to_string()[..8]
    );
    insert_user_question_asked(&pool, thread_id, &tool_use_id).await;

    let url = format!("{}/api/threads/dismiss", base_url());
    let body = serde_json::json!({ "thread_id": thread_id.to_string() });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("request failed");
    assert_eq!(resp.status().as_u16(), 200, "dismiss should succeed");

    // Wait for both UserQuestionAnswered (Canceled) and ThreadDismissed to land.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let answered = count_answered(&pool, thread_id, &tool_use_id).await;
    assert_eq!(
        answered, 1,
        "dismiss must emit UserQuestionAnswered to resolve the question card"
    );

    let canceled_kind: Option<String> = sqlx::query_scalar(
        "SELECT payload->'answer'->>'kind' FROM events \
         WHERE thread_id = $1 AND event_type = 'UserQuestionAnswered' \
           AND payload->>'tool_use_id' = $2 LIMIT 1",
    )
    .bind(thread_id)
    .bind(&tool_use_id)
    .fetch_one(&pool)
    .await
    .expect("answer event must exist");
    assert_eq!(
        canceled_kind.as_deref(),
        Some("Canceled"),
        "answer kind must be Canceled"
    );

    let dismissed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'ThreadDismissed'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap_or(0);
    assert_eq!(dismissed, 1, "ThreadDismissed must still be emitted");
}

#[tokio::test]
#[ignore]
async fn answer_question_idempotent_409_on_duplicate() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("tu-dup-{}", &Uuid::new_v4().as_simple().to_string()[..8]);
    insert_user_question_asked(&pool, thread_id, &tool_use_id).await;

    let url = format!("{}/api/claude-code/answer-question", base_url());
    let body = serde_json::json!({
        "thread_id": thread_id.to_string(),
        "tool_use_id": tool_use_id,
        "answer": { "kind": "Canceled" }
    });
    let first = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("request failed");
    assert_eq!(first.status().as_u16(), 200);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Second click of the same option should be idempotent — 409, not 500.
    let second = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("request failed");
    assert_eq!(second.status().as_u16(), 409, "double-answer should 409");

    // Still exactly one answer recorded.
    let answered = count_answered(&pool, thread_id, &tool_use_id).await;
    assert_eq!(answered, 1, "duplicate answer must not double-write");
}
