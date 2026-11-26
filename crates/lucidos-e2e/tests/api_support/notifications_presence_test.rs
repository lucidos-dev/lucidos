use crate::support::{base_url, db_url, http_client, register_device, unique_marker, user_client};
use futures::StreamExt;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::io::StreamReader;

static FAN_OUT_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn fan_out_test_lock() -> &'static Mutex<()> {
    FAN_OUT_TEST_LOCK.get_or_init(|| Mutex::new(()))
}

async fn reset_presence_state() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
    sqlx::query("DELETE FROM device_presence")
        .execute(&pool)
        .await
        .expect("clear device_presence");
    sqlx::query("DELETE FROM push_log")
        .execute(&pool)
        .await
        .expect("clear push_log");
    pool.close().await;
}

async fn register_test_subscription(suffix: &str) -> String {
    let device_id = format!("presence-test-{}", suffix);

    register_device(&device_id).await;

    let resp = http_client()
        .put(format!("{}/api/v1/devices/{}/push", base_url(), device_id))
        .header("x-lucidos-device-id", &device_id)
        .json(&serde_json::json!({ "push_enabled": true }))
        .send()
        .await
        .expect("set-push request");
    assert_eq!(resp.status(), 200, "set_device_push should succeed");

    let endpoint = format!("https://test.invalid/{}", device_id);
    let resp = http_client()
        .post(format!("{}/api/v1/push/subscribe", base_url()))
        .header("x-lucidos-device-id", &device_id)
        .json(&serde_json::json!({
            "endpoint": endpoint,
            "p256dh": "BAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "auth": "AAAAAAAAAAAAAAAAAAAAAA",
            "device_id": device_id,
        }))
        .send()
        .await
        .expect("subscribe request");
    assert_eq!(resp.status(), 200, "subscribe should succeed");
    device_id
}

async fn create_notification(title: &str, message: &str) -> String {
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/notifications", base_url()))
        .json(&serde_json::json!({
            "title": title,
            "message": message,
        }))
        .send()
        .await
        .expect("create_notification request");
    assert_eq!(resp.status(), 200, "create_notification should succeed");
    let body: serde_json::Value = resp.json().await.expect("parse json");
    body["notification_id"]
        .as_str()
        .expect("notification_id in response")
        .to_string()
}

