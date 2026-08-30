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
/// (which clears `parent_callback_pending`) but NOT the `should_decrement`
/// path, so the parent's `active_children_count` stayed at 1. The follow-up
/// `CodingAgentIdled` then early-returned via the dedup guard
/// (`is_coding_agent && !parent_callback_pending && CodingAgentIdled`) and never
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

/// Regression for the "parent stuck showing no active sub-thread while child
/// is Working" bug: when a CC child idles, `notify_parent_if_child`
/// decrements the parent's `active_children_count`. When the user types a
/// follow-up (`CodingAgentUserMessageSent`), the projection flips the child
/// back to `status='running'` — but without an explicit re-increment the
/// parent's counter stays at 0, so the drawer's collapsed sub-thread count
/// loses its active tint and the parent's status icon stays idle (no pulsing
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
         active_children_count back to 1 — otherwise the parent's status dot \
         and collapsed sub-thread count read as idle while the child is Working",
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
///      clears `parent_callback_pending` on the child.
///   2. User types a follow-up → `reincrement_parent_active_count_if_revived`
///      bumps parent back to 1.
///   3. CC child idles AGAIN.
///
/// At step 3 the dedup guard in `notify_parent_if_child`
/// (`is_coding_agent && !parent_callback_pending && CodingAgentIdled`)
/// short-circuits and the decrement is skipped — so the parent's counter is
/// stuck at 1 forever and the drawer shows the parent as having an active
/// sub-thread while the child is actually Idle. The revive helper must therefore also set
/// `parent_callback_pending=TRUE` in the same tx, so the next terminal event
/// is a fresh first-idle from the dedup's perspective.
#[tokio::test]
async fn test_cc_second_idle_after_revive_decrements_parent_again() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "baseline: 1 active CC child").await;

    // First idle: decrements parent + clears parent_callback_pending.
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

/// Regression for the "parent stuck in ACTIVE with a phantom active sub-thread"
/// bug: a CC child whose session ends with a proposed change must NOT keep the
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

