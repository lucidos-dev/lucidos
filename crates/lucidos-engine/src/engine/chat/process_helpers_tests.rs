//! Tests for `wait_for_cc_session_alive` — the race-bridge between a chat
//! handler that's holding `active_threads[thread_id]` mid-spawn and a
//! follow-up POST that needs `agent_sessions[thread_id]` populated to route
//! via msg_tx.
//!
//! Engine log signature of the bug this guards (thread `7917e84b…`):
//!   POST 1 at 08:33:18 starts CC (worktree setup ~4s).
//!   POST 2 at 08:33:21 sees `agent_sessions` empty (CC not yet registered),
//!     falls to slow path, blocks 60s in `register_thread_queued`, then
//!     force-evicts the still-spawning CC at 08:34:21 →
//!     `ResponseAborted(cause=safety_net)` on a turn the user never canceled.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::wait_for_cc_session_alive;
use crate::engine::types::{AgentSession, AgentUserInput};
use crate::engine::{InjectedPrompt, ThreadHandle};

fn make_thread_handle() -> ThreadHandle {
    let (injection_tx, _injection_rx) = mpsc::unbounded_channel::<InjectedPrompt>();
    ThreadHandle::new(CancellationToken::new(), injection_tx, 0)
}

/// Returns the receiver alongside the session: hold it (`let (s, _rx) = …`) for
/// the test's lifetime, or the session reads as a phantom and the race-bridge
/// correctly refuses to treat it as live. See `AgentSession::is_live`.
fn make_test_session(
    process_exited: bool,
) -> (AgentSession, mpsc::UnboundedReceiver<AgentUserInput>) {
    let (mut session, msg_rx) = AgentSession::for_test();
    session.is_waiting = !process_exited;
    session.process_exited = process_exited;
    (session, msg_rx)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn returns_true_when_session_populates_within_deadline() {
    let agent_sessions = Arc::new(tokio::sync::Mutex::new(HashMap::<Uuid, AgentSession>::new()));
    let active_threads = Arc::new(std::sync::Mutex::new(HashMap::<Uuid, ThreadHandle>::new()));
    let thread_id = Uuid::new_v4();

    // Chat handler 1 has registered active_threads but not yet agent_sessions
    // (Claude Code subprocess still spawning).
    active_threads
        .lock()
        .unwrap()
        .insert(thread_id, make_thread_handle());

    // Simulate CC registration after 100ms — well under the 2s deadline. The
    // receiver stays in the test body (not the spawned task) so the session is
    // live once registered; a dropped receiver would make it a phantom, which
    // the bridge correctly skips.
    let (session, _msg_rx) = make_test_session(false);
    let agent_sessions_clone = agent_sessions.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        agent_sessions_clone.lock().await.insert(thread_id, session);
    });

    let started = std::time::Instant::now();
    let result = wait_for_cc_session_alive(
        &agent_sessions,
        &active_threads,
        thread_id,
        Duration::from_secs(2),
        Duration::from_millis(20),
    )
    .await;

    assert!(
        result,
        "expected race-bridge to detect agent_sessions population"
    );
    // Should not have waited the full deadline — the polling picked it up
    // shortly after the spawn completed.
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "race-bridge took {:?}, expected to detect within ~100-200ms",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn returns_false_when_chat_handler_clears_active_threads_first() {
    let agent_sessions = Arc::new(tokio::sync::Mutex::new(HashMap::<Uuid, AgentSession>::new()));
    let active_threads = Arc::new(std::sync::Mutex::new(HashMap::<Uuid, ThreadHandle>::new()));
    let thread_id = Uuid::new_v4();

    active_threads
        .lock()
        .unwrap()
        .insert(thread_id, make_thread_handle());

    // Chat handler bails (e.g. worktree creation failed) before populating
    // agent_sessions — clears its active_threads slot. The follow-up should
    // stop waiting and fall through to a fresh slow-path spawn.
    let active_threads_clone = active_threads.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        active_threads_clone.lock().unwrap().remove(&thread_id);
    });

    let result = wait_for_cc_session_alive(
        &agent_sessions,
        &active_threads,
        thread_id,
        Duration::from_secs(2),
        Duration::from_millis(20),
    )
    .await;

    assert!(
        !result,
        "expected race-bridge to bail when chat handler clears active_threads"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn returns_false_when_deadline_elapses_with_no_population() {
    let agent_sessions = Arc::new(tokio::sync::Mutex::new(HashMap::<Uuid, AgentSession>::new()));
    let active_threads = Arc::new(std::sync::Mutex::new(HashMap::<Uuid, ThreadHandle>::new()));
    let thread_id = Uuid::new_v4();

    // Chat handler is registered but hangs forever without populating
    // agent_sessions (degenerate case: Claude Code subprocess takes longer than the
    // deadline). The follow-up returns false so the caller falls through
    // to the slow path.
    active_threads
        .lock()
        .unwrap()
        .insert(thread_id, make_thread_handle());

    let started = std::time::Instant::now();
    let result = wait_for_cc_session_alive(
        &agent_sessions,
        &active_threads,
        thread_id,
        Duration::from_millis(150),
        Duration::from_millis(20),
    )
    .await;

    assert!(!result, "expected race-bridge to time out");
    assert!(
        started.elapsed() >= Duration::from_millis(150),
        "expected to wait at least the deadline before bailing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn returns_false_when_existing_session_is_process_exited() {
    let agent_sessions = Arc::new(tokio::sync::Mutex::new(HashMap::<Uuid, AgentSession>::new()));
    let active_threads = Arc::new(std::sync::Mutex::new(HashMap::<Uuid, ThreadHandle>::new()));
    let thread_id = Uuid::new_v4();

    // A dead session sitting in the map (process_exited=true) is not a valid
    // msg_tx target — the bridge must keep waiting for a fresh registration
    // (or the chat handler bailing). Mirror the real engine's "dead session
    // stays in the map until insert() replaces it" semantic.
    active_threads
        .lock()
        .unwrap()
        .insert(thread_id, make_thread_handle());
    let (session, _msg_rx) = make_test_session(true);
    agent_sessions.lock().await.insert(thread_id, session);

    let result = wait_for_cc_session_alive(
        &agent_sessions,
        &active_threads,
        thread_id,
        Duration::from_millis(120),
        Duration::from_millis(20),
    )
    .await;

    assert!(
        !result,
        "expected race-bridge to ignore a process_exited session"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn returns_true_immediately_when_session_already_alive() {
    let agent_sessions = Arc::new(tokio::sync::Mutex::new(HashMap::<Uuid, AgentSession>::new()));
    let active_threads = Arc::new(std::sync::Mutex::new(HashMap::<Uuid, ThreadHandle>::new()));
    let thread_id = Uuid::new_v4();

    let (session, _msg_rx) = make_test_session(false);
    agent_sessions.lock().await.insert(thread_id, session);

    let started = std::time::Instant::now();
    let result = wait_for_cc_session_alive(
        &agent_sessions,
        &active_threads,
        thread_id,
        Duration::from_secs(2),
        Duration::from_millis(20),
    )
    .await;

    assert!(
        result,
        "expected immediate true for an already-alive session"
    );
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "expected sub-poll-interval return for fast path"
    );
}

// --- should_redirect_followup ---------------------------------------------

use super::should_redirect_followup;
use crate::runtime::CodingAgent;

#[test]
fn redirect_fires_for_codex_user_followup_mid_turn() {
    // A Codex turn in flight + a genuine user follow-up must interrupt-and-
    // redirect rather than queue invisibly behind the long turn. Codex does
    // this whether or not the caller marked the follow-up urgent, because a
    // Codex turn cannot see a queued message at all until it ends.
    assert!(should_redirect_followup(
        CodingAgent::Codex,
        true,
        true,
        false
    ));
    assert!(should_redirect_followup(
        CodingAgent::Codex,
        true,
        true,
        true
    ));
}

#[test]
fn redirect_skips_codex_when_idle() {
    // Idle-but-alive Codex (not in flight): a follow-up routes via turn/start
    // immediately, no interrupt. Urgency does not manufacture a turn to stop.
    assert!(!should_redirect_followup(
        CodingAgent::Codex,
        false,
        true,
        false
    ));
    assert!(!should_redirect_followup(
        CodingAgent::Codex,
        false,
        true,
        true
    ));
}

#[test]
fn redirect_skips_codex_child_wake() {
    // A child-wake (not a user message) resumes a waiting thread and must never
    // interrupt a turn, even mid-flight and even marked urgent. The flag rides
    // on the follow-up, and a wake is not one.
    assert!(!should_redirect_followup(
        CodingAgent::Codex,
        true,
        false,
        false
    ));
    assert!(!should_redirect_followup(
        CodingAgent::Codex,
        true,
        false,
        true
    ));
}

#[test]
fn redirect_skips_claude_code_unless_urgent() {
    // The opt-in half. A plain CC follow-up is forwarded as-is and queues, so a
    // benign steer never throws away an in-flight build. This is the invariant
    // `chat.spec.ts` ("mid-turn user message reaches CC during active tool
    // calls") exercises end to end.
    assert!(!should_redirect_followup(
        CodingAgent::ClaudeCode,
        true,
        true,
        false
    ));
}

#[test]
fn redirect_fires_for_urgent_claude_code_followup_mid_turn() {
    // The 2026-08-06 incident: a CC child parked in
    // `TaskOutput(block: true, timeout: 600000)` did not read a STOP for nine
    // and a half minutes, because CC surfaces a queued stdin message only at
    // the next assistant turn boundary and there was not one. An urgent
    // follow-up interrupts instead of waiting for the tool's own timeout.
    assert!(should_redirect_followup(
        CodingAgent::ClaudeCode,
        true,
        true,
        true
    ));
}

#[test]
fn redirect_skips_idle_claude_code_even_when_urgent() {
    // Nothing to interrupt: an idle CC session picks the message up from stdin
    // at once. Interrupting would cancel a turn that does not exist.
    assert!(!should_redirect_followup(
        CodingAgent::ClaudeCode,
        false,
        true,
        true
    ));
}

#[test]
fn redirect_skips_urgent_claude_code_child_wake() {
    // Urgency cannot promote an engine-internal child-wake into an interrupt.
    assert!(!should_redirect_followup(
        CodingAgent::ClaudeCode,
        true,
        false,
        true
    ));
}

// --- arm_followup_redirect -------------------------------------------------

use super::arm_followup_redirect;

/// A live Codex session mid-turn: alive (`process_exited=false`), not at a turn
/// boundary (`is_waiting=false`), with `pending_followups=1` as a normal turn
/// has after session creation (the initial turn pre-counts its own Result).
fn codex_in_flight_session() -> (AgentSession, mpsc::UnboundedReceiver<AgentUserInput>) {
    let (mut s, msg_rx) = make_test_session(false);
    s.is_waiting = false; // turn in flight
    s.coding_agent = CodingAgent::Codex;
    s.pending_followups
        .store(1, std::sync::atomic::Ordering::Release);
    (s, msg_rx)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arm_redirect_fires_for_codex_mid_turn_user_followup() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let (session, _msg_rx) = codex_in_flight_session();
    let interrupt = session.interrupt.clone();
    let pending = session.pending_followups.clone();
    sessions.insert(thread_id, session);

    let idle = arm_followup_redirect(&mut sessions, thread_id, true, false, &None);

    assert!(
        idle.is_some(),
        "a Codex mid-turn user follow-up must arm the redirect and return idle_notify"
    );
    assert!(
        sessions.get(&thread_id).unwrap().redirect_followup,
        "arming the redirect must flag the session so the interrupt arm classifies \
         the interrupted turn as SupersededByFollowup (neutral), not UserStop"
    );
    assert_eq!(
        pending.load(std::sync::atomic::Ordering::Acquire),
        2,
        "a normal in-flight turn (count 1) plus the follow-up reaches 2 so the interrupted turn's idle keeps the subprocess alive (terminate_decision needs > 1)"
    );
    // The interrupt fired: notify_one stores a permit, so a notified() created
    // AFTER the call still resolves immediately.
    assert!(
        tokio::time::timeout(Duration::from_millis(100), interrupt.notified())
            .await
            .is_ok(),
        "the live turn's interrupt must have been fired"
    );
}

#[test]
fn arm_redirect_keeps_warmup_turn_alive() {
    // A silent-resume / warm-up turn starts at pending_followups=0. A lone +1
    // would land at 1 → terminate_decision → Terminate, killing the queued
    // follow-up. arm_followup_redirect must bump it to >= 2 so the interrupted
    // warm-up turn's idle keeps the subprocess alive.
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let (session, _msg_rx) = codex_in_flight_session();
    session
        .pending_followups
        .store(0, std::sync::atomic::Ordering::Release);
    let pending = session.pending_followups.clone();
    sessions.insert(thread_id, session);

    assert!(arm_followup_redirect(&mut sessions, thread_id, true, false, &None).is_some());
    assert_eq!(
        pending.load(std::sync::atomic::Ordering::Acquire),
        2,
        "a warm-up turn (count 0) must be bumped to 2, not 1, to survive the interrupt idle"
    );
}

/// A live Claude Code session mid-turn, the shape the 2026-08-06 incident had:
/// alive, not at a turn boundary, with `pending_followups=1` from its own turn.
fn claude_code_in_flight_session() -> (AgentSession, mpsc::UnboundedReceiver<AgentUserInput>) {
    let (mut s, msg_rx) = make_test_session(false);
    s.is_waiting = false; // turn in flight
    debug_assert_eq!(s.coding_agent, CodingAgent::ClaudeCode);
    s.pending_followups
        .store(1, std::sync::atomic::Ordering::Release);
    (s, msg_rx)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arm_redirect_skips_plain_claude_code_mid_turn() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let (session, _msg_rx) = claude_code_in_flight_session();
    let interrupt = session.interrupt.clone();
    let pending = session.pending_followups.clone();
    sessions.insert(thread_id, session);

    let idle = arm_followup_redirect(&mut sessions, thread_id, true, false, &None);

    assert!(
        idle.is_none(),
        "a plain (non-urgent) CC follow-up must not be interrupted: it steers via stdin"
    );
    assert!(
        !sessions.get(&thread_id).unwrap().redirect_followup,
        "a plain CC follow-up must not be flagged for redirect"
    );
    assert_eq!(
        pending.load(std::sync::atomic::Ordering::Acquire),
        1,
        "no interrupt means no pre-count: the caller's own msg_tx send does the counting"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), interrupt.notified())
            .await
            .is_err(),
        "no interrupt should have been fired for a plain CC follow-up"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arm_redirect_fires_for_urgent_claude_code_mid_turn() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let (session, _msg_rx) = claude_code_in_flight_session();
    let interrupt = session.interrupt.clone();
    let pending = session.pending_followups.clone();
    sessions.insert(thread_id, session);

    let idle = arm_followup_redirect(&mut sessions, thread_id, true, true, &None);

    assert!(
        idle.is_some(),
        "an urgent CC mid-turn follow-up must arm the redirect and return idle_notify"
    );
    assert!(
        sessions.get(&thread_id).unwrap().redirect_followup,
        "the interrupted turn must classify as SupersededByFollowup (neutral), not UserStop"
    );
    assert_eq!(
        pending.load(std::sync::atomic::Ordering::Acquire),
        2,
        "the interrupted turn's idle must keep the subprocess alive (terminate_decision needs > 1)"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), interrupt.notified())
            .await
            .is_ok(),
        "the live CC turn's interrupt must have been fired"
    );
}