#[tokio::test]
async fn s3_presence_pong_endpoint_accepts_valid_payload() {
    let resp = http_client()
        .post(format!("{}/api/v1/presence-pong", base_url()))
        .json(&serde_json::json!({
            "notification_id": uuid::Uuid::new_v4(),
            "device_id": "presence-pong-test",
            "outcome": "shown",
        }))
        .send()
        .await
        .expect("presence-pong request");
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn s3_notification_with_no_candidates_sends_push_immediately() {
    let _guard = fan_out_test_lock().lock().await;

    reset_presence_state().await;
    let suffix = unique_marker("no-candidates");
    let device_id = register_test_subscription(&suffix).await;

    let notification_id =
        create_notification(&format!("Test no-candidates {}", suffix), "body").await;

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    loop {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM push_log WHERE notification_id = $1::uuid AND device_id = $2",
        )
        .bind(&notification_id)
        .bind(&device_id)
        .fetch_one(&pool)
        .await
        .expect("count push_log");
        if count > 0 {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "push_log never received a row for notification={} device={}",
                notification_id, device_id
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    pool.close().await;
}

#[tokio::test]
async fn s3_notification_with_connected_sse_but_no_candidate_runs_presence_check() {
    let _guard = fan_out_test_lock().lock().await;

    reset_presence_state().await;
    let suffix = unique_marker("sse-no-candidate");

    let sse_handle: tokio::task::JoinHandle<Vec<String>> = tokio::spawn(async move {
        let resp = http_client()
            .get(format!("{}/api/v1/events", base_url()))
            .header("Accept", "text/event-stream")
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .expect("SSE connect");
        let byte_stream = resp
            .bytes_stream()
            .map(|r| r.map_err(std::io::Error::other));
        let reader = StreamReader::new(byte_stream);
        let mut lines = BufReader::new(reader).lines();

        let mut collected = Vec::new();
        let deadline = tokio::time::sleep(Duration::from_secs(5));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => collected.push(line),
                    Ok(None) | Err(_) => break,
                },
                _ = &mut deadline => break,
            }
        }
        collected
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let _device_id = register_test_subscription(&suffix).await;

    let notification_id = create_notification(
        &format!("Test sse-no-candidate {}", suffix),
        "connected page, stale presence",
    )
    .await;

    let lines = sse_handle.await.expect("SSE task panicked");

    lines
        .iter()
        .find(|l| l.contains("\"type\":\"PresenceCheck\"") && l.contains(&notification_id))
        .unwrap_or_else(|| {
            panic!(
                "Expected a PresenceCheck SSE event carrying notification_id={} from a connected \
                 page with NO device_presence candidate; got {} lines, last 5: {:?}",
                notification_id,
                lines.len(),
                lines.iter().rev().take(5).collect::<Vec<_>>(),
            )
        });
}

#[tokio::test]
async fn s3_notification_with_visible_device_emits_presence_check_sse() {
    let _guard = fan_out_test_lock().lock().await;

    reset_presence_state().await;
    let suffix = unique_marker("emits-check");

    let sse_handle: tokio::task::JoinHandle<Vec<String>> = tokio::spawn(async move {
        let resp = http_client()
            .get(format!("{}/api/v1/events", base_url()))
            .header("Accept", "text/event-stream")
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .expect("SSE connect");
        let byte_stream = resp
            .bytes_stream()
            .map(|r| r.map_err(std::io::Error::other));
        let reader = StreamReader::new(byte_stream);
        let mut lines = BufReader::new(reader).lines();

        let mut collected = Vec::new();
        let deadline = tokio::time::sleep(Duration::from_secs(5));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => collected.push(line),
                    Ok(None) | Err(_) => break,
                },
                _ = &mut deadline => break,
            }
        }
        collected
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let device_id = format!("presence-visible-{}", suffix);
    register_device(&device_id).await;
    let resp = http_client()
        .post(format!("{}/api/v1/device-presence", base_url()))
        .header("x-lucidos-device-id", &device_id)
        .json(&serde_json::json!({ "device_id": device_id, "visible": true }))
        .send()
        .await
        .expect("device-presence post");
    assert_eq!(resp.status(), 200);

    let _device_id = register_test_subscription(&suffix).await;

    let notification_id = create_notification(
        &format!("Test emits-check {}", suffix),
        "trigger PresenceCheck",
    )
    .await;

    let lines = sse_handle.await.expect("SSE task panicked");

    let presence_check_line = lines
        .iter()
        .find(|l| {
            l.contains("\"type\":\"PresenceCheck\"") && l.contains(&notification_id)
        })
        .unwrap_or_else(|| {
            panic!(
                "Expected a PresenceCheck SSE event carrying notification_id={}; got {} lines, last 5: {:?}",
                notification_id,
                lines.len(),
                lines.iter().rev().take(5).collect::<Vec<_>>(),
            )
        });
    let expected_deadline = format!(
        "\"deadline_ms\":{}",
        lucidos_engine::scheduler::push::DEADLINE_MS
    );
    assert!(
        presence_check_line.contains(&expected_deadline),
        "PresenceCheck must include {} per spec; line was: {}",
        expected_deadline,
        presence_check_line,
    );
}

