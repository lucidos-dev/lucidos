use crate::support::{
    base_url, db_url, http_client, poll_thread_summary_by_marker, unique_marker, user_client,
};
use uuid::Uuid;

#[tokio::test]
async fn chat_stream_returns_event_id() {
    let client = user_client().await;
    let url = format!("{}/api/v1/chat/stream", base_url());
    let marker = unique_marker("api-chat");

    let body = serde_json::json!({
        "message": format!("Say exactly: \"hello {marker}\""),
        "mode": "human",
    });

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("Chat stream request failed");

    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());

    let result: serde_json::Value = resp.json().await.expect("Invalid JSON response");
    assert!(
        result["event_id"].is_string(),
        "Response should contain event_id"
    );
    let event_id = result["event_id"].as_str().unwrap();
    assert!(!event_id.is_empty(), "event_id should not be empty");
}

#[tokio::test]
async fn chat_stream_with_thread_id() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let url = format!("{}/api/v1/chat/stream", base_url());
    let marker = unique_marker("api-thread");

    // First message creates a thread
    let body1 = serde_json::json!({
        "message": format!("Say exactly: \"first {marker}\""),
        "mode": "human",
    });
    let resp1: serde_json::Value = client
        .post(&url)
        .json(&body1)
        .send()
        .await
        .expect("First chat request failed")
        .json()
        .await
        .expect("Invalid JSON");
    assert!(
        resp1["event_id"].is_string(),
        "First message should return event_id"
    );

    // Resolve the thread by marker — `archive[0]` would race with parallel
    // tests whose threads can sort ahead and yield a 409 when their (mode,
    // repo) doesn't match this follow-up.
    let row = poll_thread_summary_by_marker(&pool, &marker, 15).await;
    let thread_id = row.thread_id.to_string();

    // Send a follow-up to the same thread
    let body2 = serde_json::json!({
        "message": format!("Say exactly: \"second {marker}\""),
        "mode": "human",
        "thread_id": thread_id,
    });
    let resp2 = client
        .post(&url)
        .json(&body2)
        .send()
        .await
        .expect("Follow-up chat request failed");

    assert_eq!(resp2.status(), 200);
    let result2: serde_json::Value = resp2.json().await.expect("Invalid JSON");
    assert!(result2["event_id"].is_string());
}

#[tokio::test]
async fn chat_empty_message_is_rejected() {
    let client = user_client().await;
    let url = format!("{}/api/v1/chat/stream", base_url());

    let body = serde_json::json!({
        "message": "",
        "mode": "human",
    });

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("Request failed");

    // Empty message should be rejected (400) or handled gracefully (not 500)
    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 200,
        "Empty message should return 400 or 200, got {status}"
    );
}

/// Refactor regression: POST /api/v1/chat/stream used to hardcode parent_thread_id=NULL
/// and initiator='user', causing CC-spawned child threads to be mislabeled as
/// user-initiated roots. Verify the wire fields actually reach thread_summaries
/// when the caller explicitly identifies as system.
#[tokio::test]
async fn parent_thread_id_propagates_to_projection() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let url = format!("{}/api/v1/chat/stream", base_url());
    let parent_uuid = Uuid::new_v4();
    let spawning_event = Uuid::new_v4();
    let marker = unique_marker("api-parent-prop");

    let body = serde_json::json!({
        "message": format!("noop test message {marker}"),
        "mode": "agent",
        "parent_thread_id": parent_uuid.to_string(),
        "spawning_event_id": spawning_event.to_string(),
    });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("POST failed");
    assert_eq!(resp.status(), 200);

    let row = poll_thread_summary_by_marker(&pool, &marker, 15).await;
    assert_eq!(
        row.parent_thread_id,
        Some(parent_uuid),
        "parent_thread_id from request must reach projection"
    );
    assert_eq!(
        row.initiator, "system",
        "mode=agent must mark the thread system-initiated"
    );
}

/// mode=human keeps threads user-initiated (regression check for the default path).
#[tokio::test]
async fn human_mode_means_user_initiated() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let url = format!("{}/api/v1/chat/stream", base_url());
    let marker = unique_marker("api-no-parent");

    let body = serde_json::json!({
        "message": format!("noop test message {marker}"),
        "mode": "human",
    });
    client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("POST failed");

    let row = poll_thread_summary_by_marker(&pool, &marker, 15).await;
    assert_eq!(
        row.parent_thread_id, None,
        "human-mode requests have NULL parent_thread_id in projection"
    );
    assert_eq!(row.initiator, "user", "mode=human produces initiator=user");
}

