use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::HeaderMap;
use axum::{routing::get, Json, Router};
use lucidos_gateway_stamp::row_stream;
use serde_json::json;
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use crate::engine_hop::{test_engine, EngineAddress};
use crate::notification_count_tests::{listing, seed_device};
use crate::server::GatewayState;
use crate::stack::{Health, StackRuntime};

struct Engine {
    address: EngineAddress,
    frames: mpsc::UnboundedSender<Option<String>>,
    unread: Arc<AtomicU64>,
    device_unread: Arc<AtomicU64>,
    stream_requests: Arc<AtomicU64>,
    unanswered: Arc<std::sync::atomic::AtomicBool>,
    shared_reads: Arc<AtomicU64>,
    device_reads: Arc<AtomicU64>,
    _dir: tempfile::TempDir,
}

impl Engine {
    fn serve() -> Self {
        let dir = test_engine::scratch();
        let (frames, rx) = mpsc::unbounded_channel::<Option<String>>();
        let feed = Arc::new(AsyncMutex::new(rx));
        let unread = Arc::new(AtomicU64::new(0));
        let device_unread = Arc::new(AtomicU64::new(0));
        let stream_requests = Arc::new(AtomicU64::new(0));
        let unanswered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shared_reads = Arc::new(AtomicU64::new(0));
        let device_reads = Arc::new(AtomicU64::new(0));
        let app = Router::new()
            .route(row_stream::PATH, {
                let stream_requests = stream_requests.clone();
                let unanswered = unanswered.clone();
                get(move || {
                    stream_requests.fetch_add(1, Ordering::SeqCst);
                    let feed = feed.clone();
                    let unanswered = unanswered.load(Ordering::SeqCst);
                    async move {
                        if unanswered {
                            std::future::pending::<()>().await;
                        }
                        let hello = json!({"type": "StreamHello", "data": {}}).to_string();
                        let body = futures::stream::unfold(Some(hello), move |first| {
                            let feed = feed.clone();
                            async move {
                                let json = match first {
                                    Some(hello) => hello,
                                    None => feed.lock().await.recv().await??,
                                };
                                let frame = format!("data: {json}\n\n");
                                Some((Ok::<_, std::convert::Infallible>(frame), None))
                            }
                        });
                        Body::from_stream(body)
                    }
                })
            })
            .route("/api/v1/notifications", {
                let (unread, device_unread) = (unread.clone(), device_unread.clone());
                let (shared_reads, device_reads) = (shared_reads.clone(), device_reads.clone());
                get(move |headers: HeaderMap| async move {
                    let count = if headers.contains_key(crate::stack::HEADER_DEVICE_ID) {
                        device_reads.fetch_add(1, Ordering::SeqCst);
                        device_unread.load(Ordering::SeqCst)
                    } else {
                        shared_reads.fetch_add(1, Ordering::SeqCst);
                        unread.load(Ordering::SeqCst)
                    };
                    Json(json!({"notifications": [], "unread_count": count, "has_more": false}))
                })
            })
            .route(
                "/api/v1/backup/last-successful",
                get(|| async { Json(json!({"at": null, "stale": true, "configured": true})) }),
            );
        Self {
            address: test_engine::serve(&dir, app),
            frames,
            unread,
            device_unread,
            stream_requests,
            unanswered,
            shared_reads,
            device_reads,
            _dir: dir,
        }
    }

    fn send(&self, frame: serde_json::Value) {
        self.frames.send(Some(frame.to_string())).unwrap();
    }

    fn event(&self, event_type: &str, aggregate: &str) {
        self.send(json!({"type": event_type, "aggregate": aggregate}));
    }
}

async fn gateway(engine: &Engine) -> (GatewayState, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(root.path().to_path_buf()));
    state.set_stamp_key_for_test("dev", lucidos_gateway_stamp::StampKey::generate().unwrap());
    state.set_route_for_test("dev", engine.address.clone());
    state.install_notification_stack_for_test(root.path()).await;
    let stack = state.stack_for_test("dev").await;
    stack.lock().await.stop_row_subscription();
    (state, root)
}

async fn probe(state: &GatewayState, engine: &Engine, event_filter: bool) {
    let stack = state.stack_for_test("dev").await;
    let mut s = stack.lock().await;
    s.keep_row_subscription(event_filter, &stack, &engine.address);
}

