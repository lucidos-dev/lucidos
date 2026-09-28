//! E2E coverage for `POST /api/v1/coding-agents/side-question` and the chat
//! route's `/btw` guard (ADR 0318).
//!
//! Every case here is a refusal the engine gives before any Claude Code runs,
//! so it needs no real session. The answered path runs against a real Claude
//! Code in `crates/lucidos-app/e2e/side-question.spec.ts`. Each case also
//! checks that nothing was recorded on the thread.

use crate::support::{
    base_url, db_url, seed_cc_thread_summary, seed_chat_thread_summary, user_client,
};
use serde_json::{json, Value};
use uuid::Uuid;

async fn ask(thread_id: &str, question: &str) -> (u16, Value) {
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/coding-agents/side-question", base_url()))
        .json(&json!({ "thread_id": thread_id, "question": question }))
        .send()
        .await
        .expect("side-question request failed");
    let status = resp.status().as_u16();
    (
        status,
        resp.json().await.expect("side-question body is JSON"),
    )
}

async fn thread_event_count(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("count events")
}

fn error_of(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn a_malformed_thread_id_is_refused() {
    let (status, body) = ask("not-a-uuid", "q").await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(error_of(&body), "Invalid thread_id");
}

#[tokio::test]
async fn a_chat_thread_cannot_take_a_side_question() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(&pool, thread_id, "idle").await;

    let (status, body) = ask(&thread_id.to_string(), "what is X?").await;
    assert_eq!(status, 400, "{body}");
    assert!(
        error_of(&body).contains("only in Claude Code threads"),
        "{body}"
    );
    assert_eq!(thread_event_count(&pool, thread_id).await, 0);
}

#[tokio::test]
async fn a_codex_thread_says_side_questions_are_not_available() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "idle").await;
    sqlx::query("UPDATE thread_summaries SET coding_agent = 'codex' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();

    let (status, body) = ask(&thread_id.to_string(), "what is X?").await;
    assert_eq!(status, 400, "{body}");
    assert!(
        error_of(&body).contains("not available in Codex threads"),
        "{body}"
    );
    assert_eq!(thread_event_count(&pool, thread_id).await, 0);
}

#[tokio::test]
async fn a_claude_code_thread_with_no_session_yet_is_refused_without_spawning() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "idle").await;

    let (status, body) = ask(&thread_id.to_string(), "what is X?").await;
    assert_eq!(status, 400, "{body}");
    assert!(
        error_of(&body).contains("no Claude Code session yet"),
        "{body}"
    );
    let (status, body) = ask(&thread_id.to_string(), "   ").await;
    assert_eq!(status, 400, "{body}");
    assert!(error_of(&body).contains("Type a question"), "{body}");
    assert_eq!(thread_event_count(&pool, thread_id).await, 0);
}

/// A caller that skips the composer must not turn `/btw` into a main-session
/// turn: the chat route refuses it before recording anything.
#[tokio::test]
async fn the_chat_route_refuses_btw_in_a_coding_agent_thread() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "idle").await;

    let resp = user_client()
        .await
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&json!({
            "message": "/btw what is X?",
            "mode": "human",
            "thread_id": thread_id.to_string(),
        }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status().as_u16(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["reason"], "side-question", "{body}");
    assert_eq!(thread_event_count(&pool, thread_id).await, 0);
}

/// Creating a coding-agent thread with `/btw` as its first message is refused
/// too. Otherwise it would open the session as its first turn.
#[tokio::test]
async fn the_chat_route_refuses_btw_that_would_create_a_coding_agent_thread() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let thread_id = Uuid::new_v4();

    let resp = user_client()
        .await
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&json!({
            "message": "/btw what is X?",
            "mode": "human",
            "thread_id": thread_id.to_string(),
            "new_thread": true,
            "use_coding_agent": true,
        }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status().as_u16(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["reason"], "side-question", "{body}");
    assert_eq!(thread_event_count(&pool, thread_id).await, 0);
}
