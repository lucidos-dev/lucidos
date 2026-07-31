//! Integration tests for the §2/§3 PresenceCheck fan-out flow in
//! `system-knowhow/notifications.md`. Booted against the e2e workspace
//! (engine built with `--features e2e-test-hooks` — see
//! `scripts/lib/e2e.sh`'s `ENGINE_BUILD_FEATURES`).
//!
//! Unit tests in `crates/lucidos-engine` cover the decision functions
//! (`decide_push_allowed`, `PresenceTracker`); these tests cover the
//! wiring: HTTP → EventBus emit → SSE broadcast → push_log writer.

use crate::support::{base_url, db_url, http_client, unique_marker};
use futures::StreamExt;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::io::StreamReader;

/// Both fan-out tests mutate `device_presence` and `push_log`. Cargo runs
/// integration tests in parallel; let them race on that state and the
/// "no candidates" / "expects PresenceCheck" branches flip non-deterministically.
/// Serialize them through this lock.
static FAN_OUT_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn fan_out_test_lock() -> &'static Mutex<()> {
    FAN_OUT_TEST_LOCK.get_or_init(|| Mutex::new(()))
}

/// Cleanup helper — wipe presence + push_log rows so the test starts
/// from a known state. The e2e workspace is throwaway, so destructive
/// SQL is fine here.
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

