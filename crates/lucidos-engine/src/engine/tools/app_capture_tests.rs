//! Which device answers a `capture_app` / `refresh_app` call.
//!
//! Only the turn's last used device may answer. A quick "No app UI is currently
//! open" from another device must never beat the real capture.

use super::{app_ui_tool_impl, resolve_capture, CaptureRefusal, PendingCaptures, CAPTURE_TIMEOUT};
use crate::engine::event_bus::{BusEvent, EmittedEvent, EventBus};
use crate::engine::thread_events::{MessageOrigin, ThreadEvent};
use crate::engine::CaptureResult;
use crate::test_support::{seed_device, setup_test_db, teardown_test_db};
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use uuid::Uuid;

const PHONE: &str = "device-phone";
const LAPTOP: &str = "device-laptop";

/// An error in the shape a page with no app frame sends back (`captureAppUIInner`).
const NO_APP_OPEN: &str = "Error: No app UI is currently open. Ask the user to open the app.";

async fn setup() -> (Arc<EventBus>, Receiver<EmittedEvent>, PgPool, String) {
    let (pool, db) = setup_test_db().await;
    seed_device(&pool, PHONE, Some("iPhone Safari/604.1"), Some("My iPhone")).await;
    seed_device(&pool, LAPTOP, Some("Macintosh Chrome"), Some("My MacBook")).await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let events = bus.subscribe();
    (Arc::new(bus), events, pool, db)
}

fn answer(screenshot: &str, dom: &str) -> CaptureResult {
    CaptureResult {
        screenshot: screenshot.to_string(),
        dom: dom.to_string(),
    }
}

fn actor_device(actor: Option<&MessageOrigin>) -> Option<String> {
    match actor {
        Some(MessageOrigin::Device { device_id }) => Some(device_id.clone()),
        _ => None,
    }
}

/// The next `AppUiCaptureRequested` for `thread_id`: its request id and the
/// device it is scoped to.
async fn next_capture(
    rx: &mut Receiver<EmittedEvent>,
    thread_id: Uuid,
) -> (String, Option<String>) {
    loop {
        let ev = rx.recv().await.expect("broadcast channel should not close");
        if let BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::AppUiCaptureRequested { request_id, .. },
            meta,
        } = ev.typed
        {
            if tid == thread_id {
                return (request_id, actor_device(meta.actor.as_ref()));
            }
        }
    }
}

/// Start the tool on its own task, as the agentic loop would, so the test can
/// answer the capture while it waits.
fn start(
    bus: &Arc<EventBus>,
    pool: &PgPool,
    pending: &Arc<PendingCaptures>,
    thread_id: Uuid,
    refresh: bool,
    last_used: Option<&'static str>,
    timeout: Duration,
) -> tokio::task::JoinHandle<String> {
    let (bus, pool, pending) = (bus.clone(), pool.clone(), pending.clone());
    tokio::spawn(async move {
        let args = json!({ "app_id": "habit-tracker" });
        app_ui_tool_impl(
            &bus, &pool, &pending, &args, thread_id, refresh, last_used, timeout,
        )
        .await
    })
}

