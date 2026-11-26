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

    let row = poll_thread_summary_by_marker(&pool, &marker, 15).await;
    let thread_id = row.thread_id.to_string();

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

    for body in [
        serde_json::json!({ "message": "", "mode": "human" }),
        serde_json::json!({ "message": "  \n", "mode": "agent", "caller_workspace": "elsewhere" }),
    ] {
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .expect("Request failed");

        assert_eq!(resp.status().as_u16(), 400, "body: {body}");
        let err: serde_json::Value = resp.json().await.expect("standard error body");
        assert!(
            err["error"]
                .as_str()
                .is_some_and(|e| e.contains("no thread was created")),
            "says nothing was created: {err}"
        );
    }
}

#[tokio::test]
async fn chat_missing_image_refuses_text_and_image_only_sends() {
    let client = user_client().await;
    let url = format!("{}/api/v1/chat/stream", base_url());
    let missing_blob = "0".repeat(64);
    for message in ["", "Do not send without the image"] {
        let resp = client
            .post(&url)
            .json(&serde_json::json!({
                "message": message,
                "mode": "human",
                "image_hashes": [missing_blob],
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 400);
        let err: serde_json::Value = resp.json().await.unwrap();
        assert!(
            err["error"].as_str().unwrap().contains("is missing"),
            "{err}"
        );
        assert!(
            err["error"].as_str().unwrap().contains("was not sent"),
            "{err}"
        );
    }
}

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

#[tokio::test]
async fn chat_stream_rejects_an_unoffered_coding_agent_model() {
    let client = user_client().await;
    let url = format!("{}/api/v1/chat/stream", base_url());
    let body = serde_json::json!({
        "message": "test",
        "mode": "human",
        "use_coding_agent": true,
        "cc_model": "claude-sonnet-9",
    });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("Request failed");
    assert_eq!(resp.status(), 400, "an unoffered model must be refused");
    let text = resp.text().await.expect("body");
    assert!(text.contains("claude-sonnet-9"), "names the id: {text}");
    assert!(
        text.contains("Choose one of:"),
        "lists the valid ids: {text}"
    );
}

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

#[tokio::test]
async fn chat_stream_accepts_large_image_payload() {
    let client = user_client().await;
    let url = format!("{}/api/v1/chat/stream", base_url());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.resize(4 * 1024 * 1024, 0);
    let large_base64 = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(&png)
    };
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

#[tokio::test]
async fn follow_up_is_accepted_before_the_ack_and_records_in_send_order() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let url = format!("{}/api/v1/chat/stream", base_url());
    let marker = unique_marker("api-send-order");

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

    let mut event_ids = Vec::new();
    for (nth, text) in ["first follow-up", "second follow-up"].iter().enumerate() {
        let body = serde_json::json!({
            "message": format!("{text} {marker}"),
            "mode": "human",
            "thread_id": thread_id.to_string(),
        });
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .expect("Follow-up chat request failed");
        assert_eq!(resp.status(), 200, "follow-up {nth} should be accepted");
        let ack: serde_json::Value = resp.json().await.expect("Invalid JSON");
        let event_id: Uuid = ack["event_id"].as_str().unwrap().parse().unwrap();

        let accepted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM events WHERE event_type = 'OrdinarySubmissionAccepted' \
             AND payload->'data'->>'message_event_id' = $1)",
        )
        .bind(event_id.to_string())
        .fetch_one(&pool)
        .await
        .expect("events query failed");
        assert!(
            accepted,
            "follow-up {nth} ({text}) was not durably accepted by the time the POST returned 200",
        );
        event_ids.push(event_id);
    }

    let mut sequences = Vec::new();
    for event_id in event_ids {
        sequences.push(crate::input_order_test::wait_until_recorded(&pool, event_id).await);
    }

    assert!(
        sequences[0] < sequences[1],
        "follow-ups must keep send order, got sequences {sequences:?}",
    );
}

