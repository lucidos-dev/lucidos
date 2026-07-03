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

/// Hung-tool ceiling for the integration ticks. Larger than every `limit_ms`
/// used below so "stale past the limit" and "stale past the ceiling" stay
/// distinct regimes.
const CEILING_MS: i64 = 10 * 60 * 1000;

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
        redirect_followup: false,
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
        question_resume_pending: false,
        tools_in_flight: Arc::new(AtomicI32::new(0)),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        agent_cancel: tokio_util::sync::CancellationToken::new(),
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
    let session = make_session(stale_for(limit_ms));
    let cancel = session.agent_cancel.clone();
    sessions.lock().await.insert(thread_id, session);

    let watchdog =
        ExternalWatchdog::new(sessions.clone(), bus.clone(), pool.clone(), limit_ms, CEILING_MS);
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
    assert!(
        cancel.is_cancelled(),
        "recovering a stuck session must cancel agent_cancel so the driver_task tears \
         down the subprocess — otherwise the --resume spawns a second concurrent agent"
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
    let session = make_session(now_epoch_millis() - 1000);
    let cancel = session.agent_cancel.clone();
    sessions.lock().await.insert(thread_id, session);

    let watchdog =
        ExternalWatchdog::new(sessions.clone(), bus.clone(), pool.clone(), limit_ms, CEILING_MS);
    watchdog.tick().await;

    assert_eq!(count_continuation_requests(&pool, thread_id).await, 0);
    assert!(sessions.lock().await.contains_key(&thread_id));
    assert!(
        !cancel.is_cancelled(),
        "a healthy (Skip) session must never have its subprocess cancelled"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Phase B: an exited session whose projection is still `running` and stale
/// past the limit means the in-loop cleanup is wedged / never settled — the
/// external watchdog recovers it (the prior unconditional `process_exited` skip
/// left it a zombie). `external_terminal_emitted` + the `thread_is_running`
/// re-check prevent a double-fire with the in-loop.
#[tokio::test]
async fn tick_recovers_exited_stale_session_when_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await; // SessionStarted → status='running'
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let mut session = make_session(stale_for(limit_ms));
    session.process_exited = true;
    sessions.lock().await.insert(thread_id, session);

    let watchdog =
        ExternalWatchdog::new(sessions.clone(), bus.clone(), pool.clone(), limit_ms, CEILING_MS);
    watchdog.tick().await;

    assert_eq!(
        count_continuation_requests(&pool, thread_id).await,
        1,
        "exited + stale + still `running` (wedged in-loop cleanup) must be recovered"
    );
    assert!(
        !sessions.lock().await.contains_key(&thread_id),
        "recovered session's entry must be dropped"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An exited session that is NOT yet stale (within the limit) is left to the
/// in-loop cleanup — recovering here would race it. This is the case the
/// original unconditional `process_exited` skip protected, now narrowed to
/// "still being cleaned up".
#[tokio::test]
async fn tick_leaves_fresh_exited_session_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 10 * 60 * 1000; // 10 min — the session below is fresh
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let mut session = make_session(now_epoch_millis() - 1000); // 1 s ago
    session.process_exited = true;
    sessions.lock().await.insert(thread_id, session);

    let watchdog =
        ExternalWatchdog::new(sessions.clone(), bus.clone(), pool.clone(), limit_ms, CEILING_MS);
    watchdog.tick().await;

    assert_eq!(count_continuation_requests(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Phase A (the root-cause fix): a tool in flight past the hung-tool ceiling on
/// a still-`running` thread is recovered — the prior unbounded `tools_in_flight`
/// skip would have left it stuck forever (the thread-72120ca6 incident).
#[tokio::test]
async fn tick_recovers_hung_tool_past_ceiling_when_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await; // status='running'
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let session = make_session(stale_for(CEILING_MS)); // past the ceiling
    session
        .tools_in_flight
        .store(3, std::sync::atomic::Ordering::Relaxed);
    sessions.lock().await.insert(thread_id, session);

    let watchdog =
        ExternalWatchdog::new(sessions.clone(), bus.clone(), pool.clone(), limit_ms, CEILING_MS);
    watchdog.tick().await;

    assert_eq!(
        count_continuation_requests(&pool, thread_id).await,
        1,
        "hung tool past the ceiling on a running thread must be recovered"
    );
    assert!(!sessions.lock().await.contains_key(&thread_id));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The user-answer protection: a tool in flight past the ceiling on a thread
/// that is NOT `running` (here settled to idle; in production a pending
/// question/permission card sits at `waiting_for_user_answer`) must NOT be
/// euthanized — the `thread_is_running` re-check guards it.
#[tokio::test]
async fn tick_skips_hung_tool_past_ceiling_when_not_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await; // status='running'…
    // …then settle it so the projection is no longer `running` (stands in for
    // `waiting_for_user_answer`, which `thread_is_running` also treats as not-running).
    crate::engine::claude_code::settle_stuck_running_thread(&pool, &bus, thread_id, None)
        .await
        .expect("settle");

    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let session = make_session(stale_for(CEILING_MS));
    session
        .tools_in_flight
        .store(3, std::sync::atomic::Ordering::Relaxed);
    sessions.lock().await.insert(thread_id, session);

    let watchdog =
        ExternalWatchdog::new(sessions.clone(), bus.clone(), pool.clone(), limit_ms, CEILING_MS);
    watchdog.tick().await;

    assert_eq!(
        count_continuation_requests(&pool, thread_id).await,
        0,
        "a not-running thread (pending user answer / settled) must not be euthanized"
    );
    assert!(
        sessions.lock().await.contains_key(&thread_id),
        "entry must be left intact when the re-check declines to recover"
    );

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

    let watchdog =
        ExternalWatchdog::new(sessions.clone(), bus.clone(), pool.clone(), limit_ms, CEILING_MS);
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

    let watchdog =
        ExternalWatchdog::new(sessions.clone(), bus.clone(), pool.clone(), limit_ms, CEILING_MS);
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
    let watchdog = ExternalWatchdog::new(sessions, bus, pool.clone(), 50, CEILING_MS);
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

    let watchdog =
        ExternalWatchdog::new(sessions.clone(), bus.clone(), pool.clone(), limit_ms, CEILING_MS);
    watchdog.tick().await;

    assert!(
        flag.load(std::sync::atomic::Ordering::Acquire),
        "tick must set external_terminal_emitted=true to suppress the wedged in-loop's safety-net abort"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The liveness guard: a session that produced a fresh event *after* the
/// snapshot (its `last_event_at` advanced past `snapshot_last_ms`) has recovered
/// on its own — `recover_stuck` must leave it ENTIRELY alone: no token cancel
/// (so a live, progressing subprocess is never killed), no entry drop, and no
/// `ContinuationRequested`. Exercised at the `recover_stuck` seam so the
/// snapshot→mutate liveness window is deterministic.
#[tokio::test]
async fn recover_stuck_skips_session_that_recovered_since_snapshot() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;

    // Was stale when the snapshot ran…
    let snapshot_last_ms = stale_for(limit_ms);
    let session = make_session(snapshot_last_ms);
    let cancel = session.agent_cancel.clone();
    let last_event_at = session.last_event_at.clone();
    let external_terminal = session.external_terminal_emitted.clone();
    let idle_notify = session.idle_notify.clone();
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    sessions.lock().await.insert(thread_id, session);

    // …but a fresh event arrives before the mutate pass — the session recovered.
    last_event_at.store(now_epoch_millis(), std::sync::atomic::Ordering::Relaxed);

    let candidate = super::StuckSession {
        thread_id,
        elapsed_ms: 0,
        external_terminal,
        idle_notify,
        agent_cancel: cancel.clone(),
        last_event_at,
        snapshot_last_ms,
        needs_running_check: false,
    };

    let watchdog =
        ExternalWatchdog::new(sessions.clone(), bus.clone(), pool.clone(), limit_ms, CEILING_MS);
    watchdog.recover_stuck(vec![candidate]).await;

    assert!(
        !cancel.is_cancelled(),
        "a session that recovered since the snapshot must NOT be cancelled/killed"
    );
    assert!(
        sessions.lock().await.contains_key(&thread_id),
        "a recovered session's entry must be left intact (not dropped)"
    );
    assert_eq!(
        count_continuation_requests(&pool, thread_id).await,
        0,
        "a recovered session must not be resumed (no duplicate --resume)"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
