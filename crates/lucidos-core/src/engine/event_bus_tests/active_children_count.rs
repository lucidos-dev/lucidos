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

#[tokio::test]
async fn test_active_children_not_decremented_on_switch_aborted_child() {
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
        meta: switch_meta(),
    })
    .await
    .unwrap();

    assert_active_children(
        &pool,
        parent_id,
        1,
        "parent active_children_count must stay at 1 after a switch abort: \
         the engine resumes the child",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

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
            spend_cap: None,
            agent_exit: None,
            provider_error: None,
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

#[tokio::test]
async fn test_cc_child_switch_abort_does_not_decrement_parent() {
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
        meta: switch_meta(),
    })
    .await
    .unwrap();

    assert_active_children(
        &pool,
        parent_id,
        1,
        "parent active_children_count must stay at 1 after a CC switch abort: \
         the engine resumes the child",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_continuation_requested_re_increments_parent_after_restart_park() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "baseline: 1 active CC child").await;

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

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            reason: Some(
                crate::engine::continuation_reason::ENGINE_RESTART_INTERRUPT_REASON.to_string(),
            ),
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
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

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ContinuationRequested {
            reason: crate::engine::continuation_reason::USER_CLICKED_CONTINUE_REASON.to_string(),
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

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ContinuationRequested {
            reason: crate::engine::continuation_reason::USER_CLICKED_CONTINUE_REASON.to_string(),
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

#[tokio::test]
async fn test_cc_user_message_re_increments_parent_after_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "baseline: 1 active CC child").await;

    emit_cc_idle(&bus, child_id, false, None).await;
    assert_active_children(
        &pool,
        parent_id,
        0,
        "CodingAgentIdled decrements parent active_children_count to 0",
    )
    .await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentUserMessageSent {
            text: "another change please".into(),
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
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

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentUserMessageSent {
            text: "still typing".into(),
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
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

#[tokio::test]
async fn test_cc_second_idle_after_revive_decrements_parent_again() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "baseline: 1 active CC child").await;

    emit_cc_idle(&bus, child_id, false, None).await;
    assert_active_children(
        &pool,
        parent_id,
        0,
        "first CodingAgentIdled decrements parent to 0",
    )
    .await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentUserMessageSent {
            text: "another change please".into(),
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
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

#[tokio::test]
async fn test_cc_child_proposed_change_does_not_block_parent_active() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;
    emit_cc_idle(&bus, child_id, true, None).await;

    let (status, proposed): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        status, "idle",
        "child settles to 'idle' after CodingAgentIdled+ChangeProposed (Option B)"
    );
    assert!(proposed, "ChangeProposed makes the thread proposed");

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

async fn callback_pending(pool: &PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT parent_callback_pending FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

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
            owner_approval: None,
            permission_request: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn followed_up_live_coding_agent_child_reports_its_own_completion() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "baseline: the child is running").await;

    emit_cc_message_received(&bus, child_id, None, "go the other way").await;
    assert_active_children(
        &pool,
        parent_id,
        1,
        "a follow-up to a RUNNING child owes no re-increment",
    )
    .await;

    emit_cc_idle(&bus, child_id, false, None).await;
    assert_eq!(
        completion_cards(&pool, parent_id).await,
        1,
        "the interrupted turn reports once"
    );
    assert_active_children(&pool, parent_id, 0, "and the count comes down").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentPromptSent {
            input_event_id: None,
            text: "go the other way".into(),
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
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

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentPromptSent {
            input_event_id: None,
            text: String::new(),
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
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

#[tokio::test]
async fn question_parked_child_survives_the_startup_reconcile() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_question_asked(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "the parked child is in flight").await;

    EventBus::rebuild_children_counts(&pool).await.unwrap();

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

#[tokio::test]
async fn startup_reconcile_repairs_a_drifted_active_children_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    sqlx::query("UPDATE thread_summaries SET active_children_count = 2 WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();
    EventBus::rebuild_children_counts(&pool).await.unwrap();
    assert_active_children(&pool, parent_id, 1, "over-count repaired").await;

    sqlx::query("UPDATE thread_summaries SET active_children_count = 0 WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();
    EventBus::rebuild_children_counts(&pool).await.unwrap();
    assert_active_children(&pool, parent_id, 1, "under-count repaired").await;

    emit_cc_idle(&bus, child_id, false, None).await;
    sqlx::query("UPDATE thread_summaries SET active_children_count = 3 WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();
    EventBus::rebuild_children_counts(&pool).await.unwrap();
    assert_active_children(&pool, parent_id, 0, "an idle child counts for nothing").await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn follow_up_to_a_coding_agent_child_holding_a_pending_change() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, true, None).await;
    assert_active_children(&pool, parent_id, 0, "the child's work is done").await;
    let proposed: bool = sqlx::query_scalar(
        "SELECT coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
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

#[tokio::test]
async fn attribution_without_linkage_counts_no_child_and_wakes_nobody() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let spawning_thread_id = Uuid::new_v4();
    let spawned_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: spawning_thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "spawn something independent".into(),
            user_image_hashes: vec![],
            device_id: None,
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

    bus.emit(BusEvent::Thread {
        thread_id: spawned_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "independent work".into(),
            user_image_hashes: vec![],
            device_id: None,
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

fn switch_meta() -> EventMeta {
    EventMeta {
        actor: Some(MessageOrigin::Device {
            device_id: "test-device".into(),
        }),
        ..EventMeta::NONE
    }
}

#[tokio::test]
async fn archiving_a_paused_child_recounts_its_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    assert_active_children(&pool, parent_id, 1, "the spawn counts the child").await;
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseAborted {
            text: "".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        meta: switch_meta(),
    })
    .await
    .unwrap();
    assert_active_children(&pool, parent_id, 1, "a switch abort expects the resume").await;

    for id in [parent_id, child_id] {
        bus.emit(BusEvent::Thread {
            thread_id: id,
            event: ThreadEvent::ThreadArchived,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }

    assert_active_children(
        &pool,
        parent_id,
        0,
        "an archived child is idle, so its parent no longer counts it",
    )
    .await;
    assert_eq!(read_blocking_descendant_count(&pool, parent_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_boot_rebuild_heals_a_parent_still_counting_archived_children() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    for id in [parent_id, child_id] {
        bus.emit(BusEvent::Thread {
            thread_id: id,
            event: ThreadEvent::ThreadArchived,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }
    sqlx::query("UPDATE thread_summaries SET active_children_count = 12 WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();

    EventBus::rebuild_children_counts(&pool).await.unwrap();

    assert_active_children(
        &pool,
        parent_id,
        0,
        "the rebuild counts no archived idle child",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_family_recount_heals_one_drifted_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    for id in [parent_id, child_id] {
        bus.emit(BusEvent::Thread {
            thread_id: id,
            event: ThreadEvent::ThreadArchived,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }
    sqlx::query("UPDATE thread_summaries SET active_children_count = 3 WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();

    bus.recount_family_counts(parent_id).await.unwrap();

    assert_active_children(
        &pool,
        parent_id,
        0,
        "the recount counts no archived idle child",
    )
    .await;
    assert_eq!(read_blocking_descendant_count(&pool, parent_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
