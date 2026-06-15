use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32};
use std::sync::Arc;

use uuid::Uuid;

use crate::engine::change_ops::now_epoch_millis;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::engine::types::{AgentSession, AgentUserInput};
use crate::test_support::{setup_test_db, teardown_test_db};

use super::ExternalWatchdog;

/// Without this seed, every CC-only emit (`ContinuationRequested`) fails with
/// "is not valid for Chat threads" — the lifecycle projection needs a
/// prior CC-channel event to classify the thread.
async fn seed_cc_thread(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-test".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("SessionStarted emit")
    .expect("SessionStarted persisted");
}

fn make_session(last_event_at_ms: i64) -> AgentSession {
    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
    let (control_tx, _control_rx) = tokio::sync::mpsc::unbounded_channel();
    AgentSession {
        msg_tx,
        is_waiting: false,
        has_changes: false,
        requires_restart: false,
        pending_stop: None,
        cancel_actor: None,
        stop: Arc::new(tokio::sync::Notify::new()),
        interrupt: Arc::new(tokio::sync::Notify::new()),
        idle_notify: Arc::new(tokio::sync::Notify::new()),
        apply_now_in_progress: false,
        process_exited: false,
        worktree_path: None,
        branch_name: None,
        repo_root: None,
        cc_session_id: None,
        shutting_down: Arc::new(AtomicBool::new(false)),
        external_terminal_emitted: Arc::new(AtomicBool::new(false)),
        control_tx,
        builtin_commands: vec![],
        skill_commands: vec![],
        current_model: None,
        current_reasoning_effort: None,
        last_event_at: Arc::new(AtomicI64::new(last_event_at_ms)),
        pending_followups: Arc::new(AtomicU32::new(0)),
        tools_in_flight: Arc::new(AtomicI32::new(0)),
    }
}

fn stale_for(limit_ms: i64) -> i64 {
    now_epoch_millis() - limit_ms - 5
}

async fn count_continuation_requests(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ContinuationRequested'",
    )
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    .expect("count query")
}

#[tokio::test]
async fn tick_fires_for_stuck_session_emits_continuation_requested_and_drops_entry() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    sessions
        .lock()
        .await
        .insert(thread_id, make_session(stale_for(limit_ms)));

    let watchdog = ExternalWatchdog::new(sessions.clone(), bus.clone(), limit_ms);
    watchdog.tick().await;

    let count = count_continuation_requests(&pool, thread_id).await;
    assert_eq!(
        count, 1,
        "stuck session must produce exactly one ContinuationRequested"
    );
    assert!(
        !sessions.lock().await.contains_key(&thread_id),
        "stuck session's entry must be removed so spawn dispatcher can boot a fresh --resume"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tick_leaves_healthy_session_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 10 * 60 * 1000; // 10 min
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    // Last event 1 s ago — far inside 10-min limit.
    sessions
        .lock()
        .await
        .insert(thread_id, make_session(now_epoch_millis() - 1000));

    let watchdog = ExternalWatchdog::new(sessions.clone(), bus.clone(), limit_ms);
    watchdog.tick().await;

    assert_eq!(count_continuation_requests(&pool, thread_id).await, 0);
    assert!(sessions.lock().await.contains_key(&thread_id));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Double-fire here means two `SpawnRequest::Continue` dispatches and two
/// competing `--resume` sessions on the same worktree.
#[tokio::test]
async fn tick_leaves_exited_session_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let mut session = make_session(stale_for(limit_ms));
    session.process_exited = true;
    sessions.lock().await.insert(thread_id, session);

    let watchdog = ExternalWatchdog::new(sessions.clone(), bus.clone(), limit_ms);
    watchdog.tick().await;

    assert_eq!(count_continuation_requests(&pool, thread_id).await, 0);
    // We don't assert presence — the in-loop owns removal on exit.

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tick_leaves_session_with_tool_in_flight_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let session = make_session(stale_for(limit_ms));
    session
        .tools_in_flight
        .store(2, std::sync::atomic::Ordering::Relaxed);
    sessions.lock().await.insert(thread_id, session);

    let watchdog = ExternalWatchdog::new(sessions.clone(), bus.clone(), limit_ms);
    watchdog.tick().await;

    assert_eq!(count_continuation_requests(&pool, thread_id).await, 0);
    assert!(sessions.lock().await.contains_key(&thread_id));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tick_leaves_waiting_session_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let mut session = make_session(stale_for(limit_ms));
    session.is_waiting = true;
    sessions.lock().await.insert(thread_id, session);

    let watchdog = ExternalWatchdog::new(sessions.clone(), bus.clone(), limit_ms);
    watchdog.tick().await;

    assert_eq!(count_continuation_requests(&pool, thread_id).await, 0);
    assert!(sessions.lock().await.contains_key(&thread_id));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tick_with_empty_sessions_is_noop() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let watchdog = ExternalWatchdog::new(sessions, bus, 50);
    watchdog.tick().await; // must not panic / hang

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Stuck session's `external_terminal_emitted` flag MUST flip to true.
/// This is the suppression the wedged in-loop's safety net checks at
/// `run_session.rs::external_terminal_already_emitted`. If we forgot to
/// flip it, the wedged loop (when it eventually wakes) would emit a
/// duplicate `ResponseAborted` on top of our `ContinuationRequested` — the user
/// would see both an auto-resume AND an "Aborted" terminal.
#[tokio::test]
async fn tick_flips_external_terminal_emitted_before_dropping_stuck_session() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let session = make_session(stale_for(limit_ms));
    let flag = session.external_terminal_emitted.clone();
    sessions.lock().await.insert(thread_id, session);

    assert!(
        !flag.load(std::sync::atomic::Ordering::Acquire),
        "precondition: flag starts unset"
    );

    let watchdog = ExternalWatchdog::new(sessions.clone(), bus.clone(), limit_ms);
    watchdog.tick().await;

    assert!(
        flag.load(std::sync::atomic::Ordering::Acquire),
        "tick must set external_terminal_emitted=true to suppress the wedged in-loop's safety-net abort"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
