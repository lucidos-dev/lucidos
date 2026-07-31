use super::super::*;
use super::*;

/// Regression for the bug fixed in commit 601224815: when the runtime
/// out-of-tx decrement in `notify_parent_if_child` fails (transient error,
/// or path skipped entirely), the parent's `active_children_count` stays
/// non-zero. Pre-fix, ChangeApplied never touched the counter — the parent
/// remained pinned in ACTIVE indefinitely even after the user clicked Apply.
/// Post-fix, ChangeApplied recomputes the counter from ground truth.
#[tokio::test]
async fn test_change_applied_reconciles_parent_active_children_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, true, None).await;

    // Simulate the runtime drift: force parent's active_children_count back
    // up to 1 (as if the CodingAgentIdled decrement failed). The reconcile on
    // ChangeApplied must heal it.
    sqlx::query("UPDATE thread_summaries SET active_children_count = 1 WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_active_children(&pool, parent_id, 1, "drift seeded").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ChangeApplied {
            change_id: format!("test-cid-{child_id}"),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (status, proposed): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "idle", "ChangeApplied keeps child at idle");
    assert!(!proposed, "ChangeApplied clears coding_agent_proposed");

    assert_active_children(
        &pool,
        parent_id,
        0,
        "ChangeApplied must reconcile parent active_children_count from running children — \
         drift heals without waiting for a restart",
    )
    .await;
    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        0,
        "ChangeApplied also reconciles blocking_descendant_count for the ancestor chain"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Mirror of `test_change_applied_reconciles_parent_active_children_count`
/// for the Discard path. The proposal lifecycle ends the same way whether
/// the user accepts or rejects, so both events must reconcile the parent.
#[tokio::test]
async fn test_change_discarded_reconciles_parent_active_children_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, true, None).await;

    // Seed the same drift as the Apply test.
    sqlx::query("UPDATE thread_summaries SET active_children_count = 1 WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_active_children(&pool, parent_id, 1, "drift seeded").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ChangeDiscarded {
            change_id: format!("test-cid-{child_id}"),
            actor: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (status, proposed): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "idle", "ChangeDiscarded keeps child at idle");
    assert!(!proposed, "ChangeDiscarded clears coding_agent_proposed");

    assert_active_children(
        &pool,
        parent_id,
        0,
        "ChangeDiscarded must reconcile parent active_children_count too — same lifecycle as Apply",
    )
    .await;
    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        0,
        "ChangeDiscarded also reconciles blocking_descendant_count for the ancestor chain"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: when a CC child idles, the parent's `active_children_count`