/// Register a device with push_enabled=true and attach a fake push
/// subscription so the engine has someone to push to. The endpoint URL is
/// never hit — under `e2e-test-hooks` the transport writes to push_log
/// instead. Returns the device_id.
///
/// Both prerequisites matter: `push_subscriptions.device_id` has a FK
/// to `devices(id)`, so subscribing without registering first fails the
/// insert silently (subscribe handler returns 200 with an error body via
/// `ApiResult`); and `PushSubscriptionStore::get_push_enabled` filters on
/// `devices.push_enabled = true`, so a registered-but-push-disabled
/// device gets filtered out of the fan-out.
async fn register_test_subscription(suffix: &str) -> String {
    let device_id = format!("presence-test-{}", suffix);

    let resp = http_client()
        .post(format!("{}/api/v1/devices/register", base_url()))
        .json(&serde_json::json!({ "device_id": device_id, "user_agent": "presence-e2e/1" }))
        .send()
        .await
        .expect("register request");
    assert_eq!(resp.status(), 200, "device registration should succeed");

    let resp = http_client()
        .put(format!("{}/api/v1/devices/{}/push", base_url(), device_id))
        .json(&serde_json::json!({ "push_enabled": true }))
        .send()
        .await
        .expect("set-push request");
    assert_eq!(resp.status(), 200, "set_device_push should succeed");

    let endpoint = format!("https://test.invalid/{}", device_id);
    let resp = http_client()
        .post(format!("{}/api/v1/push/subscribe", base_url()))
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
    let resp = http_client()
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
    // Spec §3 — endpoint always ACKs 200, even for a notification id the
    // engine isn't tracking (stray pongs from a closed-out PresenceCheck
    // are normal). The body shape is what the SDK
    // (presence-pong.ts) sends.
    let resp = http_client()
        .post(format!("{}/api/v1/presence-pong", base_url()))
        .json(&serde_json::json!({
            "notification_id": uuid::Uuid::new_v4(),
            "device_id": "presence-pong-test",
            "is_active": true,
            "focused_thread_id": null,
            "event_in_viewport": false,
        }))
        .send()
        .await
        .expect("presence-pong request");
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn s3_notification_with_no_candidates_sends_push_immediately() {
    let _guard = fan_out_test_lock().lock().await;

    // Spec §2 Step A — with no candidate AND no other reachable page, the
    // engine skips the PresenceCheck and decides push_allowed=true directly,
    // so a push subscription should see a push_log row. This test never opens
    // its own SSE connection, so its OWN gate input is zero — but other tests
    // in this binary run concurrently and may hold an SSE connection open,
    // which makes the engine run the PresenceCheck for THIS notification too.
    // No active pong arrives for it (those pages pong for their own checks, or
    // don't pong at all), so the decision still resolves to push — just after
    // up to DEADLINE_MS instead of instantly. The deadline below covers that
    // worst case so the assertion stays robust to test concurrency.
    reset_presence_state().await;
    let suffix = unique_marker("no-candidates");
    let device_id = register_test_subscription(&suffix).await;

    let notification_id =
        create_notification(&format!("Test no-candidates {}", suffix), "body").await;

    // Allow up to 5s (> DEADLINE_MS) for the spawned fan-out task to write to
    // push_log — see the concurrency note above.
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

    // Spec §3 — the PresenceCheck must run whenever a page is connected via
    // SSE, NOT only when a `device_presence` heartbeat row is fresh. iOS
    // suspends the 30s heartbeat while the PWA is foregrounded, so the
    // device_presence row ages out even though the EventSource is still open
    // and the page would pong `is_active`. Gating purely on heartbeat
    // freshness skipped the check → the OS push fired on top of an active
    // foreground PWA (the "push while active for a while" report). This test
    // pins the live-SSE-connection gate: with ZERO device_presence candidates
    // but an open SSE connection, NotificationCreated still broadcasts a
    // PresenceCheck.
    //
    // Pre-fix this fails: the empty-candidates branch set push_allowed=true
    // directly and never emitted a PresenceCheck.
    reset_presence_state().await;
    let suffix = unique_marker("sse-no-candidate");

    // The SSE subscriber IS the "connected page" — opening it is what makes
    // the live-connection count non-zero. It never POSTs device-presence, so
    // candidates() stays empty.
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
                    // Collect for the full window — do NOT break on the first
                    // PresenceCheck. The SSE stream is global, so the first frame
                    // may belong to another notification (a sibling fan-out test
                    // whose async PresenceCheck lands here). The assertion below
                    // selects the frame carrying THIS notification's id.
                    Ok(Some(line)) => collected.push(line),
                    Ok(None) | Err(_) => break,
                },
                _ = &mut deadline => break,
            }
        }
        collected
    });

    // Give the SSE subscriber time to register so the live-connection count
    // is non-zero before the notification fans out.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // A push subscription so the fan-out doesn't short-circuit on "nobody to
    // push to" before it ever reaches the gate. NOTE: registering a device
    // does NOT mark it visible — `device_presence` stays empty.
    let _device_id = register_test_subscription(&suffix).await;

    let notification_id = create_notification(
        &format!("Test sse-no-candidate {}", suffix),
        "connected page, stale presence",
    )
    .await;

    let lines = sse_handle.await.expect("SSE task panicked");

    // Select the PresenceCheck for THIS notification specifically — the global
    // SSE stream may also carry a sibling notification's frame.
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

    // Spec §3 — when at least one device pinged visible recently,
    // emitting NotificationCreated triggers a SystemEvent::PresenceCheck
    // broadcast carrying the notification's id. Verifies that the
    // PresenceCheck arrives on the SSE channel with the expected
    // wire shape.
    reset_presence_state().await;
    let suffix = unique_marker("emits-check");

    // Subscribe to SSE in the background so we don't miss the
    // PresenceCheck. The handle returns lines seen until the marker
    // matches or the deadline fires.
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
                    // Collect for the full window — do NOT break on the first
                    // PresenceCheck. The SSE stream is global, so the first frame
                    // may belong to another notification (a sibling fan-out test
                    // whose async PresenceCheck lands here). The assertion below
                    // selects the frame carrying THIS notification's id.
                    Ok(Some(line)) => collected.push(line),
                    Ok(None) | Err(_) => break,
                },
                _ = &mut deadline => break,
            }
        }
        collected
    });

    // Give the SSE subscriber a moment to register before producing the
    // candidate row + notification.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Mark a device visible so candidates() is non-empty → fan-out runs
    // the PresenceCheck protocol.
    let device_id = format!("presence-visible-{}", suffix);
    let resp = http_client()
        .post(format!("{}/api/v1/device-presence", base_url()))
        .json(&serde_json::json!({ "device_id": device_id, "visible": true }))
        .send()
        .await
        .expect("device-presence post");
    assert_eq!(resp.status(), 200);

    // Push subscription so the fan-out has someone to push to (the
    // presence check fires regardless, but registering the sub keeps the
    // test realistic).
    let _device_id = register_test_subscription(&suffix).await;

    let notification_id = create_notification(
        &format!("Test emits-check {}", suffix),
        "trigger PresenceCheck",
    )
    .await;

    let lines = sse_handle.await.expect("SSE task panicked");

    // Find the PresenceCheck frame for THIS notification — the global SSE
    // stream may also carry a sibling notification's frame, so match by id.
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

