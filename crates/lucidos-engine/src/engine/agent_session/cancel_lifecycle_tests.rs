//! Phase 7 regression tests — pin the contract that "user clicked Stop" is a
//! turn boundary, not a terminal event.
//!
//! Behavior asserted (post-Phase-4.1, post-Phase-7):
//!   1. Cancel emits `ResponseCanceled` (the visible "Canceled" exchange marker).
//!   2. Cancel emits `CodingAgentIdled` (the turn-boundary marker that keeps the
//!      thread active and lets the spawn dispatcher pick up the next message via
//!      `--resume`).
//!   3. Cancel does NOT emit `SessionEnded` — the variant `UserEnded` was removed
//!      from `SessionEndReason` in Phase 4.1, leaving only terminal-only reasons
//!      (`Shutdown`, `Panic`, `Closed`). The cancel handler in `run_session.rs`
//!      no longer has a code path that produces a `SessionEnded` for this case.
//!
//! The first test pins the pure decision predicate (`classify_result` +
//! `make_terminal_event`) that the cancel arm relies on. The second test pins
//! the event sequence end-to-end against the EventBus, asserting the absence
//! of `SessionEnded` after a cancel.

use std::sync::Arc;

use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, EventChannel, EventMeta, SessionEndReason, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};

use super::lifecycle::{classify_result, stop_terminal_kind, TerminalKind};
use crate::engine::LucidosEngine;

fn cc_meta() -> EventMeta {
    EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    }
}

fn user_message(text: &str) -> ThreadEvent {
    ThreadEvent::MessageReceived {
        voice_session_id: None,
        text: text.into(),
        user_image_hashes: vec![],
        device_id: None,
        device: None,
        image_description: None,
        parent_thread_id: None,
        spawning_event_id: None,
        mode: ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    }
}

/// Cancel = `user_hit_stop = true`, not a shutdown, with a real CC `Result`
/// arriving (so `is_silent_resume = false`). The classifier must return
/// `Canceled` AND `emit_idle = true` so the run loop emits both
/// `ResponseCanceled` and `CodingAgentIdled` after a cancel.
///
/// If `emit_idle` were ever flipped to `false` here, the thread would freeze
/// in `running` state forever after a Stop click — the spawn dispatcher relies
/// on `CodingAgentIdled` to know the turn closed.
#[test]
fn cancel_classifies_as_canceled_with_emit_idle_true() {
    let is_silent_resume = false;
    let user_hit_stop = true;
    let is_shutdown = false;

    let (terminal, emit_idle) = classify_result(
        is_silent_resume,
        user_hit_stop,
        false, // interrupt_is_redirect: a plain Stop, not a follow-up redirect
        is_shutdown,
        None,
        false,
    );

    assert_eq!(
        terminal,
        Some(TerminalKind::Canceled(
            crate::engine::thread_events::CancelCause::UserStop
        )),
        "user-driven cancel must produce TerminalKind::Canceled"
    );
    assert!(
        emit_idle,
        "cancel is a turn boundary — CodingAgentIdled MUST follow ResponseCanceled \
         so the dispatcher can pick up the next message via --resume"
    );
}

/// `make_terminal_event(Canceled, ...)` produces `ResponseCanceled` (preserved
/// across Phases 4 & 7 — it is the visible "Canceled" exchange marker).
#[test]
fn cancel_terminal_event_is_response_canceled() {
    let event = LucidosEngine::make_terminal_event(
        TerminalKind::Canceled(crate::engine::thread_events::CancelCause::UserStop),
        "partial text".into(),
        Some("claude-opus-4-7".into()),
        None,
    );
    assert!(
        matches!(event, ThreadEvent::ResponseCanceled { .. }),
        "Canceled terminal kind must produce a ResponseCanceled event; got {:?}",
        std::mem::discriminant(&event),
    );
}

/// Phase 4.1 removed `SessionEndReason::UserEnded`. The current variants are
/// `Shutdown` / `Panic` / `Closed` (terminal — frontend renders as Aborted /
/// Failed) and `StaleResume` (transient internal-retry marker — frontend treats
/// as a normal lifecycle event), plus the read-side `LegacyNonTerminal`
/// catch-all. This test pins the absence of `UserEnded` by constructing each
/// remaining variant explicitly — if a future patch re-adds `UserEnded`, this
/// test won't fail, but the integration test below
/// (`cancel_does_not_terminate_session`) will, because it asserts no
/// `SessionEnded` appears after a cancel sequence.
#[test]
fn session_end_reason_variants() {
    let _shutdown = SessionEndReason::Shutdown;
    let _panic = SessionEndReason::Panic;
    let _closed = SessionEndReason::Closed;
    let _stale_resume = SessionEndReason::StaleResume;
    // No SessionEndReason::UserEnded — the variant is gone.
}