#[test]
fn arm_redirect_skips_idle_claude_code_even_when_urgent() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    // make_test_session(false) leaves is_waiting=true, i.e. idle but alive.
    let (session, _msg_rx) = make_test_session(false);
    let pending = session.pending_followups.clone();
    sessions.insert(thread_id, session);

    assert!(
        arm_followup_redirect(&mut sessions, thread_id, true, true, &None).is_none(),
        "an idle CC session reads stdin at once: there is no turn to interrupt"
    );
    assert_eq!(pending.load(std::sync::atomic::Ordering::Acquire), 0);
}

#[test]
fn arm_redirect_skips_idle_codex() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let (mut session, _msg_rx) = make_test_session(false); // is_waiting=true → idle, not in flight
    session.coding_agent = CodingAgent::Codex;
    let pending = session.pending_followups.clone();
    sessions.insert(thread_id, session);

    // Idle Codex: route via turn/start immediately, no interrupt.
    assert!(arm_followup_redirect(&mut sessions, thread_id, true, false, &None).is_none());
    assert_eq!(pending.load(std::sync::atomic::Ordering::Acquire), 0);
}

#[test]
fn arm_redirect_skips_child_wake() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let (session, _msg_rx) = codex_in_flight_session();
    sessions.insert(thread_id, session);

    // is_user_message=false (child-wake) must never interrupt a live turn.
    assert!(arm_followup_redirect(&mut sessions, thread_id, false, false, &None).is_none());
}

#[test]
fn arm_redirect_none_when_no_session() {
    let mut sessions = HashMap::new();
    assert!(arm_followup_redirect(&mut sessions, Uuid::new_v4(), true, false, &None).is_none());
}
