//! E2E for POST /api/v1/threads/{thread_id}/answer-question. We can't easily
//! spawn a real CC subprocess from a test, so we drive the endpoint by
//! inserting a synthetic `UserQuestionAsked` event directly into the events
//! table. The endpoint should:
//!   - 200 + emit `UserQuestionAnswered` for valid Selected/FreeText/Canceled
//!   - 409 when no pending question exists
//!   - 409 when a question is already answered (idempotency)
//!   - emit `CodingAgentPromptSent` only on CC-channel questions (the
//!     chat agent's `ask_user_question` tool is in-process and needs no
//!     timeline placeholder while it processes the answer)
//!
//! For the success cases we use `AnswerKind::Canceled`, which short-circuits
//! the resume path (no fresh CC spawn) and just emits `UserQuestionAnswered`.

use crate::support::{
    base_url, db_url, http_client, seed_cc_thread_summary, seed_chat_thread_summary,
};
use uuid::Uuid;

async fn insert_user_question_asked(pool: &sqlx::PgPool, thread_id: Uuid, tool_use_id: &str) {
    seed_cc_thread_summary(pool, thread_id, "waiting_for_user_answer").await;

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
        "channel": "claude_code",
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

/// Same shape as `insert_user_question_asked`, but on a chat thread with
/// `channel: "chat"` so the answer endpoint exercises the in-process path
/// (no `CodingAgentPromptSent` marker, no `ContinuationRequested` spawn).
async fn insert_chat_user_question_asked(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
) {
    seed_chat_thread_summary(pool, thread_id, "waiting_for_user_answer").await;

    let payload = serde_json::json!({
        "tool_use_id": tool_use_id,
        "cc_session_id": "",
        "question": "Pick one:",
        "options": [
            { "id": "opt-0", "label": "Yes" },
            { "id": "opt-1", "label": "No" },
        ],
        "channel": "chat",
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
    .expect("failed to insert chat UserQuestionAsked");
}

fn answer_question_url(thread_id: Uuid) -> String {
    format!("{}/api/v1/threads/{}/answer-question", base_url(), thread_id)
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

/// Count CodingAgentPromptSent events for a thread. After answering a
/// question with an active answer (Selected / FreeText / MultiSelected),
/// the engine emits one to surface a "Thinking" spinner in the timeline
/// while CC processes the tool_result. `AnswerKind::Canceled` skips the
/// marker — no CC turn follows, the QuestionCard's own ✓ Cancel state
/// already conveys the outcome (see `emit_resume_marker_for_cc_answer`).
async fn count_resume_marker(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM events \
         WHERE thread_id = $1 AND event_type = 'CodingAgentPromptSent'",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0)
}

#[tokio::test]
async fn answer_question_canceled_emits_answered_event_without_resume_marker() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("tu-cancel-{}", &Uuid::new_v4().as_simple().to_string()[..8]);
    insert_user_question_asked(&pool, thread_id, &tool_use_id).await;

    let body = serde_json::json!({
        "tool_use_id": tool_use_id,
        "answer": { "kind": "Canceled" }
    });
    let resp = client
        .post(answer_question_url(thread_id))
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
    let resume_markers = count_resume_marker(&pool, thread_id).await;
    assert_eq!(
        resume_markers, 0,
        "Canceled must skip the CodingAgentPromptSent marker — no CC turn follows, so the marker would strand as an empty 'Thinking ✓' under the QuestionCard's own ✓ Cancel state"
    );
}

#[tokio::test]
async fn answer_question_missing_returns_409() {
    let client = http_client();
    let body = serde_json::json!({
        "tool_use_id": "does-not-exist",
        "answer": { "kind": "Canceled" }
    });
    let resp = client
        .post(answer_question_url(Uuid::new_v4()))
        .json(&body)
        .send()
        .await
        .expect("request failed");
    assert_eq!(resp.status().as_u16(), 409, "missing question should 409");
}

/// Chat-channel questions are raised by the chat agent's in-process
/// `ask_user_question` tool. The tool blocks on the question wait registry
/// and returns the answer as a tool_result on the same turn — no CC
/// subprocess to respawn, no timeline placeholder needed. The answer-side
/// must skip both `CodingAgentPromptSent` and `ContinuationRequested` for
/// these channels (`should_emit_cc_resume_side_effects` in agent_question.rs).
#[tokio::test]
async fn answer_question_chat_channel_skips_cc_resume_marker() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("tu-chat-{}", &Uuid::new_v4().as_simple().to_string()[..8]);
    insert_chat_user_question_asked(&pool, thread_id, &tool_use_id).await;

    let body = serde_json::json!({
        "tool_use_id": tool_use_id,
        "answer": { "kind": "Selected", "option_id": "opt-0" }
    });
    let resp = client
        .post(answer_question_url(thread_id))
        .json(&body)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "chat-channel answer should succeed (got body: {:?})",
        resp.text().await.unwrap_or_default()
    );

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let answered = count_answered(&pool, thread_id, &tool_use_id).await;
    assert_eq!(
        answered, 1,
        "exactly one UserQuestionAnswered event must exist for the chat thread"
    );

    let answered_channel: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'channel' FROM events \
         WHERE thread_id = $1 AND event_type = 'UserQuestionAnswered' LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .ok()
    .flatten();
    assert_eq!(
        answered_channel.as_deref(),
        Some("chat"),
        "the answer event must carry the same channel as the question (chat)"
    );

    let resume_markers = count_resume_marker(&pool, thread_id).await;
    assert_eq!(
        resume_markers, 0,
        "chat-channel answer must NOT emit CodingAgentPromptSent — the chat tool is in-process and returns the answer directly as a tool_result"
    );

    let continue_signals: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'ContinuationRequested'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap_or(0);
    assert_eq!(
        continue_signals, 0,
        "chat-channel answer must NOT emit ContinuationRequested — chat has no subprocess to respawn"
    );
}

#[tokio::test]
async fn archive_with_pending_question_emits_canceled_answer() {
    // Bug: a CC thread sitting in WaitingForUserAnswer had no Archive button
    // and archiving didn't resolve the question card. Dismiss must auto-cancel
    // any pending question so the card resolves cleanly to "Canceled" and the
    // thread can leave REVIEW.
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    let tool_use_id = format!(
        "tu-archive-{}",
        &Uuid::new_v4().as_simple().to_string()[..8]
    );
    insert_user_question_asked(&pool, thread_id, &tool_use_id).await;

    let url = format!("{}/api/v1/threads/archive", base_url());
    let body = serde_json::json!({ "thread_id": thread_id.to_string() });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("request failed");
    assert_eq!(resp.status().as_u16(), 200, "archive should succeed");

    // Wait for both UserQuestionAnswered (Canceled) and ThreadArchived to land.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let answered = count_answered(&pool, thread_id, &tool_use_id).await;
    assert_eq!(
        answered, 1,
        "archive must emit UserQuestionAnswered to resolve the question card"
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

    let archived: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'ThreadArchived'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap_or(0);
    assert_eq!(archived, 1, "ThreadArchived must still be emitted");
}

#[tokio::test]
async fn answer_question_idempotent_409_on_duplicate() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("tu-dup-{}", &Uuid::new_v4().as_simple().to_string()[..8]);
    insert_user_question_asked(&pool, thread_id, &tool_use_id).await;

    let url = answer_question_url(thread_id);
    let body = serde_json::json!({
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

/// Regression: a chat thread (Lucidos Agent) with an active `UserQuestionAsked`
/// must route a typed follow-up sent through `POST /api/v1/chat/stream` as a
/// `FreeText` answer, not as a fresh `MessageReceived`. Without this, the
/// chat agent's `ask_user_question` tool stays blocked on the wait registry
/// and the thread sits stuck in "Requesting" forever.
///
/// The original gate in `chat::process` only routed when
/// `use_coding_agent == Some(true)`; chat threads (`None`/`Some(false)`) fell
/// through the gate, created a fresh exchange, and deadlocked. This test
/// pins the chat-channel path.
#[tokio::test]
async fn chat_freeform_followup_on_chat_thread_routes_to_pending_question() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    let tool_use_id = format!(
        "tu-chatft-{}",
        &Uuid::new_v4().as_simple().to_string()[..8]
    );
    insert_chat_user_question_asked(&pool, thread_id, &tool_use_id).await;

    let followup_text = format!("free-text follow-up {}", &Uuid::new_v4().as_simple().to_string()[..6]);
    let body = serde_json::json!({
        "message": followup_text,
        "mode": "human",
        "thread_id": thread_id.to_string(),
    });
    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&body)
        .send()
        .await
        .expect("chat stream request failed");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "chat stream should accept follow-up"
    );

    // Poll for the routed answer — the spawn-task path needs a moment to
    // re-enter the engine, look up the active question, and emit
    // UserQuestionAnswered. 5s is generous; the routing is a single DB
    // round-trip + emit, so this lands in well under a second locally.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let n = count_answered(&pool, thread_id, &tool_use_id).await;
        if n >= 1 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "UserQuestionAnswered never landed for chat follow-up — chat thread is stuck (the original bug)"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // The answer must be FreeText carrying the typed text, not Canceled or
    // Selected. This is what tells the chat agent the user's intent.
    let (kind, text): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT payload->'answer'->>'kind', payload->'answer'->>'text' \
         FROM events \
         WHERE thread_id = $1 AND event_type = 'UserQuestionAnswered' \
           AND payload->>'tool_use_id' = $2 LIMIT 1",
    )
    .bind(thread_id)
    .bind(&tool_use_id)
    .fetch_one(&pool)
    .await
    .expect("answer event must exist");
    assert_eq!(kind.as_deref(), Some("FreeText"), "must be a FreeText answer");
    assert_eq!(
        text.as_deref(),
        Some(followup_text.as_str()),
        "FreeText.text must carry the user's typed message verbatim"
    );

    // No fresh MessageReceived may have been emitted — the typed text became
    // the answer, not a new exchange. (insert_chat_user_question_asked only
    // inserts UserQuestionAsked; any MessageReceived rows are noise from the
    // routing failing and falling through to the normal chat path.)
    let mr_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'MessageReceived'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap_or(0);
    assert_eq!(
        mr_count, 0,
        "free-text routing must NOT emit a fresh MessageReceived — it must be absorbed as the answer"
    );
}