async fn answer_presence_check(outcome: &str) -> (String, String, String) {
    reset_presence_state().await;
    let suffix = unique_marker(&format!("ack-{outcome}"));
    let device_id = register_test_subscription(&suffix).await;

    let resp = http_client()
        .post(format!("{}/api/v1/device-presence", base_url()))
        .header("x-lucidos-device-id", &device_id)
        .json(&serde_json::json!({ "device_id": device_id, "visible": true }))
        .send()
        .await
        .expect("device-presence post");
    assert_eq!(resp.status(), 200);

    let sse_device_id = device_id.clone();
    let reader_suffix = suffix.clone();
    let pong_outcome = outcome.to_string();
    let sse_handle: tokio::task::JoinHandle<Option<String>> = tokio::spawn(async move {
        let resp = http_client()
            .get(format!("{}/api/v1/events", base_url()))
            .header("Accept", "text/event-stream")
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .expect("SSE connect");
        let byte_stream = resp
            .bytes_stream()
            .map(|r| r.map_err(std::io::Error::other));
        let reader = StreamReader::new(byte_stream);
        let mut lines = BufReader::new(reader).lines();

        let deadline = tokio::time::sleep(Duration::from_secs(8));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        if line.contains("\"type\":\"PresenceCheck\"") && line.contains(&reader_suffix) {
                            let nid = extract_notification_id(&line).expect("notification_id on the check");
                            let resp = http_client()
                                .post(format!("{}/api/v1/presence-pong", base_url()))
                                .json(&serde_json::json!({
                                    "notification_id": nid,
                                    "device_id": sse_device_id,
                                    "outcome": pong_outcome,
                                }))
                                .send()
                                .await
                                .expect("presence-pong post");
                            assert_eq!(resp.status(), 200);
                            return Some(line);
                        }
                    }
                    Ok(None) | Err(_) => return None,
                },
                _ = &mut deadline => return None,
            }
        }
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let notification_id =
        create_notification(&format!("Test ack {}", suffix), "the page acts first").await;

    let check_line = sse_handle
        .await
        .expect("SSE task panicked")
        .unwrap_or_else(|| panic!("no PresenceCheck reached the page for {notification_id}"));
    (device_id, notification_id, check_line)
}

async fn push_log_rows(notification_id: &str, device_id: &str) -> i64 {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM push_log WHERE notification_id = $1::uuid AND device_id = $2",
    )
    .bind(notification_id)
    .bind(device_id)
    .fetch_one(&pool)
    .await
    .expect("count push_log");
    pool.close().await;
    count
}

#[tokio::test]
async fn s4_a_shown_ack_suppresses_the_push_and_the_check_carries_the_toast() {
    let _guard = fan_out_test_lock().lock().await;

    let (device_id, notification_id, check_line) = answer_presence_check("shown").await;

    assert!(
        check_line.contains(&notification_id) && check_line.contains("the page acts first"),
        "the PresenceCheck must carry the toast for notification={notification_id}; line was: {check_line}",
    );

    tokio::time::sleep(Duration::from_millis(700)).await;
    let push_count = push_log_rows(&notification_id, &device_id).await;
    assert_eq!(
        push_count, 0,
        "a page that showed the toast must NOT also get an OS push \
         (got {push_count} push_log rows for notification={notification_id} device={device_id})",
    );
}

#[tokio::test]
async fn s4_a_not_shown_ack_leaves_the_push_to_the_os() {
    let _guard = fan_out_test_lock().lock().await;

    let (device_id, notification_id, _) = answer_presence_check("not_shown").await;

    let mut push_count = 0;
    for _ in 0..50 {
        push_count = push_log_rows(&notification_id, &device_id).await;
        if push_count > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        push_count > 0,
        "a not_shown answer must leave the OS push to go out \
         (no push_log row for notification={notification_id} device={device_id})",
    );
}

