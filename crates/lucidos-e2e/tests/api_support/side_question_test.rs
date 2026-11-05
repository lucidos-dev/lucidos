//! E2E coverage for `POST /api/v1/side-questions` (ADR 0320), and for `/btw`
//! text on the chat route, which is an ordinary message.
//!
//! A Lucidos Agent thread is answered here by the mock model. Every Claude
//! Code case is a refusal the engine gives before any Claude Code runs, so it
//! needs no real session. Each refusal also checks that nothing was recorded.

use crate::support::{
    base_url, db_url, seed_cc_thread_summary, seed_chat_thread_summary, user_client,
};
use serde_json::{json, Value};
use uuid::Uuid;

async fn post(path: &str, body: Value) -> (u16, Value) {
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/{path}", base_url()))
        .json(&body)
        .send()
        .await
        .expect("side-question request failed");
    let status = resp.status().as_u16();
    (
        status,
        resp.json().await.expect("side-question body is JSON"),
    )
}

async fn ask(thread_id: &str, question: &str) -> (u16, Value) {
    let body = json!({
        "thread_id": thread_id,
        "side_question_id": Uuid::new_v4(),
        "question": question,
    });
    post("side-questions", body).await
}

/// The seed helpers leave a row at the column default, a started thread. Only
/// `ThreadStarted` records a composing draft, so a draft is marked explicitly.
async fn mark_composing(pool: &sqlx::PgPool, thread_id: Uuid) {
    sqlx::query("UPDATE thread_summaries SET state = 'composing' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(pool)
        .await
        .expect("mark the thread a composing draft");
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

/// The side-question event types recorded on a thread, oldest first.
async fn recorded_types(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT event_type FROM events WHERE thread_id = $1 ORDER BY sequence")
        .bind(thread_id)
        .fetch_all(pool)
        .await
        .expect("read events")
}

/// A Lucidos Agent thread answers from its own model. The ask and the answer
/// are recorded as a card with the cost beside it, and never as a turn.
#[tokio::test]
async fn a_lucidos_agent_thread_answers_a_side_question() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(&pool, thread_id, "idle").await;

    let (status, body) = ask(&thread_id.to_string(), "what is X?").await;
    assert_eq!(status, 200, "{body}");
    assert!(
        !body["answer"].as_str().unwrap_or_default().is_empty(),
        "{body}"
    );
    assert_eq!(
        recorded_types(&pool, thread_id).await,
        [
            "SideQuestionAsked",
            "ContextCaptured",
            "SideQuestionAnswered"
        ]
    );
    let purpose: String = sqlx::query_scalar(
        "SELECT payload->>'purpose' FROM events \
         WHERE thread_id = $1 AND event_type = 'ContextCaptured'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(purpose, "side_question");
}

/// A draft still composing has not started, so it takes no side question,
/// and nothing is recorded on it.
#[tokio::test]
async fn a_composing_draft_is_refused() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(&pool, thread_id, "idle").await;
    mark_composing(&pool, thread_id).await;

    let (status, body) = ask(&thread_id.to_string(), "what is X?").await;
    assert_eq!(status, 400, "{body}");
    assert!(
        error_of(&body).contains("once this thread has started"),
        "{body}"
    );
    assert_eq!(thread_event_count(&pool, thread_id).await, 0);
}

/// An ask naming an image the workspace never received is refused by name,
/// before anything is recorded.
#[tokio::test]
async fn a_side_question_naming_an_unknown_image_is_refused() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(&pool, thread_id, "idle").await;
    let hash = "e".repeat(64);

    let body = json!({
        "thread_id": thread_id,
        "side_question_id": Uuid::new_v4(),
        "question": "what is this?",
        "image_hashes": [hash],
    });
    let (status, body) = post("side-questions", body).await;
    assert_eq!(status, 400, "{body}");
    assert!(error_of(&body).contains(&hash), "{body}");
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
    assert!(
        error_of(&body).contains("Type the side question first"),
        "{body}"
    );
    assert_eq!(thread_event_count(&pool, thread_id).await, 0);
}

/// Only a recorded ask can be dismissed, and a refused dismissal records
/// nothing.
#[tokio::test]
async fn dismissing_a_side_question_nobody_asked_is_not_found() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "idle").await;

    let body = json!({ "thread_id": thread_id, "side_question_id": Uuid::new_v4() });
    let (status, body) = post("side-questions/dismiss", body).await;
    assert_eq!(status, 404, "{body}");
    assert!(
        error_of(&body).contains("No side question with this id"),
        "{body}"
    );
    assert_eq!(thread_event_count(&pool, thread_id).await, 0);
}

/// The chat route sends `/btw` text as an ordinary message.
#[tokio::test]
async fn the_chat_route_sends_btw_text_as_an_ordinary_message() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(&pool, thread_id, "idle").await;

    let message = "/btw what is X?";
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&json!({
            "message": message,
            "mode": "human",
            "thread_id": thread_id.to_string(),
        }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status().as_u16(), 200);
    drop(resp);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let text: Option<String> = sqlx::query_scalar(
            "SELECT payload->>'text' FROM events WHERE thread_id = $1 AND event_type = 'MessageReceived' LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(&pool)
        .await
        .expect("message lookup");
        if let Some(text) = text {
            assert_eq!(text, message);
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no MessageReceived landed on thread {thread_id}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