/// Read the child's `parent_callback_pending` marker.
async fn callback_pending(pool: &PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT parent_callback_pending FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Count the persisted completion cards on a parent.
async fn completion_cards(pool: &PgPool, parent_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ChildThreadCompleted'",
    )
    .bind(parent_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Park `thread_id` on a user question.
async fn emit_question_asked(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: Uuid::new_v4().to_string(),
            cc_session_id: "test-session".into(),
            question: "which way?".into(),
            options: vec![],
            worktree_path: None,
            multi_select: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// Hazard 12, the live half: a parent redirects a coding-agent child that is
/// mid-turn. The follow-up's `MessageReceived` lands while the child is still
/// `running`, so no revive is owed; the interrupted turn idles and takes the
/// card with it; then the child picks the follow-up off `msg_tx` and emits a
/// non-empty `CodingAgentPromptSent` for the redirected turn. That turn's own
/// completion has to reach the parent too, or the parent hears only about the
/// turn it interrupted and never about the work it asked for.
#[tokio::test]
async fn followed_up_live_coding_agent_child_reports_its_own_completion() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "baseline: the child is running").await;

    // The follow-up lands on a live child: MessageReceived with no
    // parent_thread_id, the shape the coding-agent fast path emits.
    emit_cc_message_received(&bus, child_id, None, "go the other way").await;
    assert_active_children(
        &pool,
        parent_id,
        1,
        "a follow-up to a RUNNING child owes no re-increment",
    )
    .await;

    // Turn 1 reaches its boundary and takes the first card with it.
    emit_cc_idle(&bus, child_id, false, None).await;
    assert_eq!(
        completion_cards(&pool, parent_id).await,
        1,
        "the interrupted turn reports once"
    );
    assert_active_children(&pool, parent_id, 0, "and the count comes down").await;

    // The child picks the queued follow-up off msg_tx and starts the
    // redirected turn.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentPromptSent {
            text: "go the other way".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    assert!(
        callback_pending(&pool, child_id).await,
        "a start event means the parent has not been told about THIS turn"
    );
    assert_active_children(&pool, parent_id, 1, "and the child is in flight again").await;

    // Turn 2 completes.
    emit_cc_idle(&bus, child_id, false, None).await;
    assert_eq!(
        completion_cards(&pool, parent_id).await,
        2,
        "the redirected turn must report its own completion"
    );
    assert_active_children(&pool, parent_id, 0, "and the count comes down again").await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Hazard 12's `ContinuationRequested` twin: a human clicking Continue on a
/// coding-agent child is a start event too. The arm does its own inline
/// re-increment but never touched the marker, so the resumed turn's
/// `CodingAgentIdled` hit the dedup guard and the parent was never told.
#[tokio::test]
async fn continue_on_a_coding_agent_child_reports_its_completion() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, false, None).await;
    assert_eq!(
        completion_cards(&pool, parent_id).await,
        1,
        "first turn reports"
    );
    assert_active_children(&pool, parent_id, 0, "and decrements").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ContinuationRequested {
            reason: "user clicked Continue".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    assert_active_children(&pool, parent_id, 1, "Continue re-increments the parent").await;
    assert!(
        callback_pending(&pool, child_id).await,
        "and owes the parent a card for the continued turn"
    );

    emit_cc_idle(&bus, child_id, false, None).await;
    assert_eq!(
        completion_cards(&pool, parent_id).await,
        2,
        "the continued turn must report its own completion"
    );
    assert_active_children(&pool, parent_id, 0, "and decrement").await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The dedup guard's stated purpose must survive the marker's new predicate: a
/// coding-agent child can emit `CodingAgentIdled` more than once for the same
/// turn (auto-harden, background agents), and an extra idle with NO intervening
/// start event is still swallowed. This is a regression test for the flip and
/// for the start-event predicate at the same time.
#[tokio::test]
async fn extra_idle_without_a_start_event_is_still_deduped() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    emit_cc_idle(&bus, child_id, false, None).await;
    emit_cc_idle(&bus, child_id, false, None).await;
    emit_cc_idle(&bus, child_id, false, None).await;

    assert_eq!(
        completion_cards(&pool, parent_id).await,
        1,
        "three idles with no start event between them are one completion"
    );
    assert_active_children(&pool, parent_id, 0, "and one decrement").await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The one genuinely open membership question, settled here by card count. The
/// question-resume marker at `agent_question.rs` emits an EMPTY
/// `CodingAgentPromptSent` purely so the timeline shows a Thinking step; it
/// asserts no new agent intent, and its own arm already skips the status write
/// for that reason. Treating it as a start event would re-arm the marker after
/// a card was already sent, so the auto-harden / background-agent idle the
/// dedup guard exists for would produce a spurious second card.
#[tokio::test]
async fn an_empty_resume_marker_is_not_a_start_event() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, false, None).await;
    assert_eq!(
        completion_cards(&pool, parent_id).await,
        1,
        "the turn reports once"
    );

    // A background agent asks for a permission, the human resolves it, and the
    // resume marker fires with empty text.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentPromptSent {
            text: String::new(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    assert!(
        !callback_pending(&pool, child_id).await,
        "an empty resume marker asserts no new agent intent, so it must not \
         re-arm the parent callback"
    );

    emit_cc_idle(&bus, child_id, false, None).await;
    assert_eq!(
        completion_cards(&pool, parent_id).await,
        1,
        "so the trailing idle is still deduped, exactly as it is today"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Hazard 8(a): the revive gate skipped the re-increment only for a `running`
/// child, but the in-flight set the reconcile counts is
/// `{running, waiting_for_user_answer}`. A child parked on a question or a
/// permission card is therefore still counted in the parent's
/// `active_children_count`, and an Agent-mode message landing on it
/// re-incremented, over-counting by one. A human's message would have been
/// routed to `UserQuestionAnswered` instead, which is why the bug was latent;
/// an agent follow-up deliberately falls through to the injection path.
#[tokio::test]
async fn question_parked_child_follow_up_does_not_double_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_question_asked(&bus, child_id).await;

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(child_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "waiting_for_user_answer", "the child is parked");
    assert_active_children(
        &pool,
        parent_id,
        1,
        "a parked child is still in flight: nothing decremented it",
    )
    .await;

    emit_cc_message_received(&bus, child_id, None, "while you wait, also do this").await;

    assert_active_children(
        &pool,
        parent_id,
        1,
        "a follow-up to a parked child must not re-increment: the counter was \
         never decremented",
    )
    .await;
    assert!(
        callback_pending(&pool, child_id).await,
        "but the start event still arms the marker",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Hazard 8(a) from the other side. The startup reconcile filtered
/// `status = 'running'` alone, so every boot recomputed a parent's
/// `active_children_count` WITHOUT a child parked on a question or a
/// permission card, contradicting the in-tx reconcile. Question-parked threads
/// are deliberately preserved across a restart and `UserQuestionAnswered` does
/// not re-increment, so the under-count persisted until some sibling terminal
/// fired the in-tx reconcile.
///
/// Behaviour change, deliberate: after a restart a parent whose child is parked
/// keeps counting that child as active, which is what the in-tx reconcile has
/// always said.
#[tokio::test]
async fn question_parked_child_survives_the_startup_reconcile() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_question_asked(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "the parked child is in flight").await;

    EventBus::rebuild_active_children_count(&pool)
        .await
        .unwrap();

    assert_active_children(
        &pool,
        parent_id,
        1,
        "the startup reconcile must agree with the in-tx one: a parked child \
         is still in flight",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The startup reconcile still repairs real drift in both directions.
#[tokio::test]
async fn startup_reconcile_repairs_a_drifted_active_children_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    // Over-count: the child is running but the parent claims two.
    sqlx::query("UPDATE thread_summaries SET active_children_count = 2 WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();
    EventBus::rebuild_active_children_count(&pool)
        .await
        .unwrap();
    assert_active_children(&pool, parent_id, 1, "over-count repaired").await;

    // Under-count: the child is still running but the parent claims zero.
    sqlx::query("UPDATE thread_summaries SET active_children_count = 0 WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();
    EventBus::rebuild_active_children_count(&pool)
        .await
        .unwrap();
    assert_active_children(&pool, parent_id, 1, "under-count repaired").await;

    // A child that really did finish drops the parent back to zero.
    emit_cc_idle(&bus, child_id, false, None).await;
    sqlx::query("UPDATE thread_summaries SET active_children_count = 3 WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();
    EventBus::rebuild_active_children_count(&pool)
        .await
        .unwrap();
    assert_active_children(&pool, parent_id, 0, "an idle child counts for nothing").await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The revive gate's own stated reason: an idle coding-agent child holding a
/// pending change is `is_blocking = true` via clause 3, so an `is_blocking`
/// gate would skip the re-increment it needs. Widening the gate to the
/// in-flight set must not disturb that, because `waiting` (the status a
/// proposed change parks at) is outside the in-flight set exactly as `idle` is.
#[tokio::test]
async fn follow_up_to_a_coding_agent_child_holding_a_pending_change() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, true, None).await;
    assert_active_children(&pool, parent_id, 0, "the child's work is done").await;
    let proposed: bool = sqlx::query_scalar(
        "SELECT coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(proposed, "the child left a pending change");

    emit_cc_message_received(&bus, child_id, None, "revise it").await;

    assert_active_children(
        &pool,
        parent_id,
        1,
        "a child holding a pending change still re-increments on revive",
    )
    .await;
    assert!(
        callback_pending(&pool, child_id).await,
        "and owes the parent a card for the revised turn"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A `relation: "top"` spawn stamps a `ThreadLink` origin naming its spawning thread
/// so the message route popover can link back, but carries NO
/// `parent_thread_id`. The counting and callback paths key on the linkage, and
/// this pins that they keep doing so: the spawning thread must not grow a child, must
/// not be owed a card, and must not be woken when the spawned thread finishes.
///
/// This is the counting-side half of the display fix. If either path ever
/// starts reading `origin` instead, the spawning thread's drawer sprouts an
/// "N sub-threads" badge for work it deliberately fired and forgot.
#[tokio::test]
async fn attribution_without_linkage_counts_no_child_and_wakes_nobody() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let spawning_thread_id = Uuid::new_v4();
    let spawned_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: spawning_thread_id,
        event: ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "spawn something independent".into(),
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
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // The top-spawn shape: origin names the spawning thread, linkage is absent.
    bus.emit(BusEvent::Thread {
        thread_id: spawned_id,
        event: ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "independent work".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Agent,
            model: None,
            reasoning_effort: None,
            origin: Some(MessageOrigin::ThreadLink {
                thread_id: spawning_thread_id,
                title: None,
                spawning_event_id: Some(Uuid::new_v4()),
                mode: ActorMode::Agent,
                direction: crate::engine::thread_events::ThreadDirection::Parent,
            }),
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    assert_children_counters(
        &pool,
        spawning_thread_id,
        0,
        0,
        "attribution alone must not make the spawning thread a parent",
    )
    .await;
    let projected_parent: Option<Uuid> =
        sqlx::query_scalar("SELECT parent_thread_id FROM thread_summaries WHERE thread_id = $1")
            .bind(spawned_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        projected_parent, None,
        "the projection reads the linkage field, not the origin"
    );
    assert!(
        !callback_pending(&pool, spawned_id).await,
        "a top spawn owes nobody a report"
    );

    bus.emit(BusEvent::Thread {
        thread_id: spawned_id,
        event: ThreadEvent::ResponseGenerated {
            text: "done".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let mut callbacks = vec![];
    while let Ok(cb) = callback_rx.try_recv() {
        callbacks.push(cb);
    }
    assert!(
        callbacks.is_empty(),
        "the spawning thread must not be woken by a thread it fired and forgot: {callbacks:?}"
    );
    assert_eq!(
        completion_cards(&pool, spawning_thread_id).await,
        0,
        "and gets no child-completion card"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