/// decrement runs OUT-OF-TX in `notify_parent_if_child` (separate UPDATE on
/// the pool, then a fire-and-forget `ChildrenCountChanged` broadcast). The
/// IN-TX broadcast that piggy-backs in the affected-ancestor loop carries
/// the parent's aggregate captured BEFORE the out-of-tx decrement, so it
/// reports `active=1` while the next broadcast reports `active=0`. If the
/// post-tx broadcast is lost (iOS PWA SSE drop, page off, restart window),
/// the page is stuck pulsing "waiting on children" until the next event with
/// an ancestor refresh — or, in the user-reported case, until `ChangeApplied`
/// runs `reconcile_parent_active_children_count` in-tx.
///
/// Contract: every `ChildrenCountChanged` broadcast for the parent that
/// fires during the terminal-event emit MUST carry the post-decrement
/// `active` value. The IN-TX aggregate is the only durable signal — the
/// out-of-tx broadcast is best-effort and can be lost without recourse.
#[tokio::test]
async fn test_cc_idle_in_tx_broadcast_carries_decremented_parent_active() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let mut rx = bus.subscribe();

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "child running, parent count = 1").await;

    // Drain setup events so we only observe what the idle emit produces.
    while rx.try_recv().is_ok() {}

    emit_cc_idle(&bus, child_id, false, None).await;

    let mut parent_active_values: Vec<i64> = Vec::new();
    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::ChildrenCountChanged { active, .. },
            ..
        } = &emitted.typed
        {
            if *tid == parent_id {
                parent_active_values.push(*active);
            }
        }
    }

    assert!(
        !parent_active_values.is_empty(),
        "Expected at least one ChildrenCountChanged broadcast for parent after child idled"
    );
    assert!(
        parent_active_values.iter().all(|&a| a == 0),
        "Every ChildrenCountChanged broadcast for parent after child idled must carry \
         active=0. The IN-TX broadcast is what the page durably applies via \
         applyAggregateToMeta; if it carries stale active=1, a missed out-of-tx \
         broadcast leaves the parent pulsing 'waiting on children' forever. \
         Got: {:?}",
        parent_active_values
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression for the multi-sibling double-decrement: when the parent has
/// 2+ running CC children and one of them idles, the in-tx
/// `reconcile_parent_active_children_count` correctly recomputes the parent's
/// `active_children_count` from ground truth (siblings still running), and
/// the out-of-tx `update_parent_after_child_terminal` must NOT then subtract
/// another 1 on top — otherwise the parent reports one fewer active child
/// than reality. The single-child test
/// `test_cc_idle_in_tx_broadcast_carries_decremented_parent_active` doesn't
/// catch this because `GREATEST(0, 0 - 1) = 0` clamps the over-decrement to
/// the same value the reconcile already wrote.
#[tokio::test]
async fn test_cc_idle_multi_sibling_parent_count_remains_at_running_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let mut rx = bus.subscribe();

    // Parent + two CC children sharing the same parent.
    let parent_id = Uuid::new_v4();
    let child_a = Uuid::new_v4();
    let child_b = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "fan out".into(),
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
    for cid in [child_a, child_b] {
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::MessageReceived {
                text: "child".into(),
                user_image_hashes: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: Some(parent_id),
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
        emit_cc_session_started(&bus, cid).await;
    }
    assert_active_children(
        &pool,
        parent_id,
        2,
        "two CC children running, parent count = 2",
    )
    .await;

    // Drain setup events.
    while rx.try_recv().is_ok() {}

    // Idle just child_a. Child_b is still running.
    emit_cc_idle(&bus, child_a, false, None).await;

    assert_active_children(
        &pool,
        parent_id,
        1,
        "parent.active_children_count must equal the count of still-running children (1, just child_b); \
         in-tx reconcile + out-of-tx decrement must not double-subtract",
    )
    .await;

    let mut parent_active_values: Vec<i64> = Vec::new();
    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::ChildrenCountChanged { active, .. },
            ..
        } = &emitted.typed
        {
            if *tid == parent_id {
                parent_active_values.push(*active);
            }
        }
    }
    assert!(
        !parent_active_values.is_empty(),
        "Expected at least one ChildrenCountChanged broadcast for parent"
    );
    assert!(
        parent_active_values.iter().all(|&a| a == 1),
        "Every ChildrenCountChanged broadcast for parent must carry active=1 \
         (child_b is still running). A broadcast of active=0 would tell the page \
         'all children done' while a sibling is still mid-stream. Got: {:?}",
        parent_active_values
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A child paused on a user question (`status='waiting_for_user_answer'`)
/// is still in flight — the user owes an answer before the agent can
/// resume. The `+1` from MessageReceived is never decremented on entry to
/// WaitingForUserAnswer, and `reincrement_parent_active_count_if_revived`
/// explicitly opts out of re-incrementing on UserQuestionAnswered, so the
/// parent's counter is expected to stay bumped throughout the paused
/// window. The in-tx `reconcile_parent_active_children_count` must match
/// that semantic — counting WaitingForUserAnswer children as active —
/// otherwise a sibling's terminal event silently zeroes the parent's
/// counter while a paused child still owes work. (Same definition the
/// sibling helper `reconcile_blocking_descendant_count_for_ancestors`
/// uses at the `status IN ('running', 'waiting_for_user_answer')` filter.)
#[tokio::test]
async fn test_cc_idle_with_waiting_for_user_answer_sibling_keeps_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_paused = Uuid::new_v4();
    let child_running = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "fan out".into(),
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
    for cid in [child_paused, child_running] {
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::MessageReceived {
                text: "child".into(),
                user_image_hashes: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: Some(parent_id),
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
        emit_cc_session_started(&bus, cid).await;
    }
    assert_active_children(
        &pool,
        parent_id,
        2,
        "two CC children running, parent count = 2",
    )
    .await;

    // child_paused asks a permission question → status='waiting_for_user_answer'.
    // No reincrement / decrement on the parent; parent.count stays at 2.
    bus.emit(BusEvent::Thread {
        thread_id: child_paused,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-1".into(),
            tool_use_id: "tu-1".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({"file_path": "/tmp/x"}),
            summary: "Edit /tmp/x".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    let paused_status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(child_paused)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(paused_status, "waiting_for_user_answer");
    assert_active_children(
        &pool,
        parent_id,
        2,
        "WaitingForUserAnswer entry does not decrement parent count",
    )
    .await;

    // Sibling idles. The in-tx reconcile must still count child_paused as in
    // flight (the user owes an answer) — anything else silently drops the
    // count below the true number of unfinished children.
    emit_cc_idle(&bus, child_running, false, None).await;

    assert_active_children(
        &pool,
        parent_id,
        1,
        "parent.active_children_count must still count the WaitingForUserAnswer child; \
         reconcile_parent_active_children_count must filter on \
         status IN ('running', 'waiting_for_user_answer') to match the \
         +1-on-MessageReceived / no-decrement-on-WfUA invariant the rest of the \
         projection maintains",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Terminal-cause CC abort (SafetyNet / ProcessKilled) is unrecoverable
/// without explicit user resume — must decrement (regression of 5a017815c
/// otherwise: parent stays Active forever).
#[tokio::test]
async fn test_cc_child_terminal_abort_decrements_parent() {
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
            cause: crate::engine::thread_events::AbortCause::ProcessKilled,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent active_children_count must drop to 0 after terminal CC abort \
         (ProcessKilled) — without it the parent stays Active forever",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// If a CC sub-thread is canceled and a later CodingAgentIdled lands (e.g. a
/// background Claude tick that fires after the cancel persisted), the parent
/// must NOT be double-decremented. The cancel decrements once and marks the
/// callback as sent; the late idle is a no-op.
#[tokio::test]
async fn test_cc_child_idle_after_cancel_does_not_double_decrement() {
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
    assert_active_children(&pool, parent_id, 0, "decremented after cancel").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
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
        "late CodingAgentIdled after cancel must not double-decrement (the cancel \
         already decremented; the late idle must be suppressed by callback-sent guard)",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: a CC sub-thread that hits a stale resume must NOT decrement its
/// parent's `active_children_count` or fire a completion callback. The chat
/// handler is mid-retry against a fresh session — the real CodingAgentIdled
/// (with results) lands seconds later, and decrementing now would orphan it
/// (the second decrement clamps at 0; the second callback is suppressed by
/// `parent_callback_sent`).
#[tokio::test]
async fn test_cc_child_stale_resume_does_not_decrement_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "stale-sid".into(),
            branch: "claude-code/stale".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    // Stale resume: SessionEnded { StaleResume } fires, then the chat handler
    // retries with a fresh session.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionEnded {
            reason: SessionEndReason::StaleResume,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_active_children(
        &pool,
        parent_id,
        1,
        "StaleResume is mid-retry — parent's active_children_count must stay 1",
    )
    .await;

    let mut callbacks = vec![];
    while let Ok(cb) = callback_rx.try_recv() {
        callbacks.push(cb);
    }
    assert!(
        callbacks.is_empty(),
        "StaleResume must not fire a parent callback — the retry hasn't produced \
         a result yet, so the parent would be told the child finished with no work"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
