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

use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::wait_for_cc_session_alive;
use crate::engine::types::{AgentSession, AgentUserInput};
use crate::engine::{InjectedPrompt, ThreadHandle};

fn make_thread_handle() -> ThreadHandle {
    let (injection_tx, _injection_rx) = mpsc::unbounded_channel::<InjectedPrompt>();
    ThreadHandle {
        token: CancellationToken::new(),
        injection_tx,
        generation: 0,
        cancel_actor: Arc::new(std::sync::Mutex::new(None)),
    }
}

fn make_test_session(process_exited: bool) -> AgentSession {
    let (msg_tx, _msg_rx) = mpsc::unbounded_channel::<AgentUserInput>();
    let (control_tx, _control_rx) = mpsc::unbounded_channel();
    AgentSession {
        msg_tx,
        is_waiting: !process_exited,
        has_changes: false,
        requires_restart: false,
        pending_stop: None,
        cancel_actor: None,
        redirect_followup: false,
        stop: Arc::new(Notify::new()),
        interrupt: Arc::new(Notify::new()),
        idle_notify: Arc::new(Notify::new()),
        apply_now_in_progress: false,
        process_exited,
        worktree_path: None,
        branch_name: None,
        repo_root: None,
        cc_session_id: None,
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        external_terminal_emitted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        control_tx,
        builtin_commands: vec![],
        skill_commands: vec![],
        current_model: None,
        current_reasoning_effort: None,
        last_event_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        pending_followups: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        question_resume_pending: false,
        tools_in_flight: Arc::new(std::sync::atomic::AtomicI32::new(0)),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
    }
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

    // Simulate CC registration after 100ms — well under the 2s deadline.
    let agent_sessions_clone = agent_sessions.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        agent_sessions_clone
            .lock()
            .await
            .insert(thread_id, make_test_session(false));
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
    agent_sessions
        .lock()
        .await
        .insert(thread_id, make_test_session(true));

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

    agent_sessions
        .lock()
        .await
        .insert(thread_id, make_test_session(false));

    let started = std::time::Instant::now();
    let result = wait_for_cc_session_alive(
        &agent_sessions,
        &active_threads,
        thread_id,
        Duration::from_secs(2),
        Duration::from_millis(20),
    )
    .await;

    assert!(result, "expected immediate true for an already-alive session");
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "expected sub-poll-interval return for fast path"
    );
}

// --- should_redirect_codex_followup ---------------------------------------

use super::should_redirect_codex_followup;
use crate::runtime::CodingAgent;

#[test]
fn redirect_fires_for_codex_user_followup_mid_turn() {
    // The reported bug: a Codex turn in flight + a genuine user follow-up must
    // interrupt-and-redirect rather than queue invisibly behind the long turn.
    assert!(should_redirect_codex_followup(CodingAgent::Codex, true, true));
}

#[test]
fn redirect_skips_codex_when_idle() {
    // Idle-but-alive Codex (not in flight): a follow-up routes via turn/start
    // immediately, no interrupt.
    assert!(!should_redirect_codex_followup(
        CodingAgent::Codex,
        false,
        true
    ));
}

#[test]
fn redirect_skips_codex_child_wake() {
    // A child-wake (not a user message) resumes a waiting thread and must never
    // interrupt a turn, even mid-flight.
    assert!(!should_redirect_codex_followup(
        CodingAgent::Codex,
        true,
        false
    ));
}

#[test]
fn redirect_never_fires_for_claude_code() {
    // CC steers via stdin — a mid-turn follow-up is forwarded as-is, never
    // interrupted. Guards the "CC behavior unchanged" invariant.
    assert!(!should_redirect_codex_followup(
        CodingAgent::ClaudeCode,
        true,
        true
    ));
}

// --- arm_codex_redirect ----------------------------------------------------

use super::arm_codex_redirect;

/// A live Codex session mid-turn: alive (`process_exited=false`), not at a turn
/// boundary (`is_waiting=false`), with `pending_followups=1` as a normal turn
/// has after session creation (the initial turn pre-counts its own Result).
fn codex_in_flight_session() -> AgentSession {
    let mut s = make_test_session(false);
    s.is_waiting = false; // turn in flight
    s.coding_agent = CodingAgent::Codex;
    s.pending_followups
        .store(1, std::sync::atomic::Ordering::Release);
    s
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arm_redirect_fires_for_codex_mid_turn_user_followup() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let session = codex_in_flight_session();
    let interrupt = session.interrupt.clone();
    let pending = session.pending_followups.clone();
    sessions.insert(thread_id, session);

    let idle = arm_codex_redirect(&mut sessions, thread_id, true, &None);

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
    // follow-up. arm_codex_redirect must bump it to >= 2 so the interrupted
    // warm-up turn's idle keeps the subprocess alive.
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let session = codex_in_flight_session();
    session
        .pending_followups
        .store(0, std::sync::atomic::Ordering::Release);
    let pending = session.pending_followups.clone();
    sessions.insert(thread_id, session);

    assert!(arm_codex_redirect(&mut sessions, thread_id, true, &None).is_some());
    assert_eq!(
        pending.load(std::sync::atomic::Ordering::Acquire),
        2,
        "a warm-up turn (count 0) must be bumped to 2, not 1, to survive the interrupt idle"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arm_redirect_skips_claude_code_mid_turn() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let mut session = make_test_session(false);
    session.is_waiting = false; // CC turn in flight
    debug_assert_eq!(session.coding_agent, CodingAgent::ClaudeCode);
    let interrupt = session.interrupt.clone();
    let pending = session.pending_followups.clone();
    sessions.insert(thread_id, session);

    let idle = arm_codex_redirect(&mut sessions, thread_id, true, &None);

    assert!(idle.is_none(), "CC must never be interrupted on a follow-up");
    assert!(
        !sessions.get(&thread_id).unwrap().redirect_followup,
        "CC must not be flagged for redirect — it steers via stdin, never cancels"
    );
    assert_eq!(pending.load(std::sync::atomic::Ordering::Acquire), 0);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), interrupt.notified())
            .await
            .is_err(),
        "no interrupt should have been fired for CC"
    );
}

#[test]
fn arm_redirect_skips_idle_codex() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let mut session = make_test_session(false); // is_waiting=true → idle, not in flight
    session.coding_agent = CodingAgent::Codex;
    let pending = session.pending_followups.clone();
    sessions.insert(thread_id, session);

    // Idle Codex: route via turn/start immediately, no interrupt.
    assert!(arm_codex_redirect(&mut sessions, thread_id, true, &None).is_none());
    assert_eq!(pending.load(std::sync::atomic::Ordering::Acquire), 0);
}

#[test]
fn arm_redirect_skips_child_wake() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    sessions.insert(thread_id, codex_in_flight_session());

    // is_user_message=false (child-wake) must never interrupt a live turn.
    assert!(arm_codex_redirect(&mut sessions, thread_id, false, &None).is_none());
}

#[test]
fn arm_redirect_none_when_no_session() {
    let mut sessions = HashMap::new();
    assert!(arm_codex_redirect(&mut sessions, Uuid::new_v4(), true, &None).is_none());
}