#[tokio::test]
async fn s4_push_allowed_emits_native_push_requested_sse_with_no_web_subscription() {
    let _guard = fan_out_test_lock().lock().await;

    reset_presence_state().await;
    let suffix = unique_marker("native-push");
    let reader_suffix = suffix.clone();

    let sse_handle: tokio::task::JoinHandle<Vec<String>> = tokio::spawn(async move {
        let resp = http_client()
            .get(format!("{}/api/v1/events", base_url()))
            .header("Accept", "text/event-stream")
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .expect("SSE connect");
        let byte_stream = resp
            .bytes_stream()
            .map(|r| r.map_err(std::io::Error::other));
        let reader = StreamReader::new(byte_stream);
        let mut lines = BufReader::new(reader).lines();

        let mut collected = Vec::new();
        let deadline = tokio::time::sleep(Duration::from_secs(8));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        let is_my_native = line.contains("\"type\":\"NativePushRequested\"")
                            && line.contains(&reader_suffix);
                        collected.push(line);
                        if is_my_native { break; }
                    }
                    Ok(None) | Err(_) => break,
                },
                _ = &mut deadline => break,
            }
        }
        collected
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let notification_id = create_notification(
        &format!("Test native-push {}", suffix),
        "no web sub, connected desktop page",
    )
    .await;

    let lines = sse_handle.await.expect("SSE task panicked");

    lines
        .iter()
        .find(|l| l.contains("\"type\":\"NativePushRequested\"") && l.contains(&notification_id))
        .unwrap_or_else(|| {
            panic!(
                "Expected a NativePushRequested SSE event for {} from a connected page with NO \
                 web-push subscription; got {} lines, last 5: {:?}",
                notification_id,
                lines.len(),
                lines.iter().rev().take(5).collect::<Vec<_>>(),
            )
        });
}

async fn create_and_read_back(body: serde_json::Value) -> serde_json::Value {
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/notifications", base_url()))
        .json(&body)
        .send()
        .await
        .expect("create notification request");
    assert_eq!(resp.status(), 200, "create notification should succeed");
    let created: serde_json::Value = resp.json().await.expect("parse create response");
    let id = created["notification_id"]
        .as_str()
        .expect("notification_id in response")
        .to_string();

    let resp = http_client()
        .get(format!("{}/api/v1/notifications?limit=100", base_url()))
        .send()
        .await
        .expect("list notifications request");
    assert_eq!(resp.status(), 200, "list notifications should succeed");
    let listed: serde_json::Value = resp.json().await.expect("parse list response");
    listed["notifications"]
        .as_array()
        .expect("notifications array")
        .iter()
        .find(|n| n["id"].as_str() == Some(id.as_str()))
        .unwrap_or_else(|| panic!("notification {} missing from the list", id))
        .clone()
}

async fn seed_event(event_type: &str, thread: Option<uuid::Uuid>) -> uuid::Uuid {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e DB");
    let event = uuid::Uuid::new_v4();
    let aggregate = if thread.is_some() { "thread" } else { "domain" };
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, $2, '{}'::jsonb, NOW(), COALESCE($3::text, $2), $4, $3)",
    )
    .bind(event)
    .bind(event_type)
    .bind(thread)
    .bind(aggregate)
    .execute(&pool)
    .await
    .expect("seed event");
    event
}

#[tokio::test]
async fn a_tapless_notification_with_an_event_navigates_to_it() {
    let thread_id = uuid::Uuid::new_v4();
    let thread = thread_id.to_string();
    let event = seed_event("UserQuestionAsked", Some(thread_id))
        .await
        .to_string();
    let row = create_and_read_back(serde_json::json!({
        "title": unique_marker("derived-tap"),
        "message": "should deep-link to the event",
        "thread_id": thread,
        "event_id": event,
    }))
    .await;

    assert_eq!(row["tap"]["kind"], "navigate", "tap was {}", row["tap"]);
    assert_eq!(row["tap"]["to"]["target"], "thread");
    assert_eq!(row["tap"]["to"]["id"], thread);
    assert_eq!(row["tap"]["to"]["event_id"], event);
}

