use super::super::*;
use super::*;

#[tokio::test]
async fn test_active_children_count_on_child_spawn() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, _child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    assert_active_children(
        &pool,
        parent_id,
        1,
        "parent should have 1 active child after spawn",
    )
    .await;

    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    // archive_state stays at the column default; Ongoing is display-only.
    assert_eq!(section, "inbox");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_active_children_decremented_on_canceled_child() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseCanceled {
            text: "partial".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent active_children_count must be 0 after child canceled",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Transient `ResponseAborted` (engine shutdown, recovery sweep) is mid-retry —
/// the engine resumes the child on next visit. The active-children counter
/// must stay put so the parent's UI keeps showing the child as still running
/// until either resume succeeds (no change) or the user explicitly cancels.
#[tokio::test]
async fn test_active_children_not_decremented_on_transient_aborted_child() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseAborted {
            text: "".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_active_children(
        &pool,
        parent_id,
        1,
        "parent active_children_count must stay at 1 after transient abort — \
         the engine resumes the child on next visit",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Terminal `ResponseAborted` (SafetyNet / ProcessKilled) is unrecoverable
/// without explicit user resume — the parent's counter must drop or the
/// parent UI displays as Active forever (regression of the bug fixed by
/// commit 5a017815c).
#[tokio::test]
async fn test_active_children_decremented_on_terminal_aborted_child() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseAborted {
            text: "".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::SafetyNet,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent active_children_count must drop to 0 after terminal abort \
         (SafetyNet) — without it the parent stays Active forever",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_cc_child_session_ended_without_idle_decrements_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    // Claude Code session starts then is canceled immediately — no CodingAgentIdled emitted
    emit_cc_session_started(&bus, child_id).await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseCanceled {
            text: "".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // SessionEnded must decrement since no CodingAgentIdled was emitted.
    // Phase 4: only terminal-only reasons (Shutdown / Panic / Closed) remain;
    // Closed stands in for the prior `UserEnded` here.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionEnded {
            reason: SessionEndReason::Closed,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent active_children_count must be 0 after CC child ended without idle",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: a CC sub-thread that fails its agentic loop emits
/// `ResponseFailed` *and then* `CodingAgentIdled` in rapid succession.
/// Before this fix, ResponseFailed went through the `should_callback` path
/// (which marks `parent_callback_sent = true`) but NOT the `should_decrement`
/// path, so the parent's `active_children_count` stayed at 1. The follow-up
/// `CodingAgentIdled` then early-returned via the dedup guard
/// (`is_coding_agent && callback_already_sent && CodingAgentIdled`) and never
/// got a chance to decrement either. Result: the parent pulses as "waiting
/// for children" indefinitely even though no child is actually running.
#[tokio::test]
async fn test_cc_child_response_failed_then_idled_decrements_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseFailed {
            error: "agentic loop blew up".into(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    emit_cc_idle(&bus, child_id, false, None).await;

    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent active_children_count must drop to 0 after ResponseFailed + Idled",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: when a CC sub-thread is canceled by the user before it ever
/// emits CodingAgentIdled and without a follow-up SessionEnded (the typical
/// shape: ResponseCanceled fires from the cancellation path, the session sits
/// archived without ever being resumed), the parent's `active_children_count`
/// must still be decremented — otherwise the parent stays "Active" forever.
#[tokio::test]
async fn test_cc_child_canceled_without_session_ended_decrements_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseCanceled {
            text: "".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent active_children_count must be 0 after CC child canceled without \
         SessionEnded — otherwise the parent stays Active forever",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Transient-cause CC abort (EngineShutdown / RecoveryAfterRestart) is
/// mid-retry — must NOT decrement, the engine resumes on next visit.
#[tokio::test]
async fn test_cc_child_transient_abort_does_not_decrement_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseAborted {
            text: "".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_active_children(
        &pool,
        parent_id,
        1,
        "parent active_children_count must stay at 1 after transient CC abort \
         — the engine resumes the child on next visit",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression for the restart→resume flow: after engine crash recovery emits
/// the paired ResponseAborted{RecoveryAfterRestart} + synthetic
/// CodingAgentIdled{reason=engine_restart_interrupt} for an interrupted CC
/// child, the parked child is no longer active so the parent's count
/// correctly drops to 0. When the user clicks Continue and the child starts
/// running again via ContinuationRequested, the parent's `active_children_count`
/// MUST bounce back to 1 — otherwise the parent's ThreadStatusIcon stays
/// 'idle' (no pulsing dot) even though the child is actively running again.
#[tokio::test]
async fn test_continuation_requested_re_increments_parent_after_restart_park() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "baseline: 1 active CC child").await;

    // Recovery's ResponseAborted with the transient cause — no decrement
    // (existing behavior, asserted by the sibling transient-abort test).
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseAborted {
            text: "".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::RecoveryAfterRestart,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Recovery's paired synthetic Idled — decrements: the parked child is
    // no longer active (it's awaiting a user Continue click).
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: Some(
                crate::engine::agent_recovery::ENGINE_RESTART_INTERRUPT_REASON.to_string(),
            ),
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_active_children(
        &pool,
        parent_id,
        0,
        "parked CC child must drop parent active_children_count to 0",
    )
    .await;

    // User clicks Continue — the child is alive again.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ContinuationRequested {
            reason: crate::engine::agent_recovery::USER_CLICKED_CONTINUE_REASON.to_string(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    assert_active_children(
        &pool,
        parent_id,
        1,
        "ContinuationRequested on a parked CC child must bring parent active_children_count back to 1",
    )
    .await;

    // Idempotency: a second ContinuationRequested (e.g. duplicate user click)
    // must NOT double-increment — the child was already running.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ContinuationRequested {
            reason: crate::engine::agent_recovery::USER_CLICKED_CONTINUE_REASON.to_string(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    assert_active_children(
        &pool,
        parent_id,
        1,
        "duplicate ContinuationRequested on an already-running child must not double-increment",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression for the "parent stuck at 1/1 sub-threads done while child is
/// Working" bug: when a CC child idles, `notify_parent_if_child`
/// decrements the parent's `active_children_count`. When the user types a
/// follow-up (`CodingAgentUserMessageSent`), the projection flips the child
/// back to `status='running'` — but without an explicit re-increment the
/// parent's counter stays at 0, so the drawer's "X/Y sub-threads done"
/// label is wrong and the parent's status icon stays idle (no pulsing
/// dot) even though the child is actively running.
///
/// Mirrors `test_continuation_requested_re_increments_parent_after_restart_park`
/// but for the user-follow-up path instead of the parked-restart path.
#[tokio::test]
async fn test_cc_user_message_re_increments_parent_after_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "baseline: 1 active CC child").await;

    // CC child completes normally — terminal CodingAgentIdled decrements
    // parent via notify_parent_if_child.
    emit_cc_idle(&bus, child_id, false, None).await;
    assert_active_children(
        &pool,
        parent_id,
        0,
        "CodingAgentIdled decrements parent active_children_count to 0",
    )
    .await;

    // User types a follow-up in the CC thread — the child is alive again.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentUserMessageSent {
            text: "another change please".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    assert_active_children(
        &pool,
        parent_id,
        1,
        "CodingAgentUserMessageSent on an idled CC child must bring parent \
         active_children_count back to 1 — otherwise the drawer shows '1/1 \
         sub-threads done' while the child is Working",
    )
    .await;

    // Idempotency: a second user message on the already-running child must
    // not double-increment (CC threads can receive multiple follow-ups
    // back-to-back without an intervening idle).
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentUserMessageSent {
            text: "still typing".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    assert_active_children(
        &pool,
        parent_id,
        1,
        "second CodingAgentUserMessageSent on already-running child must not \
         double-increment",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression for the second-idle stuck-counter bug introduced by the revive
/// re-increment. Sequence:
///
///   1. CC child idles → `notify_parent_if_child` decrements parent to 0 and
///      sets `parent_callback_sent=TRUE` on the child.
///   2. User types a follow-up → `reincrement_parent_active_count_if_revived`
///      bumps parent back to 1.
///   3. CC child idles AGAIN.
///
/// At step 3 the dedup guard in `notify_parent_if_child`
/// (`is_coding_agent && callback_already_sent && CodingAgentIdled`)
/// short-circuits and the decrement is skipped — so the parent's counter is
/// stuck at 1 forever and the drawer reads "0/1 sub-threads done" while the
/// child is actually Idle. The revive helper must therefore also clear
/// `parent_callback_sent=FALSE` in the same tx, so the next terminal event
/// is a fresh first-idle from the dedup's perspective.
#[tokio::test]
async fn test_cc_second_idle_after_revive_decrements_parent_again() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "baseline: 1 active CC child").await;

    // First idle: decrements parent + marks parent_callback_sent.
    emit_cc_idle(&bus, child_id, false, None).await;
    assert_active_children(
        &pool,
        parent_id,
        0,
        "first CodingAgentIdled decrements parent to 0",
    )
    .await;

    // User follow-up revives the child.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentUserMessageSent {
            text: "another change please".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    assert_active_children(
        &pool,
        parent_id,
        1,
        "revive bumps parent active_children_count back to 1",
    )
    .await;

    emit_cc_idle(&bus, child_id, false, None).await;
    assert_active_children(
        &pool,
        parent_id,
        0,
        "second CodingAgentIdled after revive decrements parent back to 0",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression for the "parent stuck in ACTIVE with 0/1 sub-threads done" bug:
/// a CC child whose session ends with a proposed change must NOT keep the
/// parent's `active_children_count` incremented. Under Option B the child
/// settles to `status='idle'` (the diff is an artifact, not a parked loop),
/// so the `CodingAgentIdled` decrement runs through `notify_parent_if_child`
/// AND the child's status matches the startup-recompute predicate. Parent
/// `blocking_descendant_count` stays at 1 because `is_blocking` clause 3
/// fires on the proposed change.
#[tokio::test]
async fn test_cc_child_proposed_change_does_not_block_parent_active() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;
    emit_cc_idle(&bus, child_id, true, None).await;

    let (status, proposed): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        status, "idle",
        "child settles to 'idle' after CodingAgentIdled+ChangeProposed (Option B)"
    );
    assert!(proposed, "ChangeProposed sets coding_agent_proposed");

    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent active_children_count must drop to 0 — the child's work is done, \
         the diff is an artifact for review (not a parked loop)",
    )
    .await;
    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        1,
        "parent blocking_descendant_count stays at 1 — proposed change blocks archive \
         via is_blocking clause 3"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