#[tokio::test]
async fn invalid_parent_thread_id_returns_400() {
    let client = user_client().await;
    let url = format!("{}/api/v1/chat/stream", base_url());
    let body = serde_json::json!({
        "message": "test",
        "mode": "agent",
        "parent_thread_id": "not-a-valid-uuid",
    });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("Request failed");
    assert_eq!(
        resp.status(),
        400,
        "invalid parent_thread_id must return 400, not silently fall back"
    );
}

/// `repo_id` accepts a UUID or a registered repo name (the
/// `lucidos spawn-thread --repo <name>` CLI sends names). An unknown repo
/// must surface a clean 400 at the API boundary, not a 500 from deep inside
/// the engine when worktree creation fails. This is the chat-side mirror of
/// the existing repo-switch lock check.
#[tokio::test]
async fn chat_stream_rejects_unknown_repo() {
    let client = user_client().await;
    let url = format!("{}/api/v1/chat/stream", base_url());
    let body = serde_json::json!({
        "message": "test",
        "mode": "human",
        "use_coding_agent": true,
        "repo_id": "definitely-not-a-real-repo-name-for-e2e",
    });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("Request failed");
    assert_eq!(
        resp.status(),
        400,
        "unknown repo must return 400, not 500 or silent fallback"
    );
}

/// `mode` is mandatory — requests omitting it must be rejected (not silently
/// defaulted to human) so the spawn-thread skill and other callers cannot
/// accidentally produce mislabeled threads.
#[tokio::test]
async fn missing_mode_returns_400() {
    let client = user_client().await;
    let url = format!("{}/api/v1/chat/stream", base_url());
    let body = serde_json::json!({
        "message": "test",
    });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("Request failed");
    let status = resp.status().as_u16();
    assert!(
        (400..500).contains(&status),
        "missing mode must be rejected as a bad request, got {status}",
    );
}

/// A thread is locked to its (mode, repo) at first message — switching
/// either mid-thread caused multi-repo session mixing where the executor
/// card and the commands menu disagreed (e.g. menu showed Lucidos skills
/// while the session ran on a different repo). The chat handler must reject
/// follow-ups that try to flip mode or repo.
///
/// `state='active'` here is the operative bit: the lock only kicks in once
/// the thread has actually been sent. Drafts (`state='composing'`) intentionally
/// permit mode toggling — see `chat_stream_allows_mode_switch_on_composing_thread`.
#[tokio::test]
async fn chat_stream_rejects_mode_switch_on_existing_thread() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect e2e db");

    let thread_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, source, state) \
         VALUES ($1, 'chat', 'active')",
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("seed chat thread");

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({
            "message": "switch me to CC",
            "mode": "human",
            "thread_id": thread_id.to_string(),
            "use_coding_agent": true,
            "repo_id": Uuid::new_v4().to_string(),
        }))
        .send()
        .await
        .expect("send follow-up");

    assert_eq!(
        resp.status(),
        409,
        "chat\u{2192}CC switch on existing thread must 409, got {}",
        resp.status(),
    );

    // The rejection must SAY why. A bare status sends an empty body, and the
    // frontend falls back to `statusText`, which is "" over HTTP/2, so the
    // user's toast read "Failed to send message: 409" and named neither the
    // thread's mode nor the one that was requested.
    let body: serde_json::Value = resp.json().await.expect("409 body must be JSON");
    let message = body["error"].as_str().unwrap_or_default();
    assert!(
        message.contains("locked") && message.contains("coding-agent"),
        "409 body must name the lock and the modes, got {body}",
    );

    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .ok();
}