#[tokio::test]
async fn a_tapless_notification_without_an_event_opens_the_card() {
    let row = create_and_read_back(serde_json::json!({
        "title": unique_marker("thread-only-tap"),
        "message": "should open the inbox card",
        "thread_id": uuid::Uuid::new_v4().to_string(),
    }))
    .await;

    assert_eq!(row["tap"]["kind"], "modal", "tap was {}", row["tap"]);
}

#[tokio::test]
async fn an_explicit_tap_survives_the_derivation() {
    let thread = uuid::Uuid::new_v4();
    let event = seed_event("UserQuestionAsked", Some(thread)).await;
    let row = create_and_read_back(serde_json::json!({
        "title": unique_marker("explicit-tap"),
        "message": "caller asked for the card",
        "thread_id": thread.to_string(),
        "event_id": event.to_string(),
        "tap": { "kind": "modal" },
    }))
    .await;

    assert_eq!(row["tap"]["kind"], "modal", "tap was {}", row["tap"]);
}

async fn create_expecting_refusal(body: serde_json::Value) -> (u16, String) {
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/notifications", base_url()))
        .json(&body)
        .send()
        .await
        .expect("create notification request");
    let status = resp.status().as_u16();
    let reason = resp.text().await.expect("read refusal body");
    (status, reason)
}

#[tokio::test]
async fn a_domain_event_anchor_is_refused() {
    let event = seed_event("E2ETestsPassed", None).await;
    let (status, reason) = create_expecting_refusal(serde_json::json!({
        "title": unique_marker("domain-event-anchor"),
        "message": "should never be stored",
        "thread_id": uuid::Uuid::new_v4().to_string(),
        "event_id": event.to_string(),
    }))
    .await;
    assert_eq!(status, 400, "a domain event must be refused, got {reason}");
    assert!(
        reason.contains("belongs to no thread"),
        "must say why, got: {reason}"
    );
}

#[tokio::test]
async fn an_event_from_another_thread_is_refused() {
    let elsewhere = uuid::Uuid::new_v4();
    let event = seed_event("UserQuestionAsked", Some(elsewhere)).await;
    let (status, reason) = create_expecting_refusal(serde_json::json!({
        "title": unique_marker("foreign-event-anchor"),
        "message": "should never be stored",
        "thread_id": uuid::Uuid::new_v4().to_string(),
        "event_id": event.to_string(),
    }))
    .await;
    assert_eq!(status, 400, "a foreign event must be refused, got {reason}");
    assert!(
        reason.contains(&elsewhere.to_string()),
        "must name its thread, got: {reason}"
    );
}

#[tokio::test]
async fn a_tap_thread_id_that_is_not_a_uuid_is_refused() {
    for bad in ["t-9", "current", "the release thread"] {
        let (status, reason) = create_expecting_refusal(serde_json::json!({
            "title": unique_marker("bad-tap-thread-id"),
            "message": "should never be stored",
            "tap": { "kind": "navigate", "to": { "target": "thread", "id": bad } },
        }))
        .await;
        assert_eq!(status, 400, "{bad} must be refused, got {reason}");
        assert!(
            reason.contains("is not a uuid") || reason.contains("surface has none"),
            "{bad} must say why, got: {reason}"
        );
    }
}

#[tokio::test]
async fn the_current_alias_has_no_meaning_over_http() {
    let (status, reason) = create_expecting_refusal(serde_json::json!({
        "title": unique_marker("http-alias"),
        "message": "should never be stored",
        "thread_id": uuid::Uuid::new_v4().to_string(),
        "tap": { "kind": "navigate", "to": { "target": "thread", "id": "current" } },
    }))
    .await;
    assert_eq!(status, 400, "got {reason}");
    assert!(
        reason.contains("surface has none"),
        "the refusal must say why the alias cannot resolve, got: {reason}"
    );
}

