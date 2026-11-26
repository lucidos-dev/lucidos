use super::super::*;
use super::*;

#[tokio::test]
async fn test_change_applied_reconciles_parent_active_children_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, true, None).await;

    sqlx::query("UPDATE thread_summaries SET active_children_count = 1 WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_active_children(&pool, parent_id, 1, "drift seeded").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ChangeApplied {
            content: Default::default(),
            change_id: test_change_id(child_id).to_string(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            landing_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (status, proposed): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "idle", "ChangeApplied keeps child at idle");
    assert!(!proposed, "ChangeApplied takes the thread off proposed");

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

#[tokio::test]
async fn test_change_discarded_reconciles_parent_active_children_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, true, None).await;

    sqlx::query("UPDATE thread_summaries SET active_children_count = 1 WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_active_children(&pool, parent_id, 1, "drift seeded").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ChangeDiscarded {
            change_id: test_change_id(child_id).to_string(),
            actor: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (status, proposed): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "idle", "ChangeDiscarded keeps child at idle");
    assert!(!proposed, "ChangeDiscarded takes the thread off proposed");

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

#[tokio::test]
async fn test_cc_idle_in_tx_broadcast_carries_decremented_parent_active() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let mut rx = bus.subscribe();

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "child running, parent count = 1").await;

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

#[tokio::test]
async fn test_cc_idle_multi_sibling_parent_count_remains_at_running_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let mut rx = bus.subscribe();

    let parent_id = Uuid::new_v4();
    let child_a = Uuid::new_v4();
    let child_b = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "fan out".into(),
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
    for cid in [child_a, child_b] {
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: "child".into(),
                user_image_hashes: vec![],
                device_id: None,
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

    while rx.try_recv().is_ok() {}

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
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "fan out".into(),
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
    for cid in [child_paused, child_running] {
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: "child".into(),
                user_image_hashes: vec![],
                device_id: None,
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

    bus.emit(BusEvent::Thread {
        thread_id: child_paused,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-1".into(),
            tool_use_id: "tu-1".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({"file_path": "/tmp/x"}),
            summary: "Edit /tmp/x".into(),
            grant_offer: None,
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
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            reason: None,
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
        "late CodingAgentIdled after cancel must not double-decrement (the cancel \
         already decremented; the late idle must be suppressed by callback-sent guard)",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_cc_child_stale_resume_does_not_decrement_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            session_id: "stale-sid".into(),
            branch: "claude-code/stale".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

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
