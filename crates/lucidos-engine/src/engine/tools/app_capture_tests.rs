//! Which device answers a `capture_app` / `refresh_app` call.
//!
//! Only the turn's last used device may answer. A quick "No app UI is currently
//! open" from another device must never beat the real capture.

use super::{
    app_ui_tool_impl, parse_save_target, resolve_capture, CaptureRefusal, PendingCaptures,
    SaveTarget, CAPTURE_TIMEOUT,
};
use crate::core::ArtifactManager;
use crate::engine::event_bus::{BusEvent, EmittedEvent, EventBus};
use crate::engine::thread_events::{CaptureFormat, MessageOrigin, ThreadEvent};
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
    let (request_id, device, _) = next_capture_with_format(rx, thread_id).await;
    (request_id, device)
}

/// [`next_capture`], plus the save format the request asks the frame for.
async fn next_capture_with_format(
    rx: &mut Receiver<EmittedEvent>,
    thread_id: Uuid,
) -> (String, Option<String>, Option<CaptureFormat>) {
    loop {
        let ev = rx.recv().await.expect("broadcast channel should not close");
        if let BusEvent::Thread {
            thread_id: tid,
            event:
                ThreadEvent::AppUiCaptureRequested {
                    request_id,
                    save_format,
                    ..
                },
            meta,
        } = ev.typed
        {
            if tid == thread_id {
                return (request_id, actor_device(meta.actor.as_ref()), save_format);
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
    let workspace = tempfile::tempdir().expect("tempdir");
    let artifacts = Arc::new(ArtifactManager::new(workspace.path().to_path_buf()).unwrap());
    let args = json!({ "app_id": "habit-tracker" });
    let tool = start_with(
        bus, pool, pending, &artifacts, args, thread_id, refresh, last_used, timeout,
    );
    // The workspace lives as long as the tool task that may write into it.
    tokio::spawn(async move {
        let out = tool.await.expect("tool task");
        drop(workspace);
        out
    })
}

/// [`start`] with the caller's arguments and artifact store.
#[allow(clippy::too_many_arguments)]
fn start_with(
    bus: &Arc<EventBus>,
    pool: &PgPool,
    pending: &Arc<PendingCaptures>,
    artifacts: &Arc<ArtifactManager>,
    args: serde_json::Value,
    thread_id: Uuid,
    refresh: bool,
    last_used: Option<&'static str>,
    timeout: Duration,
) -> tokio::task::JoinHandle<String> {
    let (bus, pool, pending, artifacts) = (
        bus.clone(),
        pool.clone(),
        pending.clone(),
        artifacts.clone(),
    );
    tokio::spawn(async move {
        app_ui_tool_impl(
            &bus, &pool, &pending, &artifacts, &args, thread_id, refresh, last_used, timeout,
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

/// A picture in `format`, base64 encoded, as a frame would send it.
fn picture(format: image::ImageFormat) -> String {
    use base64::Engine;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(6, 4)
        .write_to(&mut bytes, format)
        .expect("encode test picture");
    base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
}

#[test]
fn a_save_path_must_stay_in_artifacts_and_name_a_format() {
    let target = |path: &str| parse_save_target(&json!({ "save_as_artifact": path }));
    assert_eq!(parse_save_target(&json!({})), Ok(None));
    assert_eq!(
        target("shots/Board.PNG"),
        Ok(Some(SaveTarget {
            path: "shots/Board.PNG".into(),
            format: CaptureFormat::Png,
        }))
    );
    assert_eq!(
        target("a.jpg").unwrap().unwrap().format,
        CaptureFormat::Jpeg
    );
    assert_eq!(
        target("a.jpeg").unwrap().unwrap().format,
        CaptureFormat::Jpeg
    );
    assert_eq!(
        target("a.webp").unwrap().unwrap().format,
        CaptureFormat::Webp
    );
    for bad in ["../a.png", "/etc/a.png", "\\a.png", "a.gif", "a", "a.svg"] {
        let err = target(bad).expect_err(bad);
        assert!(err.contains(bad), "the error names the path: {err}");
    }
    assert!(target("a.gif")
        .unwrap_err()
        .contains(&CaptureFormat::extensions()));
}

/// A bad path is refused before any device is asked.
#[tokio::test]
async fn a_bad_save_path_asks_no_device() {
    let (bus, _rx, pool, db) = setup().await;
    let pending: Arc<PendingCaptures> = Arc::default();
    let workspace = tempfile::tempdir().unwrap();
    let artifacts = Arc::new(ArtifactManager::new(workspace.path().to_path_buf()).unwrap());

    let args = json!({ "app_id": "habit-tracker", "save_as_artifact": "../escape.png" });
    let out = start_with(
        &bus,
        &pool,
        &pending,
        &artifacts,
        args,
        Uuid::new_v4(),
        false,
        Some(LAPTOP),
        CAPTURE_TIMEOUT,
    )
    .await
    .expect("tool task");
    assert!(out.starts_with("Error: Invalid save_as_artifact"), "{out}");
    assert!(pending.lock().unwrap().is_empty(), "no request was opened");

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Run one saved capture to `path`, answered with `screenshot`. Returns the
/// tool result, the format the request asked for, and the workspace.
async fn saved_capture(
    path: &str,
    screenshot: &str,
    dom: &str,
) -> (
    String,
    Option<CaptureFormat>,
    tempfile::TempDir,
    Vec<String>,
) {
    let (bus, mut rx, pool, db) = setup().await;
    let mut announced = bus.subscribe();
    let pending: Arc<PendingCaptures> = Arc::default();
    let workspace = tempfile::tempdir().unwrap();
    let artifacts = Arc::new(ArtifactManager::new(workspace.path().to_path_buf()).unwrap());
    let thread_id = Uuid::new_v4();

    let args = json!({ "app_id": "habit-tracker", "save_as_artifact": path });
    let tool = start_with(
        &bus,
        &pool,
        &pending,
        &artifacts,
        args,
        thread_id,
        false,
        Some(LAPTOP),
        CAPTURE_TIMEOUT,
    );
    let (request_id, _, format) = next_capture_with_format(&mut rx, thread_id).await;
    resolve_capture(&pending, &request_id, Some(LAPTOP), answer(screenshot, dom))
        .expect("the laptop answers");
    let out = tool.await.expect("tool task");

    let mut created = Vec::new();
    while let Ok(ev) = announced.try_recv() {
        if let BusEvent::System(crate::engine::event_bus::SystemEvent::ArtifactCreated {
            artifact_path,
            writer_thread_id,
            ..
        }) = ev.typed
        {
            assert_eq!(writer_thread_id, Some(thread_id), "the writer is named");
            created.push(artifact_path);
        }
    }
    pool.close().await;
    teardown_test_db(&db).await;
    (out, format, workspace, created)
}

#[tokio::test]
async fn a_saved_capture_is_written_announced_and_still_seen() {
    let png = picture(image::ImageFormat::Png);
    let (out, format, workspace, created) = saved_capture("shots/board.png", &png, "<main/>").await;

    assert_eq!(
        format,
        Some(CaptureFormat::Png),
        "the frame is asked for PNG"
    );
    assert!(out.starts_with(&format!("[APP_CAPTURE:{png}]")), "{out}");
    assert!(
        out.contains("Saved the screenshot to shots/board.png (image/png, 6x4 px"),
        "{out}"
    );
    assert!(out.contains("<main/>"), "{out}");
    let saved = std::fs::read(workspace.path().join("data/artifacts/shots/board.png")).unwrap();
    assert_eq!(
        crate::core::blobs::sniff_image_mime(&saved),
        Some("image/png")
    );
    assert_eq!(created, vec!["shots/board.png".to_string()]);
}

/// A device that sent another format than the path names saves nothing.
#[tokio::test]
async fn a_picture_in_the_wrong_format_is_not_saved() {
    let jpeg = picture(image::ImageFormat::Jpeg);
    let (out, _, workspace, created) = saved_capture("board.png", &jpeg, "<main/>").await;

    assert!(
        out.starts_with("Error: Nothing saved to board.png"),
        "{out}"
    );
    assert!(out.contains("sent image/jpeg, not image/png"), "{out}");
    assert!(!workspace.path().join("data/artifacts/board.png").exists());
    assert!(created.is_empty());
}

/// A DOM-only answer carries no picture, so the result says nothing was saved.
#[tokio::test]
async fn a_capture_without_a_picture_saves_nothing() {
    let (out, _, workspace, created) =
        saved_capture("board.png", "", "[screenshot unavailable: decode]\n<main/>").await;

    assert!(
        out.starts_with("Error: Nothing saved to board.png: the capture carried no picture."),
        "{out}"
    );
    assert!(
        out.contains("<main/>"),
        "the DOM still reaches the agent: {out}"
    );
    assert!(!workspace.path().join("data/artifacts/board.png").exists());
    assert!(created.is_empty());
}

/// A write that fails is reported, never claimed as a save.
#[tokio::test]
async fn a_failed_write_is_reported() {
    let (bus, mut rx, pool, db) = setup().await;
    let pending: Arc<PendingCaptures> = Arc::default();
    let workspace = tempfile::tempdir().unwrap();
    let artifacts = Arc::new(ArtifactManager::new(workspace.path().to_path_buf()).unwrap());
    // A file where the target's folder must go.
    std::fs::write(
        workspace.path().join("data/artifacts/shots"),
        "not a folder",
    )
    .unwrap();
    let thread_id = Uuid::new_v4();

    let args = json!({ "app_id": "habit-tracker", "save_as_artifact": "shots/board.png" });
    let tool = start_with(
        &bus,
        &pool,
        &pending,
        &artifacts,
        args,
        thread_id,
        false,
        Some(LAPTOP),
        CAPTURE_TIMEOUT,
    );
    let (request_id, _) = next_capture(&mut rx, thread_id).await;
    let png = picture(image::ImageFormat::Png);
    resolve_capture(&pending, &request_id, Some(LAPTOP), answer(&png, "<main/>")).unwrap();
    let out = tool.await.expect("tool task");
    assert!(
        out.starts_with("Error: Saving shots/board.png failed:"),
        "{out}"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Without a save path, the request asks for no format, and the wire omits it.
#[tokio::test]
async fn a_plain_capture_asks_for_no_format() {
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
    let (request_id, _, format) = next_capture_with_format(&mut rx, thread_id).await;
    assert_eq!(format, None);
    resolve_capture(
        &pending,
        &request_id,
        Some(LAPTOP),
        answer("c2NyZWVu", "<main/>"),
    )
    .unwrap();
    let out = tool.await.expect("tool task");
    assert!(
        out.starts_with("[APP_CAPTURE:c2NyZWVu]\nDOM snapshot:"),
        "{out}"
    );

    let wire = serde_json::to_value(ThreadEvent::AppUiCaptureRequested {
        app_id: "a".into(),
        request_id: "r".into(),
        save_format: None,
    })
    .unwrap();
    assert!(wire["data"].get("save_format").is_none(), "{wire}");

    pool.close().await;
    teardown_test_db(&db).await;
}