async fn until(state: &GatewayState, what: &str, seen: impl Fn(&StackRuntime) -> bool) {
    let stack = state.stack_for_test("dev").await;
    for _ in 0..500 {
        if seen(&*stack.lock().await) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the row never showed: {what}");
}

async fn device_count(state: &GatewayState) -> serde_json::Value {
    listing(state, "first-secret").await["workspaces"][0]
        .get("unread_count")
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

#[tokio::test]
async fn an_engine_without_the_filter_is_never_subscribed_to() {
    let engine = Engine::serve();
    let (state, _root) = gateway(&engine).await;
    seed_device(&state, "first", "first-secret");
    probe(&state, &engine, false).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(engine.stream_requests.load(Ordering::SeqCst), 0);
    assert_eq!(engine.shared_reads.load(Ordering::SeqCst), 0);
    let row = &state.list_status().await[0];
    assert_eq!(row.health, Health::Healthy);
    assert_eq!((row.unread_count, row.last_successful_backup), (None, None));
    assert_eq!(device_count(&state).await, serde_json::Value::Null);
    assert_eq!(engine.device_reads.load(Ordering::SeqCst), 0);

    engine.unread.store(4, Ordering::SeqCst);
    assert_eq!(state.shared_unread_total().await, 4);
    assert_eq!(engine.shared_reads.load(Ordering::SeqCst), 1);
    assert_eq!(engine.stream_requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_engine_without_the_stream_is_noted_once_and_respawned_once() {
    let mut row = crate::row_subscription::RowSubscription::live_for_test();
    assert!(!row.engine_lacks_stream() && !row.stream_respawn_owed());
    assert!(row.note_engine_stream(false), "the first sight is logged");
    assert!(!row.note_engine_stream(false), "the next pass is not");
    assert!(row.engine_lacks_stream() && row.stream_respawn_owed());

    row.note_stream_respawn();
    row.stop();
    assert!(!row.note_engine_stream(false));
    assert!(row.engine_lacks_stream(), "the dock still asks it directly");
    assert!(!row.stream_respawn_owed(), "never respawned twice");

    assert!(!row.note_engine_stream(true));
    assert!(!row.engine_lacks_stream() && !row.stream_respawn_owed());
    assert!(
        row.note_engine_stream(false),
        "an older engine is seen anew"
    );
}

#[tokio::test]
async fn a_hello_and_each_accepted_event_read_the_snapshot_again() {
    let engine = Engine::serve();
    engine.unread.store(4, Ordering::SeqCst);
    let (state, _root) = gateway(&engine).await;
    probe(&state, &engine, true).await;
    until(&state, "the hello's snapshot", |s| s.last_unread == Some(4)).await;
    until(&state, "the backup line", |s| {
        s.last_successful_backup.is_some()
    })
    .await;
    assert_eq!(engine.stream_requests.load(Ordering::SeqCst), 1);
    assert_eq!(engine.shared_reads.load(Ordering::SeqCst), 1);
    assert_eq!(state.shared_unread_total().await, 4);

    probe(&state, &engine, true).await;
    engine.send(json!({"type": "ThreadEvent", "data": {"event": {"text": "private"}}}));
    engine.event("NativePushRequested", "notification");
    engine.unread.store(3, Ordering::SeqCst);
    engine.event("NotificationRead", "notification");
    until(&state, "the count after a read", |s| {
        s.last_unread == Some(3)
    })
    .await;
    assert_eq!(engine.stream_requests.load(Ordering::SeqCst), 1);
    assert_eq!(engine.shared_reads.load(Ordering::SeqCst), 2);
    assert_eq!(state.shared_unread_total().await, 3);
    assert_eq!(engine.shared_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_dropped_stream_blanks_the_row_until_the_next_snapshot() {
    let engine = Engine::serve();
    engine.unread.store(7, Ordering::SeqCst);
    let (state, _root) = gateway(&engine).await;
    probe(&state, &engine, true).await;
    until(&state, "the first snapshot", |s| s.last_unread == Some(7)).await;

    engine.frames.send(None).unwrap();
    until(&state, "a blank row", |s| !s.row_subscription.is_live()).await;
    let row = &state.list_status().await[0];
    assert_eq!((row.unread_count, row.last_successful_backup), (None, None));
    assert_eq!(state.shared_unread_total().await, 0);

    engine.unread.store(2, Ordering::SeqCst);
    probe(&state, &engine, true).await;
    until(&state, "the row after a restart", |s| {
        s.last_unread == Some(2)
    })
    .await;
    assert_eq!(engine.stream_requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_device_count_is_held_between_the_frames_that_could_move_it() {
    let engine = Engine::serve();
    engine.device_unread.store(5, Ordering::SeqCst);
    let (state, _root) = gateway(&engine).await;
    seed_device(&state, "first", "first-secret");
    probe(&state, &engine, true).await;
    until(&state, "the first snapshot", |s| s.last_unread == Some(0)).await;
    let reads = || engine.device_reads.load(Ordering::SeqCst);

    assert_eq!(device_count(&state).await, 5);
    assert_eq!(device_count(&state).await, 5);
    assert_eq!(reads(), 1, "the second listing asked no engine");

    engine.event("BackupCompleted", "ops");
    until(&state, "the read after a backup", |_| {
        engine.shared_reads.load(Ordering::SeqCst) == 2
    })
    .await;
    assert_eq!(device_count(&state).await, 5);
    assert_eq!(reads(), 1, "a backup does not move a device's count");

    for (step, frame) in [
        json!({"type": "NotificationCreated", "aggregate": "notification"}),
        json!({"type": "ThreadsDeleted", "aggregate": "ops"}),
        json!({"type": "UsersChanged", "aggregate": "user"}),
        json!({"type": "Lagged", "data": {"count": 9}}),
    ]
    .into_iter()
    .enumerate()
    {
        let (count, shared) = (6 + step as u64, 3 + step as u64);
        engine.device_unread.store(count, Ordering::SeqCst);
        engine.unread.store(count + 10, Ordering::SeqCst);
        engine.send(frame);
        until(&state, "the shared count after a frame", |s| {
            s.last_unread == Some(count + 10)
        })
        .await;
        assert_eq!(engine.shared_reads.load(Ordering::SeqCst), shared);
        assert_eq!(device_count(&state).await, count);
        assert_eq!(device_count(&state).await, count);
        assert_eq!(reads(), 2 + step as u64);
    }

    engine.frames.send(None).unwrap();
    until(&state, "the stream's end", |s| {
        !s.row_subscription.is_live()
    })
    .await;
    assert_eq!(device_count(&state).await, serde_json::Value::Null);
    assert_eq!(reads(), 5);
}

#[tokio::test]
async fn a_stalled_stream_is_dropped_and_subscribed_to_again() {
    let engine = Engine::serve();
    engine.unread.store(7, Ordering::SeqCst);
    let (state, _root) = gateway(&engine).await;
    let stack = state.stack_for_test("dev").await;
    let limit = Duration::from_millis(1500);
    stack
        .lock()
        .await
        .row_subscription
        .set_silence_limit_for_test(limit);
    probe(&state, &engine, true).await;
    until(&state, "the first snapshot", |s| s.last_unread == Some(7)).await;

    tokio::time::sleep(limit / 3).await;
    engine.unread.store(6, Ordering::SeqCst);
    engine.event("NotificationRead", "notification");
    until(&state, "the count after a read", |s| {
        s.last_unread == Some(6)
    })
    .await;
    assert_eq!(engine.stream_requests.load(Ordering::SeqCst), 1);

    engine.unread.store(2, Ordering::SeqCst);
    until(&state, "a blank row", |s| !s.row_subscription.is_live()).await;
    let row = &state.list_status().await[0];
    assert_eq!((row.unread_count, row.last_successful_backup), (None, None));

    probe(&state, &engine, true).await;
    until(&state, "the row after the stall", |s| {
        s.last_unread == Some(2)
    })
    .await;
    assert_eq!(engine.stream_requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_unanswered_stream_request_is_dropped_and_sent_again() {
    let engine = Engine::serve();
    engine.unread.store(3, Ordering::SeqCst);
    engine.unanswered.store(true, Ordering::SeqCst);
    let (state, _root) = gateway(&engine).await;
    let stack = state.stack_for_test("dev").await;
    stack
        .lock()
        .await
        .row_subscription
        .set_silence_limit_for_test(Duration::from_millis(300));
    probe(&state, &engine, true).await;
    assert!(stack.lock().await.row_subscription.is_live());
    until(&state, "the task's end", |s| !s.row_subscription.is_live()).await;
    assert_eq!(engine.stream_requests.load(Ordering::SeqCst), 1);
    assert_eq!(state.list_status().await[0].unread_count, None);

    engine.unanswered.store(false, Ordering::SeqCst);
    probe(&state, &engine, true).await;
    until(&state, "the row once the engine answers", |s| {
        s.last_unread == Some(3)
    })
    .await;
}

#[tokio::test]
async fn a_read_that_started_before_a_drop_is_not_held() {
    let mut row = crate::row_subscription::RowSubscription::live_for_test();
    let (held, epoch) = row.device_unread("first").expect("a live subscription");
    assert_eq!(held, None);
    row.forget_devices();
    row.remember_device(epoch, "first", 9);
    assert_eq!(row.device_unread("first").unwrap().0, None);

    let (_, epoch) = row.device_unread("first").unwrap();
    row.remember_device(epoch, "first", 9);
    assert_eq!(row.device_unread("first").unwrap().0, Some(9));
    row.stop();
    assert_eq!(row.device_unread("first"), None);
}
