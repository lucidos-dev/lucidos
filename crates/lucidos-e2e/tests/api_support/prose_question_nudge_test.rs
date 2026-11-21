//! E2E coverage for the prose-question nudge declined.
//!
//! A reply ending on a question with no card is sent back once. The draft has
//! already streamed by then, so a model keeping its open-ended question must
//! not show the user anything more: no justification, no second copy, and no
//! Thinking row under the draft. The turn ends on the draft. Only the real
//! agent loop streams, so only this reaches it.

use crate::support::{base_url, db_url, poll_thread_summary_by_marker, unique_marker, user_client};
use lucidos_engine::llm::mock::MOCK_PROSE_NUDGE_REASON;
use serde_json::json;

#[tokio::test]
async fn a_declined_prose_nudge_ends_on_the_draft_and_shows_nothing_more() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let client = user_client().await;

    let marker = unique_marker("api-prose-nudge");
    let question = "What are we working on tonight?";

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&json!({
            "message": format!("{marker} MOCK_ASK_IN_PROSE: {question}"),
            "mode": "human",
        }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status(), 200, "chat/stream should accept the message");

    let thread_id = poll_thread_summary_by_marker(&pool, &marker, 25)
        .await
        .thread_id;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let terminal: (String, serde_json::Value) = loop {
        let row: Option<(String, serde_json::Value)> = sqlx::query_as(
            "SELECT event_type, payload FROM events \
              WHERE aggregate = 'thread' AND aggregate_id = $1 \
                AND event_type IN ('ResponseGenerated', 'ResponseFailed', 'ResponseAborted') \
              ORDER BY sequence LIMIT 1",
        )
        .bind(thread_id.to_string())
        .fetch_optional(&pool)
        .await
        .expect("DB query failed");
        if let Some(row) = row {
            break row;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the nudged turn never terminated"
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    };
    assert_eq!(terminal.0, "ResponseGenerated", "{terminal:?}");
    assert_eq!(
        terminal.1["text"].as_str(),
        Some(question),
        "the turn must end on the draft the user already saw"
    );

    // `LlmCallRetried` lives on SSE only, so the nudge shows as a second
    // round: one main-model capture per call.
    let rounds: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events \
          WHERE aggregate = 'thread' AND aggregate_id = $1 \
            AND event_type = 'ContextCaptured' AND payload->>'producer' = 'main_llm'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("DB query failed");
    assert_eq!(
        rounds, 2,
        "the nudge must have sent the turn back once, or this test proves nothing"
    );

    let streamed: Vec<String> = sqlx::query_scalar(
        "SELECT payload->>'text' FROM events \
          WHERE aggregate = 'thread' AND aggregate_id = $1 \
            AND event_type = 'TextStreamed' \
          ORDER BY sequence",
    )
    .bind(thread_id.to_string())
    .fetch_all(&pool)
    .await
    .expect("DB query failed");
    let streamed = streamed.concat();
    assert_eq!(
        streamed, question,
        "the user sees the question once, and never the model's reason \
         ({MOCK_PROSE_NUDGE_REASON:?})"
    );

    // Each `ThoughtStreamed` draws a Thinking row. One under the finished
    // draft promises more, and the declined round delivers nothing.
    let thinking_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events \
          WHERE aggregate = 'thread' AND aggregate_id = $1 \
            AND event_type = 'ThoughtStreamed'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("DB query failed");
    assert_eq!(
        thinking_rows, 1,
        "the held-back round must not open a Thinking row under the draft"
    );
}
