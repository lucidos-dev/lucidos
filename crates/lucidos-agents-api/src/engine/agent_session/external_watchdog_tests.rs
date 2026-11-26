use std::collections::HashMap;
use std::sync::atomic::AtomicI64;
use std::sync::Arc;

use uuid::Uuid;

use crate::engine::change_ops::now_epoch_millis;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::engine::types::{AgentSession, AgentUserInput};
use crate::test_support::{setup_test_db, teardown_test_db};

use super::ExternalWatchdog;

const CEILING_MS: i64 = 10 * 60 * 1000;

async fn seed_cc_thread(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-test".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
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

fn make_session(
    last_event_at_ms: i64,
) -> (
    AgentSession,
    tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>,
) {
    let (mut session, msg_rx) = AgentSession::for_test();
    session.is_waiting = false;
    session.last_event_at = Arc::new(AtomicI64::new(last_event_at_ms));
    (session, msg_rx)
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
    let (session, _msg_rx) = make_session(stale_for(limit_ms));
    let cancel = session.agent_cancel.clone();
    sessions.lock().await.insert(thread_id, session);

    let watchdog = ExternalWatchdog::new(
        sessions.clone(),
        bus.clone(),
        pool.clone(),
        limit_ms,
        CEILING_MS,
    );
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

    let limit_ms = 10 * 60 * 1000;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let (session, _msg_rx) = make_session(now_epoch_millis() - 1000);
    let cancel = session.agent_cancel.clone();
    sessions.lock().await.insert(thread_id, session);

    let watchdog = ExternalWatchdog::new(
        sessions.clone(),
        bus.clone(),
        pool.clone(),
        limit_ms,
        CEILING_MS,
    );
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

#[tokio::test]
async fn tick_recovers_exited_stale_session_when_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let (mut session, _msg_rx) = make_session(stale_for(limit_ms));
    session.process_exited = true;
    sessions.lock().await.insert(thread_id, session);

    let watchdog = ExternalWatchdog::new(
        sessions.clone(),
        bus.clone(),
        pool.clone(),
        limit_ms,
        CEILING_MS,
    );
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

#[tokio::test]
async fn tick_leaves_fresh_exited_session_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 10 * 60 * 1000;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let (mut session, _msg_rx) = make_session(now_epoch_millis() - 1000);
    session.process_exited = true;
    sessions.lock().await.insert(thread_id, session);

    let watchdog = ExternalWatchdog::new(
        sessions.clone(),
        bus.clone(),
        pool.clone(),
        limit_ms,
        CEILING_MS,
    );
    watchdog.tick().await;

    assert_eq!(count_continuation_requests(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tick_recovers_hung_tool_past_ceiling_when_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let (session, _msg_rx) = make_session(stale_for(CEILING_MS));
    session
        .tools_in_flight
        .store(3, std::sync::atomic::Ordering::Relaxed);
    sessions.lock().await.insert(thread_id, session);

    let watchdog = ExternalWatchdog::new(
        sessions.clone(),
        bus.clone(),
        pool.clone(),
        limit_ms,
        CEILING_MS,
    );
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

#[tokio::test]
async fn tick_skips_hung_tool_past_ceiling_when_not_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    crate::engine::claude_code::settle_stuck_running_thread(
        &pool,
        &bus,
        thread_id,
        None,
        crate::engine::claude_code::SettleTerminal::StuckProjection,
    )
    .await
    .expect("settle");

    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let (session, _msg_rx) = make_session(stale_for(CEILING_MS));
    session
        .tools_in_flight
        .store(3, std::sync::atomic::Ordering::Relaxed);
    sessions.lock().await.insert(thread_id, session);

    let watchdog = ExternalWatchdog::new(
        sessions.clone(),
        bus.clone(),
        pool.clone(),
        limit_ms,
        CEILING_MS,
    );
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
    let (session, _msg_rx) = make_session(stale_for(limit_ms));
    session
        .tools_in_flight
        .store(2, std::sync::atomic::Ordering::Relaxed);
    sessions.lock().await.insert(thread_id, session);

    let watchdog = ExternalWatchdog::new(
        sessions.clone(),
        bus.clone(),
        pool.clone(),
        limit_ms,
        CEILING_MS,
    );
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
    let (mut session, _msg_rx) = make_session(stale_for(limit_ms));
    session.is_waiting = true;
    sessions.lock().await.insert(thread_id, session);

    let watchdog = ExternalWatchdog::new(
        sessions.clone(),
        bus.clone(),
        pool.clone(),
        limit_ms,
        CEILING_MS,
    );
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
    watchdog.tick().await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tick_flips_external_terminal_emitted_before_dropping_stuck_session() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let (session, _msg_rx) = make_session(stale_for(limit_ms));
    let flag = session.external_terminal_emitted.clone();
    let continuation_flag = session.external_continuation_requested.clone();
    sessions.lock().await.insert(thread_id, session);

    assert!(
        !flag.load(std::sync::atomic::Ordering::Acquire),
        "precondition: flag starts unset"
    );
    assert!(
        !continuation_flag.load(std::sync::atomic::Ordering::Acquire),
        "precondition: continuation flag starts unset"
    );

    let watchdog = ExternalWatchdog::new(
        sessions.clone(),
        bus.clone(),
        pool.clone(),
        limit_ms,
        CEILING_MS,
    );
    watchdog.tick().await;

    assert!(
        flag.load(std::sync::atomic::Ordering::Acquire),
        "tick must set external_terminal_emitted=true to suppress the wedged in-loop's safety-net abort"
    );
    assert!(
        continuation_flag.load(std::sync::atomic::Ordering::Acquire),
        "tick must set external_continuation_requested=true so a conflict-resolution completion hands off instead of aborting"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn recover_stuck_skips_session_that_recovered_since_snapshot() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;

    let snapshot_last_ms = stale_for(limit_ms);
    let (session, _msg_rx) = make_session(snapshot_last_ms);
    let cancel = session.agent_cancel.clone();
    let last_event_at = session.last_event_at.clone();
    let external_terminal = session.external_terminal_emitted.clone();
    let external_continuation = session.external_continuation_requested.clone();
    let idle_notify = session.idle_notify.clone();
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    sessions.lock().await.insert(thread_id, session);

    last_event_at.store(now_epoch_millis(), std::sync::atomic::Ordering::Relaxed);

    let candidate = super::StuckSession {
        thread_id,
        elapsed_ms: 0,
        external_terminal,
        external_continuation,
        idle_notify,
        agent_cancel: cancel.clone(),
        last_event_at,
        snapshot_last_ms,
        needs_running_check: false,
    };

    let watchdog = ExternalWatchdog::new(
        sessions.clone(),
        bus.clone(),
        pool.clone(),
        limit_ms,
        CEILING_MS,
    );
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

async fn backdate_activity(pool: &sqlx::PgPool, thread_id: Uuid, secs: i64) {
    sqlx::query(
        "UPDATE thread_summaries SET last_activity = now() - make_interval(secs => $2) \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .bind(secs)
    .execute(pool)
    .await
    .expect("backdate last_activity");
}

async fn status_of(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_optional(pool)
        .await
        .expect("status query")
}

async fn aborted_count(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'",
    )
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    .expect("aborted count query")
}

async fn emit_agent_output(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentToolResult {
            name: "Bash".into(),
            result: "ok".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            tool_use_id: "tu-out".into(),
            parent_tool_use_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("CodingAgentToolResult emit")
    .expect("CodingAgentToolResult persisted");
}

async fn enqueue_pending(pool: &sqlx::PgPool, thread_id: Uuid) {
    sqlx::query(
        "INSERT INTO thread_queue (id, kind, thread_id, request, status) \
         VALUES ($1, 'coding-agent', $2, '{}'::jsonb, 'queued')",
    )
    .bind(Uuid::new_v4())
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("insert queued entry");
}

const ORPHAN_LIMIT_MS: i64 = super::EXTERNAL_WATCHDOG_LIMIT_MS;
const TWO_HOURS_SECS: i64 = 2 * 60 * 60;

#[test]
fn orphans_without_live_session_drops_only_the_live_ones() {
    let live_id = Uuid::new_v4();
    let orphan_id = Uuid::new_v4();
    let live: std::collections::HashSet<Uuid> = [live_id].into_iter().collect();

    let kept = super::orphans_without_live_session(vec![live_id, orphan_id], &live);

    assert_eq!(
        kept,
        vec![orphan_id],
        "a candidate holding a live `agent_sessions` entry must survive the pass \
         untouched; only the one with no session is an orphan"
    );
}

#[tokio::test]
async fn orphan_pass_spares_a_stalled_successor_even_after_trailing_output() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    for event in [
        serde_json::json!({"type": "SessionHandedOff", "starts": "after-this-turn",
            "opening": "narrative", "message": "Continue the work.", "coding_agent": "codex"}),
        serde_json::json!({"type": "CodingAgentIdled"}),
    ] {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: serde_json::from_value(event).unwrap(),
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }
    emit_agent_output(&bus, thread_id).await;
    backdate_activity(&pool, thread_id, TWO_HOURS_SECS).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    ExternalWatchdog::new(
        sessions,
        bus.clone(),
        pool.clone(),
        ORPHAN_LIMIT_MS,
        CEILING_MS,
    )
    .tick()
    .await;
    assert_eq!(
        status_of(&pool, thread_id).await.as_deref(),
        Some("running")
    );
    assert_eq!(aborted_count(&pool, thread_id).await, 0);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_pass_settles_a_running_thread_with_no_live_session() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    assert_eq!(
        status_of(&pool, thread_id).await.as_deref(),
        Some("running"),
        "the seeded session must leave the projection `running`, or this test \
         proves nothing about the sweep"
    );
    emit_agent_output(&bus, thread_id).await;
    backdate_activity(&pool, thread_id, TWO_HOURS_SECS).await;

    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    ExternalWatchdog::new(
        sessions,
        bus.clone(),
        pool.clone(),
        ORPHAN_LIMIT_MS,
        CEILING_MS,
    )
    .tick()
    .await;

    assert_ne!(
        status_of(&pool, thread_id).await.as_deref(),
        Some("running"),
        "a `running` thread with no live session and no activity for two hours must \
         be settled by the tick, without waiting for Stop or an engine restart"
    );
    assert_eq!(
        aborted_count(&pool, thread_id).await,
        1,
        "settling must emit exactly one ResponseAborted, via the same \
         settle_stuck_running_thread helper Stop and the boot sweep use"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_pass_reads_past_a_side_question_to_the_agents_last_output() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_agent_output(&bus, thread_id).await;
    for event in crate::test_support::every_side_question_event() {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta::NONE,
        })
        .await
        .expect("side-question emit");
    }
    backdate_activity(&pool, thread_id, TWO_HOURS_SECS).await;

    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    ExternalWatchdog::new(
        sessions,
        bus.clone(),
        pool.clone(),
        ORPHAN_LIMIT_MS,
        CEILING_MS,
    )
    .tick()
    .await;

    assert_eq!(
        aborted_count(&pool, thread_id).await,
        1,
        "a side question hid the agent's last output from the orphan pass"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_pass_leaves_a_thread_with_a_live_session_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_agent_output(&bus, thread_id).await;
    backdate_activity(&pool, thread_id, TWO_HOURS_SECS).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let (session, _msg_rx) = make_session(now_epoch_millis() - 1000);
    sessions.lock().await.insert(thread_id, session);

    ExternalWatchdog::new(
        sessions,
        bus.clone(),
        pool.clone(),
        ORPHAN_LIMIT_MS,
        CEILING_MS,
    )
    .tick()
    .await;

    assert_eq!(
        status_of(&pool, thread_id).await.as_deref(),
        Some("running"),
        "a thread whose session is alive must never be settled by the orphan pass, \
         however stale its projection row looks"
    );
    assert_eq!(aborted_count(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_pass_leaves_a_queued_thread_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_agent_output(&bus, thread_id).await;
    backdate_activity(&pool, thread_id, TWO_HOURS_SECS).await;
    enqueue_pending(&pool, thread_id).await;

    ExternalWatchdog::new(
        Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        bus.clone(),
        pool.clone(),
        ORPHAN_LIMIT_MS,
        CEILING_MS,
    )
    .tick()
    .await;

    assert_eq!(
        status_of(&pool, thread_id).await.as_deref(),
        Some("running"),
        "a thread holding a `queued` thread_queue row is waiting for a slot by \
         design and must survive the orphan pass"
    );
    assert_eq!(aborted_count(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_pass_leaves_a_recently_active_thread_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;

    ExternalWatchdog::new(
        Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        bus.clone(),
        pool.clone(),
        ORPHAN_LIMIT_MS,
        CEILING_MS,
    )
    .tick()
    .await;

    assert_eq!(
        status_of(&pool, thread_id).await.as_deref(),
        Some("running"),
        "activity inside the quiet window means the turn is live or still \
         spawning, so the orphan pass must leave it alone"
    );
    assert_eq!(aborted_count(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_pass_leaves_a_question_parked_thread_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            question: "Postgres or SQLite?".into(),
            options: vec![],
            multi_select: false,
            owner_approval: None,
            permission_request: None,
            tool_use_id: "tu-park".into(),
            cc_session_id: "sid-test".into(),
            worktree_path: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("UserQuestionAsked emit")
    .expect("UserQuestionAsked persisted");
    backdate_activity(&pool, thread_id, TWO_HOURS_SECS).await;

    ExternalWatchdog::new(
        Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        bus.clone(),
        pool.clone(),
        ORPHAN_LIMIT_MS,
        CEILING_MS,
    )
    .tick()
    .await;

    assert_eq!(
        status_of(&pool, thread_id).await.as_deref(),
        Some("waiting_for_user_answer"),
        "a question park must leave the projection at `waiting_for_user_answer`; \
         that is what the orphan pass's `status = 'running'` filter relies on"
    );
    assert_eq!(
        aborted_count(&pool, thread_id).await,
        0,
        "a thread parked on an unanswered question survives the engine going \
         quiet; the orphan pass must not kill its card"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_pass_leaves_a_thread_that_never_produced_agent_output_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    backdate_activity(&pool, thread_id, TWO_HOURS_SECS).await;

    ExternalWatchdog::new(
        Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        bus.clone(),
        pool.clone(),
        ORPHAN_LIMIT_MS,
        CEILING_MS,
    )
    .tick()
    .await;

    assert_eq!(
        status_of(&pool, thread_id).await.as_deref(),
        Some("running"),
        "the agent produced no output on this thread, so nothing proves a session \
         ever ran; the orphan pass must leave it for whoever is still holding it"
    );
    assert_eq!(aborted_count(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_pass_settles_only_after_the_agents_own_output_goes_quiet() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_agent_output(&bus, thread_id).await;
    backdate_activity(&pool, thread_id, TWO_HOURS_SECS).await;

    ExternalWatchdog::new(
        Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        bus.clone(),
        pool.clone(),
        ORPHAN_LIMIT_MS,
        CEILING_MS,
    )
    .tick()
    .await;

    assert_eq!(
        aborted_count(&pool, thread_id).await,
        1,
        "an agent that streamed output and then went silent with no session IS \
         the wedge, and must settle"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn stuck_from(thread_id: Uuid, session: &AgentSession) -> super::StuckSession {
    super::StuckSession {
        thread_id,
        elapsed_ms: ORPHAN_LIMIT_MS,
        external_terminal: session.external_terminal_emitted.clone(),
        external_continuation: session.external_continuation_requested.clone(),
        idle_notify: session.idle_notify.clone(),
        agent_cancel: session.agent_cancel.clone(),
        last_event_at: session.last_event_at.clone(),
        snapshot_last_ms: 0,
        needs_running_check: false,
    }
}

#[test]
fn a_replacement_session_is_not_this_candidates_entry() {
    let thread_id = Uuid::new_v4();
    let (snapshotted, _snapshotted_rx) = AgentSession::for_test();
    let candidate = stuck_from(thread_id, &snapshotted);

    let mut sessions = HashMap::new();
    sessions.insert(thread_id, snapshotted);
    assert!(
        super::owns_session_entry(&sessions, &candidate),
        "the entry the candidate was snapshotted from is its own"
    );

    let (replacement, _replacement_rx) = AgentSession::for_test();
    sessions.insert(thread_id, replacement);
    assert!(
        !super::owns_session_entry(&sessions, &candidate),
        "a replacement registered since the snapshot is not this candidate's session"
    );

    sessions.remove(&thread_id);
    assert!(
        !super::owns_session_entry(&sessions, &candidate),
        "an entry another pass already reaped is not ours either"
    );
}

#[tokio::test]
async fn recover_stuck_cancels_the_wedged_subprocess_but_spares_a_replacement() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let limit_ms = 50;
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;

    let snapshot_last_ms = stale_for(limit_ms);
    let (wedged, _wedged_rx) = make_session(snapshot_last_ms);
    let cancel = wedged.agent_cancel.clone();
    let external_terminal = wedged.external_terminal_emitted.clone();
    let external_continuation = wedged.external_continuation_requested.clone();
    let candidate = super::StuckSession {
        thread_id,
        elapsed_ms: 0,
        external_terminal: external_terminal.clone(),
        external_continuation: external_continuation.clone(),
        idle_notify: wedged.idle_notify.clone(),
        agent_cancel: cancel.clone(),
        last_event_at: wedged.last_event_at.clone(),
        snapshot_last_ms,
        needs_running_check: false,
    };

    let (replacement, _replacement_rx) = make_session(snapshot_last_ms);
    let replacement_last_event = replacement.last_event_at.clone();
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    sessions.lock().await.insert(thread_id, replacement);

    let watchdog = ExternalWatchdog::new(
        sessions.clone(),
        bus.clone(),
        pool.clone(),
        limit_ms,
        CEILING_MS,
    );
    watchdog.recover_stuck(vec![candidate]).await;

    assert!(
        cancel.is_cancelled(),
        "the wedged subprocess must still be killed: its entry is gone, so no \
         later tick, Stop or shutdown sweep can reach it"
    );
    assert!(
        external_terminal.load(std::sync::atomic::Ordering::Acquire),
        "suppress the wedged loop's own terminal, which would land on the \
         replacement's live turn"
    );
    assert!(
        !external_continuation.load(std::sync::atomic::Ordering::Acquire),
        "the continuation flag promises a ContinuationRequested this path does \
         not emit, so it must stay unset"
    );

    let guard = sessions.lock().await;
    let entry = guard
        .get(&thread_id)
        .expect("replacement entry must survive");
    assert!(
        Arc::ptr_eq(&entry.last_event_at, &replacement_last_event),
        "the surviving entry must be the replacement's, not a rebuilt one"
    );
    drop(guard);

    assert_eq!(
        count_continuation_requests(&pool, thread_id).await,
        0,
        "a replacement is already running this thread: a second --resume would \
         put two agents on one worktree"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_pass_spares_a_session_that_registered_after_the_snapshot() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_agent_output(&bus, thread_id).await;
    backdate_activity(&pool, thread_id, TWO_HOURS_SECS).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let (session, _msg_rx) = make_session(now_epoch_millis() - 1000);
    sessions.lock().await.insert(thread_id, session);

    let empty_snapshot = std::collections::HashSet::new();
    ExternalWatchdog::new(
        sessions,
        bus.clone(),
        pool.clone(),
        ORPHAN_LIMIT_MS,
        CEILING_MS,
    )
    .settle_orphaned_running(&empty_snapshot)
    .await;

    assert_eq!(
        status_of(&pool, thread_id).await.as_deref(),
        Some("running"),
        "a session that registered after the opening snapshot is live, not an orphan"
    );
    assert_eq!(aborted_count(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