/// Regression: a draft can have its `source` column set to `'claude_code'`
/// from a prior compose-mode toggle (the PUT compose CASE writes it). When
/// the user toggles back to Lucidos and clicks Send before the debounced PUT
/// lands, the chat request arrives with `use_coding_agent=false` and the
/// row's stale source surfaces as a 409 ("Thread is locked to coding-agent
/// mode; cannot switch to Lucidos Agent") — even though no message has ever
/// been sent on the thread.
///
/// The mode lock must only apply to threads that have actually been sent
/// (`state='active'`). Composing threads are still drafts; the user is
/// allowed to change their mind right up to the moment of send.
#[tokio::test]
async fn chat_stream_allows_mode_switch_on_composing_thread() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect e2e db");

    let thread_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, source, state, compose_mode) \
         VALUES ($1, 'claude_code', 'composing', 'claude_code')",
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("seed composing thread with stale CC source");

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({
            "message": "I changed my mind, send via Lucidos",
            "mode": "human",
            "thread_id": thread_id.to_string(),
            // No use_coding_agent → Lucidos, despite source='claude_code'
        }))
        .send()
        .await
        .expect("send composing thread");

    assert_eq!(
        resp.status(),
        200,
        "composing thread must accept mode switch, got {}",
        resp.status(),
    );

    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .ok();
}

/// Companion to the mode-switch test: a CC thread bound to repo A must
/// reject a follow-up requesting repo B (the menu would otherwise collapse
/// to whichever repo's `changes` row is most recent and disagree with the
/// executor card).
#[tokio::test]
async fn chat_stream_rejects_repo_switch_on_existing_cc_thread() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect e2e db");

    let repo_a = Uuid::new_v4().to_string();
    let repo_b = Uuid::new_v4().to_string();
    let thread_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, source, cc_repo_id, state) \
         VALUES ($1, 'claude_code', $2, 'active')",
    )
    .bind(thread_id)
    .bind(&repo_a)
    .execute(&pool)
    .await
    .expect("seed CC thread on repo A");

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({
            "message": "switch repo on me",
            "mode": "human",
            "thread_id": thread_id.to_string(),
            "use_coding_agent": true,
            "repo_id": repo_b,
        }))
        .send()
        .await
        .expect("send follow-up");

    assert_eq!(
        resp.status(),
        409,
        "CC repo switch on existing thread must 409, got {}",
        resp.status(),
    );

    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .ok();
}

/// Regression: mobile screenshots in base64 routinely exceed axum's 2 MiB
/// default body limit, surfacing as "Failed to send message: Failed to buffer
/// the request body" toast. /api/v1/chat/stream must accept large image payloads.
#[tokio::test]
async fn chat_stream_accepts_large_image_payload() {
    let client = user_client().await;
    let url = format!("{}/api/v1/chat/stream", base_url());

    // 5 MiB of base64 — well above axum's 2 MiB default, well below our 100 MiB cap.
    let large_base64 = "A".repeat(5 * 1024 * 1024);
    let body = serde_json::json!({
        "message": "ignore — body limit regression test",
        "mode": "human",
        "images": [{
            "base64": large_base64,
            "mime_type": "image/png",
        }],
    });

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("Request failed");
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert_eq!(
        status, 200,
        "chat_stream must accept >2 MiB body (mobile screenshots). Got {status}: {text}",
    );
}

/// A `200` from `POST /chat/stream` must mean the follow-up is ALREADY in the
/// thread, with its sequence assigned, and two sends issued one after the other
/// must land in that order.
///
/// This is the engine half of the fix for messages arriving reversed
/// (`docs/plans/2026-07-30-serialize-chat-sends-per-thread.md`). The handler
/// used to spawn the turn and ack immediately, with `MessageReceived` emitted
/// inside the spawned task behind a Thread Queue slot wait, so a client that
/// waited for the ack still had no ordering guarantee: the second request's
/// task could take the lower sequence. `created` is stamped at emit time and
/// the request carries no client-side ordering data, so such a reversal is
/// unrecoverable after the fact.
#[tokio::test]
async fn follow_up_is_persisted_before_the_ack_and_keeps_send_order() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let url = format!("{}/api/v1/chat/stream", base_url());
    let marker = unique_marker("api-send-order");

    // First message creates the thread. It is deliberately NOT pre-emitted (a
    // brand-new thread has nothing to be out of order with), so resolve the
    // thread the same way the other tests do.
    let resp: serde_json::Value = client
        .post(&url)
        .json(&serde_json::json!({
            "message": format!("Reply with one word. {marker}"),
            "mode": "human",
        }))
        .send()
        .await
        .expect("First chat request failed")
        .json()
        .await
        .expect("Invalid JSON");
    assert!(resp["event_id"].is_string());
    let thread_id = poll_thread_summary_by_marker(&pool, &marker, 15)
        .await
        .thread_id;

    // Two follow-ups, issued strictly one after the other, exactly as the
    // client's per-thread send chain does it.
    let mut sequences = Vec::new();
    for (nth, text) in ["first follow-up", "second follow-up"].iter().enumerate() {
        let body = serde_json::json!({
            "message": format!("{text} {marker}"),
            "mode": "human",
            "thread_id": thread_id.to_string(),
        });
        let status = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .expect("Follow-up chat request failed")
            .status();
        assert_eq!(status, 200, "follow-up {nth} should be accepted");

        // No polling: the ack is the claim under test, so read the event once,
        // immediately. A miss here means the handler acked before persisting.
        let seq: Option<i64> = sqlx::query_scalar(
            "SELECT sequence FROM events \
             WHERE thread_id = $1 AND event_type = 'MessageReceived' \
             AND payload->>'text' = $2",
        )
        .bind(thread_id)
        .bind(format!("{text} {marker}"))
        .fetch_optional(&pool)
        .await
        .expect("events query failed");
        sequences.push(seq.unwrap_or_else(|| {
            panic!("follow-up {nth} ({text}) was not persisted by the time the POST returned 200")
        }));
    }

    assert!(
        sequences[0] < sequences[1],
        "follow-ups must keep send order, got sequences {sequences:?}",
    );
}