/// The regression: the page without a frame answers first, and the laptop's
/// real screenshot still reaches the agent.
#[tokio::test]
async fn a_page_without_the_app_cannot_beat_the_last_used_device() {
    let (bus, mut rx, pool, db) = setup().await;
    let pending: Arc<PendingCaptures> = Arc::default();
    let thread_id = Uuid::new_v4();

    let tool = start(
        &bus,
        &pool,
        &pending,
        thread_id,
        false,
        Some(LAPTOP),
        CAPTURE_TIMEOUT,
    );
    let (request_id, scoped_to) = next_capture(&mut rx, thread_id).await;
    assert_eq!(
        scoped_to.as_deref(),
        Some(LAPTOP),
        "the request names the last used device"
    );

    assert_eq!(
        resolve_capture(&pending, &request_id, Some(PHONE), answer("", NO_APP_OPEN)),
        Err(CaptureRefusal::OtherDevice),
        "the phone's quick error must be refused"
    );
    resolve_capture(
        &pending,
        &request_id,
        Some(LAPTOP),
        answer("c2NyZWVu", "<main>Habits</main>"),
    )
    .expect("the laptop answers its own request");

    let out = tool.await.expect("tool task");
    assert!(out.starts_with("[APP_CAPTURE:c2NyZWVu]"), "{out}");
    assert!(out.contains("<main>Habits</main>"), "{out}");
    assert!(
        pending.lock().unwrap().is_empty(),
        "the answered request is gone"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// `refresh_app` reloads only the last used device's app, then captures it.
#[tokio::test]
async fn refresh_is_scoped_to_the_last_used_device_too() {
    let (bus, mut rx, pool, db) = setup().await;
    let pending: Arc<PendingCaptures> = Arc::default();
    let thread_id = Uuid::new_v4();

    let tool = start(
        &bus,
        &pool,
        &pending,
        thread_id,
        true,
        Some(LAPTOP),
        CAPTURE_TIMEOUT,
    );
    let refresh_actor = loop {
        let ev = rx.recv().await.expect("broadcast channel should not close");
        if let BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::AppUiRefreshRequested { .. },
            meta,
        } = ev.typed
        {
            if tid == thread_id {
                break actor_device(meta.actor.as_ref());
            }
        }
    };
    assert_eq!(refresh_actor.as_deref(), Some(LAPTOP));

    let (request_id, _) = next_capture(&mut rx, thread_id).await;
    resolve_capture(
        &pending,
        &request_id,
        Some(LAPTOP),
        answer("c2NyZWVu", "<main/>"),
    )
    .expect("the laptop answers");
    assert!(tool.await.expect("tool task").starts_with("[APP_CAPTURE:"));

    pool.close().await;
    teardown_test_db(&db).await;
}

/// An error the target sends back names it, so the agent knows which screen
/// has no app open.
#[tokio::test]
async fn an_error_from_the_last_used_device_names_it() {
    let (bus, mut rx, pool, db) = setup().await;
    let pending: Arc<PendingCaptures> = Arc::default();
    let thread_id = Uuid::new_v4();

    let tool = start(
        &bus,
        &pool,
        &pending,
        thread_id,
        false,
        Some(LAPTOP),
        CAPTURE_TIMEOUT,
    );
    let (request_id, _) = next_capture(&mut rx, thread_id).await;
    resolve_capture(&pending, &request_id, Some(LAPTOP), answer("", NO_APP_OPEN))
        .expect("the laptop answers");

    let out = tool.await.expect("tool task");
    assert!(out.starts_with("Error from \"My MacBook\""), "{out}");
    assert!(out.contains("No app UI is currently open"), "{out}");

    pool.close().await;
    teardown_test_db(&db).await;
}

/// No answer: the result names the device that was asked, never a generic
/// timeout.
#[tokio::test]
async fn a_timeout_names_the_device_that_did_not_answer() {
    let (bus, mut rx, pool, db) = setup().await;
    let pending: Arc<PendingCaptures> = Arc::default();
    let thread_id = Uuid::new_v4();

    let tool = start(
        &bus,
        &pool,
        &pending,
        thread_id,
        false,
        Some(LAPTOP),
        Duration::from_millis(300),
    );
    let (request_id, _) = next_capture(&mut rx, thread_id).await;
    assert_eq!(
        resolve_capture(&pending, &request_id, Some(PHONE), answer("", NO_APP_OPEN)),
        Err(CaptureRefusal::OtherDevice)
    );

    let out = tool.await.expect("tool task");
    assert!(out.starts_with("Error: Capture timed out"), "{out}");
    assert!(out.contains("\"My MacBook\""), "{out}");
    assert!(!out.contains("My iPhone"), "{out}");
    assert!(
        pending.lock().unwrap().is_empty(),
        "a timed-out request is removed"
    );
    assert_eq!(
        resolve_capture(&pending, &request_id, Some(LAPTOP), answer("late", "")),
        Err(CaptureRefusal::Unknown),
        "a late answer finds nothing to resolve"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A background or trigger turn has no last used device. The request names no
/// device, and the first answer from any page is taken, as before.
#[tokio::test]
async fn with_no_last_used_device_any_page_may_answer() {
    let (bus, mut rx, pool, db) = setup().await;
    let pending: Arc<PendingCaptures> = Arc::default();
    let thread_id = Uuid::new_v4();

    let tool = start(
        &bus,
        &pool,
        &pending,
        thread_id,
        false,
        None,
        CAPTURE_TIMEOUT,
    );
    let (request_id, scoped_to) = next_capture(&mut rx, thread_id).await;
    assert_eq!(scoped_to, None, "an untargeted request names no device");
    resolve_capture(&pending, &request_id, Some(PHONE), answer("", NO_APP_OPEN))
        .expect("any page may answer");
    assert_eq!(tool.await.expect("tool task"), NO_APP_OPEN);

    let tool = start(
        &bus,
        &pool,
        &pending,
        thread_id,
        false,
        None,
        Duration::from_millis(300),
    );
    next_capture(&mut rx, thread_id).await;
    let out = tool.await.expect("tool task");
    assert!(out.contains("no last used device"), "{out}");

    pool.close().await;
    teardown_test_db(&db).await;
}