#[tokio::test]
async fn a_reposted_event_id_is_acked_once_and_runs_one_turn() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let url = format!("{}/api/v1/chat/stream", base_url());
    let marker = unique_marker("api-repost");

    let opening_id = Uuid::new_v4().to_string();
    client
        .post(&url)
        .json(&serde_json::json!({
            "message": format!("Reply with one word. {marker}"),
            "mode": "human",
            "event_id": opening_id,
        }))
        .send()
        .await
        .expect("First chat request failed");
    let thread_id = poll_thread_summary_by_marker(&pool, &marker, 15)
        .await
        .thread_id;

    let turns_ended = |request: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM events WHERE thread_id = $1 \
                 AND payload->>'request_event_id' = $2 \
                 AND event_type IN ('ResponseGenerated', 'ResponseCompleted', \
                                    'ResponseFailed', 'ResponseCanceled', 'ResponseAborted')",
            )
            .bind(thread_id)
            .bind(request)
            .fetch_one(&pool)
            .await
            .expect("events query failed")
        }
    };
    let wait_for_turn_end = |request: String| async move {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while turns_ended(request.clone()).await == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the turn for {request} never ended"
            );
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    };
    wait_for_turn_end(opening_id).await;

    let event_id = Uuid::new_v4().to_string();
    let body = serde_json::json!({
        "message": format!("Reply with one word again. {marker}"),
        "mode": "human",
        "thread_id": thread_id.to_string(),
        "event_id": event_id,
    });
    for attempt in ["first post", "re-post"] {
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .unwrap_or_else(|e| panic!("{attempt} failed: {e}"));
        assert_eq!(resp.status(), 200, "{attempt} should be acked");
        let ack: serde_json::Value = resp.json().await.expect("Invalid JSON");
        assert_eq!(
            ack["event_id"], event_id,
            "{attempt} must ack the same event id"
        );
    }

    wait_for_turn_end(event_id.clone()).await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert_eq!(
        turns_ended(event_id.clone()).await,
        1,
        "the re-post must not run a second turn"
    );

    let messages: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 \
         AND event_type = 'MessageReceived' AND id = $2::uuid",
    )
    .bind(thread_id)
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .expect("events query failed");
    assert_eq!(messages, 1, "the message must be recorded once");
}

#[tokio::test]
async fn a_refused_post_leaves_its_event_id_free_for_the_retry() {
    let client = user_client().await;
    let url = format!("{}/api/v1/chat/stream", base_url());
    let event_id = Uuid::new_v4().to_string();

    let refused = client
        .post(&url)
        .json(&serde_json::json!({
            "message": "a refused post",
            "mode": "human",
            "event_id": event_id,
            "parent_thread_id": "not-a-uuid",
        }))
        .send()
        .await
        .expect("Refused chat request failed");
    assert_eq!(refused.status(), 400);

    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let marker = unique_marker("api-refused-retry");
    let retried = client
        .post(&url)
        .json(&serde_json::json!({
            "message": format!("Reply with one word. {marker}"),
            "mode": "human",
            "event_id": event_id,
        }))
        .send()
        .await
        .expect("Retried chat request failed");
    assert_eq!(retried.status(), 200);
    poll_thread_summary_by_marker(&pool, &marker, 15).await;
}

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

#[tokio::test]
async fn an_empty_body_device_id_falls_back_to_the_header() {
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

#[tokio::test]
async fn a_caller_that_names_its_thread_keeps_that_name() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let marker = unique_marker("api-title");
    let chosen = format!("Ask-card rule {marker}");
    let thread_id = Uuid::new_v4();

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({
            "message": format!("Say exactly: \"named {marker}\""),
            "mode": "human",
            "thread_id": thread_id.to_string(),
            "new_thread": true,
            "title": chosen,
        }))
        .send()
        .await
        .expect("Chat stream request failed");
    assert_eq!(resp.status(), 200, "got {}", resp.status());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    loop {
        let done: Option<bool> =
            sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&pool)
                .await
                .expect("thread_summaries query failed");
        if done == Some(true) || std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    let stored: Option<String> =
        sqlx::query_scalar("SELECT title FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("thread_summaries query failed");
    assert_eq!(
        stored.as_deref(),
        Some(chosen.as_str()),
        "the name the caller chose must survive the turn"
    );

    let title_calls: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE thread_id = $1 AND event_type = 'ContextCaptured' \
           AND payload->>'purpose' = 'title'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("events query failed");
    assert_eq!(
        title_calls, 0,
        "a named thread must not spend a title-model call"
    );
}
