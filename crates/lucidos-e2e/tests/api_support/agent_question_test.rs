use crate::support::{
    base_url, commit_on_new_branch, count_events_of_type, db_url, git, insert_session_started,
    seed_cc_thread_summary, seed_chat_thread_summary, user_client,
};
use uuid::Uuid;

async fn insert_user_question_asked(pool: &sqlx::PgPool, thread_id: Uuid, tool_use_id: &str) {
    insert_user_question_asked_on(pool, thread_id, tool_use_id, "waiting_for_user_answer").await;
}

async fn insert_user_question_asked_on(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
    status: &str,
) {
    seed_cc_thread_summary(pool, thread_id, status).await;

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

async fn insert_chat_user_question_asked(pool: &sqlx::PgPool, thread_id: Uuid, tool_use_id: &str) {
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
    format!(
        "{}/api/v1/threads/{}/answer-question",
        base_url(),
        thread_id
    )
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
    .expect("counting UserQuestionAnswered failed")
}

async fn count_resume_marker(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    count_events_of_type(pool, thread_id, "CodingAgentPromptSent").await
}

async fn count_all_events(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|e| panic!("counting events on thread {thread_id} failed: {e}"))
}

#[tokio::test]
async fn answer_question_canceled_emits_answered_event_without_resume_marker() {
    let client = user_client().await;
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
    let client = user_client().await;
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

#[tokio::test]
async fn answer_question_chat_channel_skips_cc_resume_marker() {
    let client = user_client().await;
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

    let continue_signals = count_events_of_type(&pool, thread_id, "ContinuationRequested").await;
    assert_eq!(
        continue_signals, 0,
        "chat-channel answer must NOT emit ContinuationRequested — chat has no subprocess to respawn"
    );
}

async fn post_archive(thread_id: Uuid) -> reqwest::Response {
    user_client()
        .await
        .post(format!("{}/api/v1/threads/archive", base_url()))
        .json(&serde_json::json!({ "thread_id": thread_id.to_string() }))
        .send()
        .await
        .expect("request failed")
}

#[tokio::test]
async fn archive_refuses_a_thread_waiting_on_a_question() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("tu-parked-{}", &Uuid::new_v4().as_simple().to_string()[..8]);
    insert_user_question_asked(&pool, thread_id, &tool_use_id).await;

    let resp = post_archive(thread_id).await;
    assert_eq!(
        resp.status().as_u16(),
        409,
        "archive must refuse a parked thread"
    );
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["reason"], "parent_not_archivable");
    assert_eq!(body["parent_status"], "waiting_for_user_answer");

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        count_answered(&pool, thread_id, &tool_use_id).await,
        0,
        "a refused archive must leave the question open"
    );
    assert_eq!(
        count_events_of_type(&pool, thread_id, "ThreadArchived").await,
        0
    );
    let (status, section): (String, String) =
        sqlx::query_as("SELECT status, archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(
        (status.as_str(), section.as_str()),
        ("waiting_for_user_answer", "inbox")
    );
}

#[tokio::test]
async fn archive_cancels_an_orphaned_question_card() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    let tool_use_id = format!(
        "tu-archive-{}",
        &Uuid::new_v4().as_simple().to_string()[..8]
    );
    insert_user_question_asked_on(&pool, thread_id, &tool_use_id, "idle").await;

    let resp = post_archive(thread_id).await;
    assert_eq!(resp.status().as_u16(), 200, "archive should succeed");

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

    let archived = count_events_of_type(&pool, thread_id, "ThreadArchived").await;
    assert_eq!(archived, 1, "ThreadArchived must still be emitted");
}

#[tokio::test]
async fn answer_question_idempotent_409_on_duplicate() {
    let client = user_client().await;
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

    let second = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("request failed");
    assert_eq!(second.status().as_u16(), 409, "double-answer should 409");

    let answered = count_answered(&pool, thread_id, &tool_use_id).await;
    assert_eq!(answered, 1, "duplicate answer must not double-write");
}

#[tokio::test]
async fn chat_freeform_followup_on_chat_thread_routes_to_pending_question() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("tu-chatft-{}", &Uuid::new_v4().as_simple().to_string()[..8]);
    insert_chat_user_question_asked(&pool, thread_id, &tool_use_id).await;

    let followup_text = format!(
        "free-text follow-up {}",
        &Uuid::new_v4().as_simple().to_string()[..6]
    );
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
    assert_eq!(
        kind.as_deref(),
        Some("FreeText"),
        "must be a FreeText answer"
    );
    assert_eq!(
        text.as_deref(),
        Some(followup_text.as_str()),
        "FreeText.text must carry the user's typed message verbatim"
    );

    let mr_count = count_events_of_type(&pool, thread_id, "MessageReceived").await;
    assert_eq!(
        mr_count, 0,
        "free-text routing must NOT emit a fresh MessageReceived — it must be absorbed as the answer"
    );
}