// ── Attribution and targeting refusals ──────────────────────────────
//
// Regression coverage for the 2026-08-06 incident. The Lucidos Agent was asked
// to message every running coding-agent thread, no tool covered that, and
// instead of reporting itself blocked it shelled out to `curl` and POSTed
// straight to this endpoint. Two engine defects let that through and a third
// hid it: the turn was recorded as `mode: human`, the POST reached the wrong
// engine, and that engine materialized six threads from the unknown ids so
// reading them back CONFIRMED the mistake.

/// An unattributed caller may not record a turn as the user.
///
/// `http_client()` (not `user_client()`) is the point: no device id, no origin
/// token, exactly the shape `curl` produces. The refusal must also leave the
/// workspace untouched, which is the half a status-code-only assertion misses.
#[tokio::test]
async fn chat_stream_refuses_unattributed_human_mode() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let thread_id = Uuid::new_v4();
    let marker = unique_marker("api-impersonation");

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({
            "message": format!("posted as the user {marker}"),
            "mode": "human",
            "thread_id": thread_id.to_string(),
            "new_thread": true,
        }))
        .send()
        .await
        .expect("Chat stream request failed");

    assert_eq!(
        resp.status(),
        403,
        "an unattributed mode=human POST must be refused"
    );
    let body: serde_json::Value = resp.json().await.expect("refusal must carry a JSON body");
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("registered device"),
        "the refusal must say what evidence is missing: {error}"
    );
    assert!(
        error.contains("tell the user it is not possible"),
        "the refusal must tell an agent what to do instead: {error}"
    );

    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .expect("events query failed");
    assert_eq!(events, 0, "a refused POST must persist no event");
    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("thread_summaries query failed");
    assert_eq!(rows, 0, "a refused POST must create no thread");
}

/// An empty `device_id` in the body must not shadow a valid
/// `x-lucidos-device-id` header. `Option::or` keeps a `Some("")`, so filtering
/// blanks after the fallback instead of before it would refuse a request that
/// is genuinely device-attributed.
#[tokio::test]
async fn an_empty_body_device_id_falls_back_to_the_header() {
    // `user_client` carries the header for a registered device; the body field
    // is deliberately blank on top of it.
    let client = user_client().await;
    let marker = unique_marker("api-blank-device-id");

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({
            "message": format!("Say exactly: \"hi {marker}\""),
            "mode": "human",
            "device_id": "",
        }))
        .send()
        .await
        .expect("Chat stream request failed");

    assert_eq!(
        resp.status(),
        200,
        "a blank body device_id must fall through to the header, not refuse"
    );
}

// The escape hatch is proven by every other test in this file: they all post
// `mode: "human"` through `user_client()`, which registers a device, and they
// all expect 200. A dedicated "attributed request is accepted" test would only
// spawn one more agentic turn against the shared capacity pool to assert what
// `chat_stream_returns_event_id` already asserts.