/// Regression: canceling a chat thread (Lucidos Agent) that's sitting on a
/// pending `UserQuestionAsked` must resolve the question as `Canceled` BEFORE
/// firing the cancel token. The chat agent's `ask_user_question` tool blocks
/// on `walk_question_batch.recv()` with no cancel-aware select, so firing the
/// token alone leaves the tool deadlocked and the UI hangs in "Canceling…".
///
/// Mirrors `claude_code_stop`'s pattern of calling
/// `resolve_pending_question_as_canceled` first.
#[tokio::test]
async fn cancel_chat_with_pending_question_resolves_card_as_canceled() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    let tool_use_id = format!(
        "tu-cancelchat-{}",
        &Uuid::new_v4().as_simple().to_string()[..8]
    );
    insert_chat_user_question_asked(&pool, thread_id, &tool_use_id).await;

    let resp = client
        .post(format!(
            "{}/api/v1/chat/cancel?thread_id={}",
            base_url(),
            thread_id
        ))
        .send()
        .await
        .expect("cancel request failed");
    assert_eq!(resp.status().as_u16(), 200, "chat cancel should return 200");
    // Resolving the pending question card IS a status-changing effect, so the
    // honest response must report `canceled: true` — the client keeps its
    // optimistic "canceling" state (the card resolution is the incoming event)
    // rather than treating the click as a stale no-op.
    let body: serde_json::Value = resp.json().await.expect("cancel response must be JSON");
    assert_eq!(
        body["canceled"], true,
        "cancel that resolved a pending question must report canceled=true"
    );

    // Poll for the auto-canceled answer (5s — same budget as the freeform
    // routing above; the emit path is a single DB round-trip).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let n = count_answered(&pool, thread_id, &tool_use_id).await;
        if n >= 1 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "cancel did not emit UserQuestionAnswered — chat agent's blocked tool would deadlock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

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
        "cancel must resolve the pending question with AnswerKind::Canceled"
    );
}