#[tokio::test]
async fn a_thread_tap_with_no_id_is_refused() {
    let (status, reason) = create_expecting_refusal(serde_json::json!({
        "title": unique_marker("idless-thread-tap"),
        "message": "should never be stored",
        "tap": { "kind": "navigate", "to": { "target": "thread" } },
    }))
    .await;
    assert_eq!(status, 400, "got {reason}");
    assert!(reason.contains("has none"), "got: {reason}");
}

#[tokio::test]
async fn the_sdk_navigate_bridge_refuses_a_thread_id_that_names_nothing() {
    for bad in serde_json::json!(["current", "t-9", 7]).as_array().unwrap() {
        let resp = user_client()
            .await
            .post(format!("{}/api/v1/ui/navigate", base_url()))
            .json(&serde_json::json!({ "target": "thread", "params": { "id": bad } }))
            .send()
            .await
            .expect("ui navigate request");
        let status = resp.status().as_u16();
        let reason = resp.text().await.expect("read refusal body");
        assert_eq!(status, 400, "{bad} must be refused, got {reason}");
        assert!(
            reason.contains("is not a uuid")
                || reason.contains("surface has none")
                || reason.contains("must be a string"),
            "{bad} must say why, got: {reason}"
        );
    }
}

#[tokio::test]
async fn the_sdk_navigate_bridge_still_passes_a_real_target() {
    for params in [
        serde_json::json!({ "id": uuid::Uuid::new_v4().to_string() }),
        serde_json::json!({ "app_id": "habit-tracker" }),
    ] {
        let target = if params.get("id").is_some() {
            "thread"
        } else {
            "app"
        };
        let resp = user_client()
            .await
            .post(format!("{}/api/v1/ui/navigate", base_url()))
            .json(&serde_json::json!({ "target": target, "params": params }))
            .send()
            .await
            .expect("ui navigate request");
        assert_eq!(resp.status(), 200, "{target} must still navigate");
    }
}

#[tokio::test]
async fn the_sdk_navigate_bridge_refuses_a_repository_nobody_registered() {
    let file_path = format!("repo:{}:file:README.md", unique_marker("unregistered-repo"));
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/ui/navigate", base_url()))
        .json(&serde_json::json!({ "target": "file", "params": { "file_path": file_path } }))
        .send()
        .await
        .expect("ui navigate request");
    let status = resp.status().as_u16();
    let reason = resp.text().await.expect("read refusal body");
    assert_eq!(
        status, 400,
        "an unregistered repository must be refused, got {reason}"
    );
    assert!(
        reason.contains(&file_path),
        "the refusal names the locator: {reason}"
    );

    let resp = user_client()
        .await
        .post(format!("{}/api/v1/ui/navigate", base_url()))
        .json(&serde_json::json!({ "target": "file", "params": { "file_path": "repo:x" } }))
        .send()
        .await
        .expect("ui navigate request");
    assert_eq!(resp.status(), 200, "a malformed locator still navigates");
}

#[tokio::test]
async fn a_tap_that_names_no_thread_is_untouched() {
    let row = create_and_read_back(serde_json::json!({
        "title": unique_marker("app-tap"),
        "message": "opens the app",
        "tap": { "kind": "navigate", "to": { "target": "app", "app_id": "habit-tracker" } },
    }))
    .await;
    assert_eq!(row["tap"]["to"]["app_id"], "habit-tracker");

    let row = create_and_read_back(serde_json::json!({
        "title": unique_marker("panel-tap"),
        "message": "opens the panel",
        "tap": { "kind": "navigate", "to": { "target": "changes" } },
    }))
    .await;
    assert_eq!(row["tap"]["to"]["target"], "changes");
}

fn extract_notification_id(line: &str) -> Option<String> {
    let json_part = line.strip_prefix("data:").unwrap_or(line).trim();
    let v: serde_json::Value = serde_json::from_str(json_part).ok()?;
    v.get("data")?
        .get("notification_id")?
        .as_str()
        .map(str::to_string)
}
