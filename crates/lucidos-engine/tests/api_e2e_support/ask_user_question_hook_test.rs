//! E2E for POST /api/internal/ask-user-question — the long-poll endpoint
//! invoked by the lucidos-cli ask-user-question-hook subcommand from inside
//! CC subprocesses. Drives the endpoint with HTTP only — no real CC needed.

use crate::support::{base_url, db_url, http_client};
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

/// Seed a thread_summaries row classified as CC so the lifecycle classifier
/// treats this as a CC thread. UserQuestionAsked is CC-only — without a CC
/// classification, the lifecycle layer rejects the emit and the hook endpoint
/// silently no-ops.
async fn seed_cc_session(pool: &PgPool, thread_id: Uuid, _session_id: &str) {
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, source, is_cc, created_at, last_activity, message_count, status) \
         VALUES ($1, 'claude_code', TRUE, NOW(), NOW(), 0, 'running') \
         ON CONFLICT (thread_id) DO UPDATE SET source = 'claude_code', is_cc = TRUE",
    )
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("seed thread_summaries");
}

#[tokio::test]
#[ignore]
async fn long_poll_returns_answer_when_user_responds() {
    let client = http_client();
    let pool = PgPool::connect(&db_url()).await.expect("db connect");
    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("toolu_e2e_{}", thread_id.simple());

    // Seed a SessionStarted so the lifecycle classifier treats this as a CC
    // thread (UserQuestionAsked is CC-only and would otherwise be rejected).
    seed_cc_session(&pool, thread_id, "sid-1").await;

    // Background: wait for the hook endpoint to emit UserQuestionAsked, then
    // simulate the user answering. Polling the events table avoids a race
    // between the hook's emit and the answer-question handler's pending-question
    // lookup (which would otherwise 409).
    let client_bg = client.clone();
    let tool_use_id_bg = tool_use_id.clone();
    let pool_bg = pool.clone();
    let answerer = tokio::spawn(async move {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM events WHERE thread_id = $1 \
                 AND event_type = 'UserQuestionAsked' AND payload->>'tool_use_id' = $2)",
            )
            .bind(thread_id)
            .bind(&tool_use_id_bg)
            .fetch_one(&pool_bg)
            .await
            .unwrap_or(false);
            if exists {
                break;
            }
            if std::time::Instant::now() >= deadline {
                panic!("UserQuestionAsked never persisted by the hook endpoint");
            }
            sleep(Duration::from_millis(50)).await;
        }
        let resp = client_bg
            .post(format!("{}/api/claude-code/answer-question", base_url()))
            .json(&json!({
                "thread_id": thread_id.to_string(),
                "tool_use_id": tool_use_id_bg,
                "answer": { "kind": "Selected", "option_id": "opt-0" }
            }))
            .send().await.expect("answer post");
        assert_eq!(resp.status().as_u16(), 200, "answer-question should accept");
    });

    // Hook side — blocks until the answer arrives (or test timeout).
    let resp = tokio::time::timeout(
        Duration::from_secs(5),
        client.post(format!("{}/api/internal/ask-user-question", base_url()))
            .json(&json!({
                "thread_id": thread_id.to_string(),
                "tool_use_id": tool_use_id,
                "session_id": "sid-1",
                "questions": [{
                    "question": "Fav color?",
                    "header": "color",
                    "multiSelect": false,
                    "options": [
                        {"label": "Red", "description": ""},
                        {"label": "Blue", "description": ""}
                    ]
                }]
            }))
            .send()
    ).await.expect("did not time out").expect("hook post");

    answerer.await.unwrap();
    assert_eq!(resp.status().as_u16(), 200, "hook should get 200");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["answers"], json!({"Fav color?": "Red"}),
               "hook output should contain {{question: label}}");

    // Cleanup
    sqlx::query("DELETE FROM events WHERE thread_id = $1")
        .bind(thread_id).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id).execute(&pool).await.ok();
}

#[tokio::test]
#[ignore]
async fn returns_immediately_when_answer_already_persisted() {
    let client = http_client();
    let pool = PgPool::connect(&db_url()).await.expect("db connect");
    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("toolu_recovery_{}", thread_id.simple());

    // Pre-insert a UserQuestionAnswered event directly to simulate "engine
    // restarted; the user already answered before crash".
    sqlx::query(
        "INSERT INTO events (id, thread_id, event_type, payload, created, aggregate, aggregate_id)
         VALUES ($1, $2, 'UserQuestionAnswered', $3, NOW(), 'thread', $4)"
    )
        .bind(Uuid::new_v4())
        .bind(thread_id)
        .bind(json!({
            "tool_use_id": tool_use_id,
            "answer": { "kind": "Selected", "option_id": "opt-1" }
        }))
        .bind(thread_id.to_string())
        .execute(&pool).await.expect("insert event");

    let start = std::time::Instant::now();
    let resp = client.post(format!("{}/api/internal/ask-user-question", base_url()))
        .json(&json!({
            "thread_id": thread_id.to_string(),
            "tool_use_id": tool_use_id,
            "session_id": "sid-recovery",
            "questions": [{
                "question": "Fav color?",
                "header": "color",
                "multiSelect": false,
                "options": [
                    {"label": "Red", "description": ""},
                    {"label": "Blue", "description": ""}
                ]
            }]
        }))
        .send().await.expect("hook post");

    assert!(start.elapsed() < Duration::from_secs(1),
            "should not block when answer already exists; took {:?}", start.elapsed());

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["answers"], json!({"Fav color?": "Blue"}));

    sqlx::query("DELETE FROM events WHERE thread_id = $1")
        .bind(thread_id).execute(&pool).await.unwrap();
}