/// An unknown `thread_id` with no create signal must 404 and create nothing.
///
/// This is the behaviour whose absence made the incident self-confirming: the
/// `MessageReceived` projection is an upsert, so an id naming nothing used to
/// materialize the thread, and the agent's own read-back then found its message
/// on the wrong engine and reported success.
#[tokio::test]
async fn chat_stream_does_not_create_a_thread_from_an_unknown_id() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let thread_id = Uuid::new_v4();
    let marker = unique_marker("api-phantom");

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({
            "message": format!("follow-up to a thread that does not exist {marker}"),
            "mode": "human",
            "thread_id": thread_id.to_string(),
        }))
        .send()
        .await
        .expect("Chat stream request failed");

    assert_eq!(
        resp.status(),
        404,
        "an unknown thread id must not be a create"
    );
    let body: serde_json::Value = resp.json().await.expect("refusal must carry a JSON body");
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("new_thread"),
        "the refusal must name the create signal: {error}"
    );

    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("thread_summaries query failed");
    assert_eq!(rows, 0, "the phantom thread must not exist");
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .expect("events query failed");
    assert_eq!(events, 0, "and it must carry no seeded message");
}

/// `new_thread: true` is what turns the same POST into a legitimate create.
#[tokio::test]
async fn chat_stream_creates_a_thread_when_the_request_says_it_is_creating_one() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let thread_id = Uuid::new_v4();
    let marker = unique_marker("api-declared-create");

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({
            "message": format!("Say exactly: \"created {marker}\""),
            "mode": "human",
            "thread_id": thread_id.to_string(),
            "new_thread": true,
        }))
        .send()
        .await
        .expect("Chat stream request failed");
    assert_eq!(resp.status(), 200);

    let row = poll_thread_summary_by_marker(&pool, &marker, 30).await;
    assert_eq!(
        row.thread_id, thread_id,
        "a declared create must produce the thread under the id the caller supplied"
    );
}

/// Ask the engine which workspace it serves. The same disclosure a mis-aimed
/// caller uses to re-resolve its target, which is why the 409 body points here.
async fn engine_workspace_name() -> String {
    let health: serde_json::Value = http_client()
        .get(format!("{}/api/v1/health", base_url()))
        .send()
        .await
        .expect("health request failed")
        .json()
        .await
        .expect("health must be JSON");
    health["workspace"]
        .as_str()
        .expect("health must name its workspace")
        .to_string()
}

/// A request that names a different workspace must be refused by this engine,
/// with the actual workspace named so the caller can re-resolve its target.
/// Several engines run on one machine; without this a wrong port is served in
/// full by whichever one is listening.
#[tokio::test]
async fn chat_stream_refuses_a_request_aimed_at_another_workspace() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let thread_id = Uuid::new_v4();
    let marker = unique_marker("api-wrong-workspace");

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .header("x-lucidos-target-workspace", "some-other-workspace")
        .json(&serde_json::json!({
            "message": format!("meant for somewhere else {marker}"),
            "mode": "human",
            "thread_id": thread_id.to_string(),
            "new_thread": true,
        }))
        .send()
        .await
        .expect("Chat stream request failed");

    assert_eq!(
        resp.status(),
        409,
        "a request asserted for another workspace must not be executed here"
    );
    let body: serde_json::Value = resp.json().await.expect("refusal must carry a JSON body");
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("some-other-workspace"),
        "the refusal must echo what was asserted: {error}"
    );
    let actual = engine_workspace_name().await;
    assert!(
        error.contains(&actual),
        "and must name the workspace this engine actually serves ({actual}): {error}"
    );

    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("thread_summaries query failed");
    assert_eq!(rows, 0, "a mis-aimed request must write nothing");
}

/// The assertion is honored, not merely rejected: naming THIS workspace passes
/// through. Asserted against `/health` rather than a chat POST because the
/// check is router-wide middleware, so any endpoint proves it, and a chat POST
/// would spend a real agentic turn out of the shared capacity pool to prove
/// something about a header. Absent-changes-nothing is covered by every other
/// test in this suite, none of which sends an assertion at all.
#[tokio::test]
async fn a_request_aimed_at_this_workspace_passes_through() {
    let actual = engine_workspace_name().await;

    let resp = http_client()
        .get(format!("{}/api/v1/health", base_url()))
        .header("x-lucidos-target-workspace", &actual)
        .send()
        .await
        .expect("health request failed");

    assert_eq!(
        resp.status(),
        200,
        "an assertion naming this engine's own workspace must not block anything"
    );
}
