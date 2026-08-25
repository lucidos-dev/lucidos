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

/// Mid-turn (`is_waiting = false`) session with a given heartbeat. Returns the
/// receiver so the caller can keep the session live — dropping it makes the
/// session read as a phantom, which the watchdog treats as `loop_ended`.
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

    let limit_ms = 10 * 60 * 1000; // 10 min
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    // Last event 1 s ago — far inside 10-min limit.
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
    let (mut session, _msg_rx) = make_session(now_epoch_millis() - 1000); // 1 s ago
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
    let (session, _msg_rx) = make_session(stale_for(CEILING_MS)); // past the ceiling
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
    watchdog.tick().await; // must not panic / hang

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Stuck session's `external_terminal_emitted` flag MUST flip to true.
/// This is the suppression the wedged in-loop's safety net checks at
/// `runtime_helpers.rs::external_terminal_already_emitted`. If we forgot to
/// flip it, the wedged loop (when it eventually wakes) would emit a
/// duplicate `ResponseAborted` on top of our `ContinuationRequested` — the user
/// would see both an auto-resume AND an "Aborted" terminal.
///
/// `external_continuation_requested` MUST flip alongside it: it is what tells
/// a conflict-resolution session's completion that this Skip is a RECOVERY
/// (hand the merge duty off) rather than a restart abort / concurrent cancel
/// (abort the merge). Without it the wedged loop's cleanup would emit
/// `ChangeApplyFailed`, close the duty pairing, and tear down the merge
/// worktree underneath the continuation this tick just dispatched.
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
    let (session, _msg_rx) = make_session(snapshot_last_ms);
    let cancel = session.agent_cancel.clone();
    let last_event_at = session.last_event_at.clone();
    let external_terminal = session.external_terminal_emitted.clone();
    let external_continuation = session.external_continuation_requested.clone();
    let idle_notify = session.idle_notify.clone();
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    sessions.lock().await.insert(thread_id, session);

    // …but a fresh event arrives before the mutate pass — the session recovered.
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

// -- Orphan reconciliation: `running` in the projection, no live session -------
//
// The scan above walks `agent_sessions`, so a thread that fell OUT of that map
// is invisible to it. `settle_orphaned_running` asks the converse question.
// Every case below pins one of its four exclusions, because a wrong settle
// aborts work rather than merely delaying a cleanup.

/// Push a thread's `last_activity` back, using the DATABASE clock. Computing
/// the instant host-side would reintroduce the two-clock bug the sweep's own
/// SQL exists to avoid (ADR 0053).
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

/// Emit one agent-output event, so the thread carries the orphan pass's
/// positive fingerprint. Without it, a thread's newest event is its
/// `SessionStarted`, which reads as "never started producing". The thread is
/// then spared for a reason unrelated to whichever guard the caller is testing.
async fn emit_agent_output(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentToolResult {
            name: "Bash".into(),
            result: "ok".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            tool_use_id: "tu-out".into(),
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

/// A CC thread the projection shows `running`, silent for two hours. The limit
/// is left at the real 12 minutes so the backdating, not a tiny limit, is what
/// puts it outside the window.
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

    // Empty map: the subprocess died without a terminal, exactly the wedge.
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
async fn orphan_pass_leaves_a_thread_with_a_live_session_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    // Old in the projection and carrying the fingerprint, but the session is
    // alive and its heartbeat is fresh, so the stuck scan skips it too. Only
    // the live-set exclusion can save it here.
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
    // Waiting for a capacity slot is not a wedge. A queue can legitimately hold
    // an entry far longer than the limit, and settling it orphans real work.
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
    // No backdating: this is a thread mid-spawn, whose session has not
    // registered yet. On a cold worktree that gap is seconds to minutes.

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
    // No `emit_agent_output` here on purpose: the park must be the newest
    // event, which is the state the status filter is being tested against.

    ExternalWatchdog::new(
        Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        bus.clone(),
        pool.clone(),
        ORPHAN_LIMIT_MS,
        CEILING_MS,
    )
    .tick()
    .await;

    // Assert the REASON, not just the outcome. The status filter is the whole
    // guard here. A park that stopped carrying `waiting_for_user_answer` would
    // start being settled, and an outcome-only test would still pass on its
    // way there.
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

    // A thread waiting for a capacity slot looks EXACTLY like the wedge on
    // every negative test: `running`, no live session, `last_activity` as old
    // as the wait. Both queues can hold one this long, and only one of them is
    // the `thread_queue` table the guard above checks. The user-slot pool is
    // in memory, so no SQL exclusion can see it.
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

    // The same thread as the test above, one agent-output event later. That
    // event is the whole difference between "waiting" and "orphaned", so pin
    // the pair: without it, the settle test could pass on the strength of the
    // quiet window alone.
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