#[tokio::test]
async fn cancel_chat_with_pending_question_resolves_card_as_canceled() {
    let client = user_client().await;
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
    let body: serde_json::Value = resp.json().await.expect("cancel response must be JSON");
    assert_eq!(
        body["canceled"], true,
        "cancel that resolved a pending question must report canceled=true"
    );

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

#[tokio::test]
async fn cancel_chat_idle_thread_reports_not_canceled() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(&pool, thread_id, "idle").await;

    let before = count_all_events(&pool, thread_id).await;

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

    let after = count_all_events(&pool, thread_id).await;
    assert_eq!(
        after, before,
        "a no-op cancel must not emit any event on an idle thread"
    );
}

async fn await_event_payload(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    event_type: &str,
) -> serde_json::Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let row: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT payload FROM events WHERE thread_id = $1 AND event_type = $2 LIMIT 1",
        )
        .bind(thread_id)
        .bind(event_type)
        .fetch_optional(pool)
        .await
        .expect("event lookup");
        if let Some(payload) = row {
            return payload;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no {event_type} landed on thread {thread_id}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn stop_with_pending_question_and_no_live_agent_emits_canceled_not_stale_settle() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    let tool_use_id = format!(
        "tu-stopparked-{}",
        &Uuid::new_v4().as_simple().to_string()[..8]
    );
    insert_user_question_asked(&pool, thread_id, &tool_use_id).await;

    let resp = client
        .post(format!(
            "{}/api/v1/claude-code/stop?thread_id={}",
            base_url(),
            thread_id
        ))
        .send()
        .await
        .expect("stop request failed");
    assert_eq!(resp.status().as_u16(), 200, "stop should return 200");
    let body: serde_json::Value = resp.json().await.expect("stop response must be JSON");
    assert_eq!(
        body["canceled"], true,
        "resolving the card is a status-changing effect, so the client keeps its \
         optimistic canceling state"
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
        "Stop must resolve the pending question with AnswerKind::Canceled"
    );

    let payload = await_event_payload(&pool, thread_id, "ResponseCanceled").await;
    assert_eq!(
        payload["cause"], "user_stop",
        "the terminal must read as the user's cancel, matching the live-agent path"
    );

    let aborted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'ResponseAborted'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap_or(-1);
    assert_eq!(
        aborted, 0,
        "no stale-settle abort: the cancel's own write is what made the row running"
    );
}

#[tokio::test]
async fn stop_on_a_parked_question_withholds_the_branch_work() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let branch = format!("e2e-test/stopped-plan-{suffix}");
    commit_on_new_branch(&branch, &format!("e2e-plan-{suffix}.md"), "a plan");
    let thread_id = Uuid::new_v4();
    let tool_use_id = format!("tu-stopplan-{suffix}");
    insert_session_started(&pool, thread_id, &branch).await;
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2::text, 'ResponseGenerated', $3, NOW(), $2)",
    )
    .bind(Uuid::new_v4())
    .bind(thread_id)
    .bind(serde_json::json!({ "text": "Done.", "channel": "claude_code" }))
    .execute(&pool)
    .await
    .expect("insert an earlier clean turn");
    insert_user_question_asked(&pool, thread_id, &tool_use_id).await;

    let resp = client
        .post(format!(
            "{}/api/v1/claude-code/stop?thread_id={}",
            base_url(),
            thread_id
        ))
        .send()
        .await
        .expect("stop request failed");
    assert_eq!(resp.status().as_u16(), 200, "stop should return 200");

    let withheld = await_event_payload(&pool, thread_id, "ProposalWithheld").await;
    assert_eq!(withheld["branch_name"], branch.as_str());
    assert_eq!(withheld["reason"], "turn_incomplete");
    assert_eq!(
        withheld["files"],
        serde_json::json!([format!("e2e-plan-{suffix}.md")])
    );
    let cancel = await_event_payload(&pool, thread_id, "ResponseCanceled").await;
    assert_eq!(
        cancel["channel"], "claude_code",
        "the Stop reads as the newest coding-agent terminal"
    );
    let changes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM changes WHERE branch_name = $1")
        .bind(&branch)
        .fetch_one(&pool)
        .await
        .expect("changes lookup");
    assert_eq!(changes, 0, "an unfinished turn proposes nothing");
    let (state, reason): (String, Option<String>) = sqlx::query_as(
        "SELECT coding_agent_change_state, coding_agent_unproposed_reason \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("thread summary");
    assert_eq!(
        (state.as_str(), reason.as_deref()),
        ("unproposed", Some("turn_incomplete"))
    );

    let resp = post_archive(thread_id).await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "withheld work no longer blocks Archive"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let set_aside = loop {
        let row: Option<String> =
            sqlx::query_scalar("SELECT status FROM changes WHERE branch_name = $1")
                .bind(&branch)
                .fetch_optional(&pool)
                .await
                .expect("changes lookup");
        if row.is_some() || std::time::Instant::now() >= deadline {
            break row;
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    };
    assert_eq!(
        set_aside.as_deref(),
        Some("set_aside"),
        "the archive net keeps the stopped work"
    );

    let _ = git(&["branch", "-D", &branch]);
    let _ = sqlx::query("DELETE FROM changes WHERE branch_name = $1")
        .bind(&branch)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await;
}