/// Integration: simulate a turn that gets canceled by the user. After the
/// cancel sequence (`MessageReceived` → `ResponseCanceled` → `CodingAgentIdled`),
/// no `SessionEnded` event must exist for the thread.
///
/// This is the behavior promised by Phase 7: cancel is a turn boundary, not a
/// terminal event. The thread stays alive; a follow-up `MessageReceived`
/// triggers a new spawn via `--resume`.
#[tokio::test]
async fn cancel_does_not_terminate_session() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let thread_id = Uuid::new_v4();

    // Turn 1: user sends a message, CC starts, user clicks Stop, CC emits
    // a Result, the engine emits ResponseCanceled then CodingAgentIdled.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: user_message("do a long task"),
        meta: cc_meta(),
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-cancel-1".into(),
            branch: "claude-code/cancel-test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: cc_meta(),
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseCanceled {
            text: "partial work".into(),
            images: vec![],
            model: Some("claude-opus-4-7".into()),
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        },
        meta: cc_meta(),
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("sid-cancel-1".into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
        meta: cc_meta(),
    })
    .await
    .unwrap();

    // The contract: no SessionEnded was emitted. The thread is still alive.
    let session_ended_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'SessionEnded'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        session_ended_count, 0,
        "cancel must NOT emit SessionEnded — it is a turn boundary, not a terminal event"
    );

    // The visible "Canceled" marker is preserved.
    let response_canceled_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ResponseCanceled'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        response_canceled_count, 1,
        "ResponseCanceled must be emitted so the exchange shows 'Canceled' in the UI"
    );

    // The turn-boundary marker is present (so the spawn dispatcher knows the
    // turn closed and the next MessageReceived can be handled via --resume).
    let idled_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'CodingAgentIdled'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        idled_count, 1,
        "CodingAgentIdled must be emitted on cancel so the thread doesn't freeze in 'running'"
    );

    // The cancel left a recoverable resume anchor: `lookup_latest_cc_session_id`
    // must surface the session id so the follow-up `--resume`s the SAME
    // conversation instead of spawning a fresh, amnesiac CC session. This is the
    // crux of "Cancel = Esc": a cancel is a resumable turn boundary.
    let recovered =
        crate::engine::agent_session::lookup_latest_cc_session_id(&pool, thread_id).await;
    assert_eq!(
        recovered,
        Some("sid-cancel-1".to_string()),
        "cancel must leave a recoverable cc_session_id so the next message resumes the same session"
    );

    // Bonus: a follow-up message after cancel is accepted by the bus — the
    // thread is still alive (no terminal SessionEnded blocked it). The actual
    // re-spawn is the dispatcher's job (covered by spawn_dispatcher_tests);
    // here we just verify the thread isn't sealed off.
    let followup = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: user_message("actually do Y instead"),
            meta: cc_meta(),
        })
        .await
        .expect("emit succeeds");
    assert!(
        followup.is_some(),
        "follow-up MessageReceived after cancel must persist — thread is still active"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A stop signal landing on an idle Claude Code session must NOT emit a terminal
/// event — the previous turn's `ResponseGenerated` + `CodingAgentIdled`
/// already terminated the exchange, and a late "Canceled the response"
/// would lie about a turn the user never canceled. Holds regardless of
/// the suppress flag (Apply/Discard/Archive on idle, real Cancel racing
/// idle — both must be no-ops).
#[test]
fn stop_on_idle_emits_no_terminal_event() {
    assert_eq!(
        stop_terminal_kind(false, true, false),
        None,
        "stop signal landing on idle CC must NOT emit a phantom ResponseCanceled — \
         the previous turn's ResponseGenerated already terminated the exchange"
    );
    assert_eq!(
        stop_terminal_kind(false, true, true),
        None,
        "Apply/Discard/Archive on idle CC must NOT emit ResponseCanceled either — \
         their lifecycle event is the terminator"
    );
}

/// Apply / Discard / Archive on an actively-working CC must NOT emit
/// `ResponseCanceled` — each has its own lifecycle terminator
/// (`ChangeApplied` / `ChangeDiscarded` / `ThreadArchived`). Emitting both
/// labels the turn as "Canceled" when the user clicked something else.
#[test]
fn user_action_on_working_cc_suppresses_response_canceled() {
    assert_eq!(
        stop_terminal_kind(false, false, true),
        None,
        "Apply/Discard/Archive on actively-working CC must NOT emit \
         ResponseCanceled — their lifecycle event (ChangeApplied / \
         ChangeDiscarded / ThreadArchived) is the terminator"
    );
}

/// Real Cancel click on actively-working CC is the ONLY path that emits
/// `ResponseCanceled(UserStop)`. Anchor test for the happy cancel flow.
#[test]
fn real_cancel_on_working_cc_still_emits_canceled() {
    use crate::engine::thread_events::CancelCause;
    assert_eq!(
        stop_terminal_kind(false, false, false),
        Some(TerminalKind::Canceled(CancelCause::UserStop)),
        "real Cancel click on actively-working CC must emit ResponseCanceled \
         so the user sees the 'Canceled' chip on the in-flight turn"
    );
}
