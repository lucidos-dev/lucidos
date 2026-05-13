//! E2E for POST /api/internal/ask-user-question — the long-poll endpoint
//! invoked by the lucidos-cli ask-user-question-hook subcommand from inside
//! CC subprocesses. Drives the endpoint with HTTP only — no real CC needed.

use crate::support::{base_url, db_url, http_client, seed_cc_thread_summary};
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

#[tokio::test]
async fn long_poll_returns_answer_when_user_responds() {
    let client = http_client();
    let pool = PgPool::connect(&db_url()).await.expect("db connect");
    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("toolu_e2e_{}", thread_id.simple());
    // The hook emits UserQuestionAsked / accepts the answer keyed on a
    // synthetic per-question id `{outer}#q{i}`. Even with one question we
    // address `#q0`. See `synth_question_id` in `api/internal.rs`.
    let q0_id = format!("{tool_use_id}#q0");

    // Seed a SessionStarted so the lifecycle classifier treats this as a CC
    // thread (UserQuestionAsked is CC-only and would otherwise be rejected).
    seed_cc_thread_summary(&pool, thread_id, "running").await;

    // Background: wait for the hook endpoint to emit UserQuestionAsked, then
    // simulate the user answering. Polling the events table avoids a race
    // between the hook's emit and the answer-question handler's pending-question
    // lookup (which would otherwise 409).
    let client_bg = client.clone();
    let q0_id_bg = q0_id.clone();
    let pool_bg = pool.clone();
    let answerer = tokio::spawn(async move {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM events WHERE thread_id = $1 \
                 AND event_type = 'UserQuestionAsked' AND payload->>'tool_use_id' = $2)",
            )
            .bind(thread_id)
            .bind(&q0_id_bg)
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
                "tool_use_id": q0_id_bg,
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
async fn multi_select_question_returns_joined_answer() {
    let client = http_client();
    let pool = PgPool::connect(&db_url()).await.expect("db connect");
    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("toolu_multi_{}", thread_id.simple());
    let q0_id = format!("{tool_use_id}#q0");

    seed_cc_thread_summary(&pool, thread_id, "running").await;

    let client_bg = client.clone();
    let q0_id_bg = q0_id.clone();
    let pool_bg = pool.clone();
    let answerer = tokio::spawn(async move {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM events WHERE thread_id = $1 \
                 AND event_type = 'UserQuestionAsked' AND payload->>'tool_use_id' = $2)",
            )
            .bind(thread_id)
            .bind(&q0_id_bg)
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
                "tool_use_id": q0_id_bg,
                "answer": { "kind": "MultiSelected", "option_ids": ["opt-0", "opt-1"] }
            }))
            .send()
            .await
            .expect("answer post");
        assert_eq!(resp.status().as_u16(), 200, "MultiSelected answer should accept");
    });

    let resp = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .post(format!("{}/api/internal/ask-user-question", base_url()))
            .json(&json!({
                "thread_id": thread_id.to_string(),
                "tool_use_id": tool_use_id,
                "session_id": "sid-multi",
                "questions": [{
                    "question": "Pick all that apply",
                    "header": "multi",
                    "multiSelect": true,
                    "options": [
                        {"label": "Red", "description": ""},
                        {"label": "Blue", "description": ""}
                    ]
                }]
            }))
            .send(),
    )
    .await
    .expect("did not time out")
    .expect("hook post");

    answerer.await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["answers"],
        json!({"Pick all that apply": "Red, Blue"}),
        "joined labels should round-trip to the hook output"
    );

    sqlx::query("DELETE FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
async fn multi_select_empty_answer_is_rejected() {
    let client = http_client();
    let pool = PgPool::connect(&db_url()).await.expect("db connect");
    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("toolu_multi_empty_{}", thread_id.simple());
    let q0_id = format!("{tool_use_id}#q0");

    seed_cc_thread_summary(&pool, thread_id, "running").await;

    // Pre-seed the question so answer-question can find it (we don't want to
    // race the long-poll endpoint here — this is purely a validation test).
    // Use the synthetic per-question id `#q0` because that's what the hook
    // endpoint emits and what the answer endpoint matches against.
    sqlx::query(
        "INSERT INTO events (id, thread_id, event_type, payload, created, aggregate, aggregate_id)
         VALUES ($1, $2, 'UserQuestionAsked', $3, NOW(), 'thread', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(thread_id)
    .bind(json!({
        "tool_use_id": q0_id,
        "cc_session_id": "sid-multi-empty",
        "question": "Pick all that apply",
        "options": [
            {"id": "opt-0", "label": "Red"},
            {"id": "opt-1", "label": "Blue"}
        ],
        "multi_select": true
    }))
    .bind(thread_id.to_string())
    .execute(&pool)
    .await
    .expect("insert UserQuestionAsked");

    let resp = client
        .post(format!("{}/api/claude-code/answer-question", base_url()))
        .json(&json!({
            "thread_id": thread_id.to_string(),
            "tool_use_id": q0_id,
            "answer": { "kind": "MultiSelected", "option_ids": [] }
        }))
        .send()
        .await
        .expect("answer post");
    assert_eq!(
        resp.status().as_u16(),
        409,
        "empty MultiSelected option_ids must reject as conflict"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["error"]
            .as_str()
            .map(|s| s.contains("at least one"))
            .unwrap_or(false),
        "error must explain the requirement; got {body:?}"
    );

    sqlx::query("DELETE FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
async fn returns_immediately_when_answer_already_persisted() {
    let client = http_client();
    let pool = PgPool::connect(&db_url()).await.expect("db connect");
    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("toolu_recovery_{}", thread_id.simple());
    // Crash-recovery answer is keyed on the synthetic `#q0` id — the hook
    // re-POSTs the same outer tool_use_id on restart, the handler re-derives
    // `#q0` and finds the persisted UserQuestionAnswered.
    let q0_id = format!("{tool_use_id}#q0");

    // Pre-insert a UserQuestionAnswered event directly to simulate "engine
    // restarted; the user already answered before crash".
    sqlx::query(
        "INSERT INTO events (id, thread_id, event_type, payload, created, aggregate, aggregate_id)
         VALUES ($1, $2, 'UserQuestionAnswered', $3, NOW(), 'thread', $4)"
    )
        .bind(Uuid::new_v4())
        .bind(thread_id)
        .bind(json!({
            "tool_use_id": q0_id,
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

    // Fast path = "no long-poll", not "sub-1s round-trip" — under parallel
    // test load HTTPS handshake + DB lookup can take ~1s. Long-poll timeout
    // is much higher; 5s leaves headroom while still catching regressions.
    assert!(start.elapsed() < Duration::from_secs(5),
            "should not block when answer already exists; took {:?}", start.elapsed());

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["answers"], json!({"Fav color?": "Blue"}));

    sqlx::query("DELETE FROM events WHERE thread_id = $1")
        .bind(thread_id).execute(&pool).await.unwrap();
}