#[tokio::test]
async fn s4_active_pong_emits_toast_request_and_suppresses_push() {
    let _guard = fan_out_test_lock().lock().await;

    // Spec §2/§4 — the fix for "toast AND OS push at the same time". When a
    // device pongs `is_active: true`, the engine must (a) suppress the OS push
    // entirely AND (b) emit a NotificationToastRequested SSE so the active
    // page renders the in-app toast instead. The two surfaces are mutually
    // exclusive by this single decision, not by a page-side timing race.
    //
    // The SSE reader doubles as the pong sender: when it sees the
    // PresenceCheck for our notification it POSTs an active pong (which is
    // exactly when the engine's tracker slot exists), then keeps reading
    // until the NotificationToastRequested frame lands.
    reset_presence_state().await;
    let suffix = unique_marker("active-pong");
    let device_id = register_test_subscription(&suffix).await;

    // Mark the same device visible so it's the lone PresenceCheck candidate.
    let resp = http_client()
        .post(format!("{}/api/v1/device-presence", base_url()))
        .json(&serde_json::json!({ "device_id": device_id, "visible": true }))
        .send()
        .await
        .expect("device-presence post");
    assert_eq!(resp.status(), 200);

    let sse_device_id = device_id.clone();
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
        let mut ponged = false;
        let deadline = tokio::time::sleep(Duration::from_secs(8));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        // First sight of the PresenceCheck → answer it active.
                        // The tracker slot is guaranteed to exist by now (the
                        // engine emits the PresenceCheck right after expect()).
                        if !ponged && line.contains("\"type\":\"PresenceCheck\"") {
                            ponged = true;
                            if let Some(nid) = extract_notification_id(&line) {
                                let _ = http_client()
                                    .post(format!("{}/api/v1/presence-pong", base_url()))
                                    .json(&serde_json::json!({
                                        "notification_id": nid,
                                        "device_id": sse_device_id,
                                        "is_active": true,
                                        "focused_thread_id": null,
                                        "event_in_viewport": false,
                                    }))
                                    .send()
                                    .await;
                            }
                        }
                        let saw_toast = line.contains("\"type\":\"NotificationToastRequested\"");
                        collected.push(line);
                        if saw_toast { break; }
                    }
                    Ok(None) | Err(_) => break,
                },
                _ = &mut deadline => break,
            }
        }
        collected
    });

    // Give the SSE subscriber a moment to register before producing the
    // notification.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let notification_id = create_notification(
        &format!("Test active-pong {}", suffix),
        "should toast, not push",
    )
    .await;

    let lines = sse_handle.await.expect("SSE task panicked");

    // (a) The engine asked active pages to render the in-app toast.
    let toast_line = lines
        .iter()
        .find(|l| l.contains("\"type\":\"NotificationToastRequested\""))
        .unwrap_or_else(|| {
            panic!(
                "Expected a NotificationToastRequested SSE event after an active pong for {}; \
                 got {} lines, last 5: {:?}",
                notification_id,
                lines.len(),
                lines.iter().rev().take(5).collect::<Vec<_>>(),
            )
        });
    assert!(
        toast_line.contains(&notification_id),
        "NotificationToastRequested must carry notification_id={}; line was: {}",
        notification_id,
        toast_line,
    );

    // (b) The OS push was suppressed — no push_log row for this device. Give
    // any (erroneous) fan-out a generous window to have written before we
    // assert its absence.
    tokio::time::sleep(Duration::from_millis(700)).await;
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let push_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM push_log WHERE notification_id = $1::uuid AND device_id = $2",
    )
    .bind(&notification_id)
    .bind(&device_id)
    .fetch_one(&pool)
    .await
    .expect("count push_log");
    pool.close().await;
    assert_eq!(
        push_count, 0,
        "an active device must NOT receive an OS push (got {} push_log rows for notification={} device={})",
        push_count, notification_id, device_id,
    );
}

#[tokio::test]
async fn s4_push_allowed_emits_native_push_requested_sse_with_no_web_subscription() {
    let _guard = fan_out_test_lock().lock().await;

    // Spec §1/§4 — the native desktop surface. On the push-ALLOWED branch (no
    // active device pong'd in) the engine emits a NativePushRequested SSE so a
    // connected Tauri app can render a native macOS banner — it can't receive
    // the web push (WKWebView has no service-worker push).
    //
    // This test deliberately registers NO push subscription: it pins the
    // relaxed early-return. Pre-change, `send_push_to_all_with_app` bailed the
    // instant `push_subscriptions` was empty, so a desktop-only workspace got
    // neither the decision nor any SSE — the connected page's only signal was a
    // silent bell-badge bump. Now an open SSE connection alone keeps the
    // decision alive, and push-allowed broadcasts NativePushRequested.
    //
    // The SSE reader never pongs, so it counts toward `expected_pongs` but
    // reports no `is_active` → after DEADLINE_MS the engine decides
    // push_allowed=true and emits the native frame.
    reset_presence_state().await;
    let suffix = unique_marker("native-push");
    // The reader breaks only on OUR notification's frame, matched by the unique
    // suffix carried in its title. A sibling fan-out test emits its own
    // NativePushRequested ~DEADLINE_MS after it releases the shared lock — i.e.
    // after this test has already started — so a stale frame for a different
    // notification can land on this stream first; matching the suffix ignores it.
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

    // Let the SSE subscriber register so the live-connection gate is non-zero.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let notification_id = create_notification(
        &format!("Test native-push {}", suffix),
        "no web sub, connected desktop page",
    )
    .await;

    let lines = sse_handle.await.expect("SSE task panicked");

    // Match OUR notification's native frame specifically — a stale sibling frame
    // may also sit in `collected` (see reader_suffix above), so filter by id.
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

/// Pull `data.notification_id` out of an SSE `data: {json}` line. Returns None
/// if the line isn't JSON or lacks the field.
fn extract_notification_id(line: &str) -> Option<String> {
    let json_part = line.strip_prefix("data:").unwrap_or(line).trim();
    let v: serde_json::Value = serde_json::from_str(json_part).ok()?;
    v.get("data")?
        .get("notification_id")?
        .as_str()
        .map(str::to_string)
}