/// The uncancelable-thread wedge fix: a Stop click on a chat thread that has
/// nothing to cancel (idle, no pending question, no live turn) must report
/// `{"canceled": false}` — a bodyless/`true` 200 leaves the client's optimistic
/// "canceling" flag stuck, disabling the button while the thread keeps going.
/// It must NOT fabricate a terminal event on an already-idle thread.
#[tokio::test]
async fn cancel_chat_idle_thread_reports_not_canceled() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    // Idle chat thread, no pending question, no live session.
    seed_chat_thread_summary(&pool, thread_id, "idle").await;

    let before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap_or(0);

    let resp = client
        .post(format!(
            "{}/api/v1/chat/cancel?thread_id={}",
            base_url(),
            thread_id
        ))
        .send()
        .await
        .expect("cancel request failed");
    assert_eq!(resp.status().as_u16(), 200, "chat cancel should return 200");
    let body: serde_json::Value = resp.json().await.expect("cancel response must be JSON");
    assert_eq!(
        body["canceled"], false,
        "cancel on an idle thread with nothing to cancel must report canceled=false so the client re-syncs"
    );

    // No junk terminal event fabricated on an already-idle thread.
    let after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap_or(0);
    assert_eq!(
        after, before,
        "a no-op cancel must not emit any event on an idle thread"
    );
}
