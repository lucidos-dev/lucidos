use super::super::*;
use super::*;

#[tokio::test]
async fn fresh_running_cc_child_blocks_parent_before_first_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    for i in 0..3 {
        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event: ThreadEvent::CodingAgentToolCalled {
                tool_use_id: format!("tu-{i}"),
                name: "Edit".into(),
                args: serde_json::json!({}),
                description: String::new(),
                coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
                parent_tool_use_id: None,
                api_call_id: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    let stored_section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(child_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored_section, "inbox");
    assert_eq!(read_blocking_descendant_count(&pool, parent_id).await, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn rebuild_treats_legacy_archived_running_as_blocking() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    sqlx::query(
        "UPDATE thread_summaries SET archive_state='archived', status='running' \
         WHERE thread_id = $1",
    )
    .bind(child_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE thread_summaries SET blocking_descendant_count = 0 WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();

    EventBus::rebuild_blocking_descendant_count(&pool)
        .await
        .unwrap();

    assert_eq!(read_blocking_descendant_count(&pool, parent_id).await, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn message_received_on_child_bumps_parent_blocking_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_with_idle_cc_child(&bus, &pool).await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "follow up".into(),
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

    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        1,
        "MessageReceived → Running must bump parent's blocking_descendant_count to 1"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn response_generated_decrements_parent_blocking_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_with_idle_cc_child(&bus, &pool).await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "ping".into(),
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
    assert_eq!(read_blocking_descendant_count(&pool, parent_id).await, 1);

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
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

    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        0,
        "ResponseGenerated → Idle must decrement parent's blocking_descendant_count to 0"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn change_proposed_on_cc_child_bumps_ancestor_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_with_idle_cc_child(&bus, &pool).await;

    emit_change_proposed(&bus, child_id, "claude-code/test", false).await;

    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        1,
        "ChangeProposed on CC child must bump parent's blocking_descendant_count"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn thread_archived_on_blocking_child_decrements_parent_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_with_idle_cc_child(&bus, &pool).await;
    emit_change_proposed(&bus, child_id, "claude-code/test", false).await;
    assert_eq!(read_blocking_descendant_count(&pool, parent_id).await, 1);

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        0,
        "ThreadArchived must decrement parent's blocking_descendant_count to 0"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn user_question_lifecycle_propagates_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_with_idle_cc_child(&bus, &pool).await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: "tu-1".into(),
            cc_session_id: "sess-1".into(),
            question: "Proceed?".into(),
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
    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        1,
        "UserQuestionAsked → WaitingForUserAnswer must bump parent count to 1"
    );
    assert_eq!(
        read_attention_descendant_count(&pool, parent_id).await,
        1,
        "UserQuestionAsked → WaitingForUserAnswer also bumps the attention \
         counter (WFUA needs user attention, drives Current-bubble routing)"
    );

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: "tu-1".into(),
            quiet: false,
            answer: AnswerKind::FreeText {
                text: "yes".into(),
                image_hashes: vec![],
            },
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        1,
        "UserQuestionAnswered → Running stays blocking; parent count stays at 1"
    );
    assert_eq!(
        read_attention_descendant_count(&pool, parent_id).await,
        0,
        "UserQuestionAnswered → Running drops the attention counter — system \
         is doing work again, no user attention needed"
    );

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseGenerated {
            text: "ok".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        0,
        "ResponseGenerated → Idle closes the round-trip; parent count back to 0"
    );
    assert_eq!(
        read_attention_descendant_count(&pool, parent_id).await,
        0,
        "ResponseGenerated → Idle keeps attention at 0"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_held_waiting_child_raises_no_attention_on_its_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_with_idle_cc_child(&bus, &pool).await;
    async fn counts(pool: &PgPool, parent_id: Uuid) -> (i32, i32) {
        (
            read_blocking_descendant_count(pool, parent_id).await,
            read_attention_descendant_count(pool, parent_id).await,
        )
    }
    async fn rebuilt(pool: &PgPool, parent_id: Uuid) -> (i32, i32) {
        EventBus::rebuild_blocking_descendant_count(pool)
            .await
            .unwrap();
        counts(pool, parent_id).await
    }
    let emit = |event: ThreadEvent| {
        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event,
            meta: EventMeta::NONE,
        })
    };

    emit(ThreadEvent::UserQuestionAsked {
        tool_use_id: "tu-h".into(),
        cc_session_id: "sess-h".into(),
        question: "Proceed?".into(),
        options: vec![],
        worktree_path: None,
        multi_select: false,
        owner_approval: None,
        permission_request: None,
    })
    .await
    .unwrap();
    assert_eq!(counts(&pool, parent_id).await, (1, 1));

    let hold_id = Uuid::new_v4();
    emit(ThreadEvent::ThreadHoldPlaced {
        reason: "Taken over".into(),
        hold_id,
        root_thread_id: parent_id,
    })
    .await
    .unwrap();
    assert_eq!(
        counts(&pool, parent_id).await,
        (1, 0),
        "the hold drops attention only"
    );
    assert_eq!(
        rebuilt(&pool, parent_id).await,
        (1, 0),
        "the rebuild agrees"
    );

    emit(ThreadEvent::ThreadHoldLifted {
        hold_id,
        cause: crate::engine::thread_events::HoldLiftCause::Resume,
    })
    .await
    .unwrap();
    assert_eq!(
        counts(&pool, parent_id).await,
        (1, 1),
        "lifting the hold restores attention"
    );
    assert_eq!(
        rebuilt(&pool, parent_id).await,
        (1, 1),
        "the rebuild agrees"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn cc_permission_lifecycle_propagates_attention_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_with_idle_cc_child(&bus, &pool).await;

    let request_id = Uuid::new_v4().to_string();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: request_id.clone(),
            tool_use_id: "tu-perm".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({}),
            summary: "Edit file".into(),
            grant_offer: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(
        read_attention_descendant_count(&pool, parent_id).await,
        1,
        "CodingAgentPermissionRequest → WFUA must bump the parent's \
         attention counter so Current-bubble surfaces the parent"
    );

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentPermissionResolved {
            request_id,
            allowed: true,
            reason: None,
            persist_scope: None,
            standing_deny: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(
        read_attention_descendant_count(&pool, parent_id).await,
        0,
        "CodingAgentPermissionResolved → Running drops the attention \
         counter — CC is back to delegated work"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn change_proposed_lifecycle_propagates_attention_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_with_idle_cc_child(&bus, &pool).await;
    emit_change_proposed(&bus, child_id, "claude-code/test", false).await;
    assert_eq!(
        read_attention_descendant_count(&pool, parent_id).await,
        1,
        "ChangeProposed on an in-workspace CC child bubbles attention to \
         the parent — user must Apply/Discard for the thread to settle"
    );

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(
        read_attention_descendant_count(&pool, parent_id).await,
        0,
        "ThreadArchived clears the CC proposal → attention drops to 0"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn mixed_siblings_attention_counts_only_wfua_not_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, running_child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, running_child_id).await;

    let wfua_child_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: wfua_child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "second child".into(),
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
    emit_cc_session_started(&bus, wfua_child_id).await;

    assert_eq!(read_blocking_descendant_count(&pool, parent_id).await, 2);
    assert_eq!(read_attention_descendant_count(&pool, parent_id).await, 0);

    let request_id = Uuid::new_v4().to_string();
    bus.emit(BusEvent::Thread {
        thread_id: wfua_child_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id,
            tool_use_id: "tu-mixed".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({}),
            summary: "Edit file".into(),
            grant_offer: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        2,
        "Both children still block the parent (one Running, one WFUA)"
    );
    assert_eq!(
        read_attention_descendant_count(&pool, parent_id).await,
        1,
        "Only the WFUA child counts as needing attention — Running sibling \
         doesn't bubble. This is the case that drives the parent to Current \
         in display_section despite the live sibling work."
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn three_level_tree_propagates_to_grandparent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let grandparent_id = Uuid::new_v4();
    let parent_id = Uuid::new_v4();
    let grandchild_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: grandparent_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "root task".into(),
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
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "mid task".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(grandparent_id),
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

    for tid in [parent_id, grandparent_id] {
        bus.emit(BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::ResponseGenerated {
                text: "settled".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }

    bus.emit(BusEvent::Thread {
        thread_id: grandchild_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "cc task".into(),
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
    emit_cc_session_started(&bus, grandchild_id).await;
    emit_cc_idle(&bus, grandchild_id, false, None).await;
    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        0,
        "baseline: idle grandchild gives parent count 0"
    );
    assert_eq!(
        read_blocking_descendant_count(&pool, grandparent_id).await,
        0,
        "baseline: idle grandchild gives grandparent count 0"
    );

    bus.emit(BusEvent::Thread {
        thread_id: grandchild_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "wake".into(),
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

    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        1,
        "parent count must bump after grandchild becomes Running"
    );
    assert_eq!(
        read_blocking_descendant_count(&pool, grandparent_id).await,
        1,
        "grandparent count must also bump — propagation walks the full ancestor chain"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn rebuild_recomputes_blocking_descendant_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let running_child = Uuid::new_v4();
    let idle_child = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "parent".into(),
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
        thread_id: running_child,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "running".into(),
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
    emit_cc_session_started(&bus, running_child).await;
    emit_cc_idle(&bus, running_child, false, None).await;
    bus.emit(BusEvent::Thread {
        thread_id: running_child,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "wake".into(),
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

    bus.emit(BusEvent::Thread {
        thread_id: idle_child,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "idle".into(),
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
    emit_cc_session_started(&bus, idle_child).await;
    emit_cc_idle(&bus, idle_child, false, None).await;

    assert_eq!(read_blocking_descendant_count(&pool, parent_id).await, 1);
    sqlx::query("UPDATE thread_summaries SET blocking_descendant_count = 99")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        99,
        "wipe-state sanity check"
    );

    EventBus::rebuild_blocking_descendant_count(&pool)
        .await
        .unwrap();

    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        1,
        "rebuild must recompute parent count to 1 (only running_child blocks)"
    );
    assert_eq!(
        read_blocking_descendant_count(&pool, running_child).await,
        0,
        "rebuild must reset leaf rows (no descendants) to 0"
    );
    assert_eq!(
        read_blocking_descendant_count(&pool, idle_child).await,
        0,
        "rebuild must reset leaf rows (no descendants) to 0"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn within_ten_seconds<F: std::future::Future>(
    pool: &PgPool,
    what: &str,
    fut: F,
) -> F::Output {
    match tokio::time::timeout(std::time::Duration::from_secs(10), fut).await {
        Ok(v) => v,
        Err(_) => {
            let _ = sqlx::query(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                 WHERE datname = current_database() AND pid <> pg_backend_pid()",
            )
            .execute(pool)
            .await;
            panic!("{what} did not finish in 10s: its recursive CTE looped");
        }
    }
}

async fn insert_summary_row(
    pool: &PgPool,
    thread_id: Uuid,
    parent: Option<Uuid>,
    status: &str,
    is_coding_agent: bool,
) {
    sqlx::query(
        "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, \
              state, status, archive_state, is_coding_agent, parent_thread_id) \
         VALUES ($1, 'T', $5, 1, NOW(), TRUE, 'active', $2, 'inbox', $3, $4)",
    )
    .bind(thread_id)
    .bind(status)
    .bind(is_coding_agent)
    .bind(parent)
    .bind(if is_coding_agent {
        "claude_code"
    } else {
        "chat"
    })
    .execute(pool)
    .await
    .unwrap();
}

async fn poison_descendant_counts(pool: &PgPool) {
    sqlx::query(
        "UPDATE thread_summaries \
         SET blocking_descendant_count = 99, attention_descendant_count = 99",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn emit_change_discarded(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeDiscarded {
            change_id: format!("test-cid-{thread_id}"),
            actor: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn propagation_walks_terminate_on_a_parent_cycle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    insert_summary_row(&pool, a, Some(b), "running", true).await;
    insert_summary_row(&pool, b, Some(a), "running", true).await;

    within_ten_seconds(
        &pool,
        "rebuild_blocking_descendant_count",
        EventBus::rebuild_blocking_descendant_count(&pool),
    )
    .await
    .unwrap();

    within_ten_seconds(
        &pool,
        "propagate_blocking_change",
        emit_cc_idle(&bus, a, false, None),
    )
    .await;

    within_ten_seconds(
        &pool,
        "reconcile_proposal_lifecycle_end",
        emit_change_discarded(&bus, a),
    )
    .await;

    let solo = Uuid::new_v4();
    insert_summary_row(&pool, solo, None, "running", true).await;
    sqlx::query("UPDATE thread_summaries SET parent_thread_id = $1 WHERE thread_id = $1")
        .bind(solo)
        .execute(&pool)
        .await
        .unwrap();
    within_ten_seconds(
        &pool,
        "rebuild_blocking_descendant_count on a self-parented row",
        EventBus::rebuild_blocking_descendant_count(&pool),
    )
    .await
    .unwrap();

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn dedup_still_counts_every_descendant_of_an_acyclic_tree() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    let mid = Uuid::new_v4();
    let leaf_a = Uuid::new_v4();
    let leaf_b = Uuid::new_v4();
    let leaf_c = Uuid::new_v4();
    insert_summary_row(&pool, root, None, "idle", false).await;
    insert_summary_row(&pool, mid, Some(root), "idle", false).await;
    insert_summary_row(&pool, leaf_a, Some(mid), "running", true).await;
    insert_summary_row(&pool, leaf_b, Some(mid), "running", true).await;
    insert_summary_row(&pool, leaf_c, Some(root), "waiting_for_user_answer", true).await;
    poison_descendant_counts(&pool).await;

    EventBus::rebuild_blocking_descendant_count(&pool)
        .await
        .unwrap();

    assert_eq!(
        read_blocking_descendant_count(&pool, root).await,
        3,
        "root blocks on leaf_a, leaf_b and leaf_c; mid is idle"
    );
    assert_eq!(
        read_attention_descendant_count(&pool, root).await,
        1,
        "only leaf_c waits on the user; running is not attention"
    );
    assert_eq!(
        read_blocking_descendant_count(&pool, mid).await,
        2,
        "mid blocks on its two running children, counted once each"
    );
    assert_eq!(read_attention_descendant_count(&pool, mid).await, 0);
    for leaf in [leaf_a, leaf_b, leaf_c] {
        assert_eq!(read_blocking_descendant_count(&pool, leaf).await, 0);
        assert_eq!(read_attention_descendant_count(&pool, leaf).await, 0);
    }

    emit_change_discarded(&bus, leaf_a).await;

    assert_eq!(
        read_blocking_descendant_count(&pool, mid).await,
        1,
        "reconcile drops the idled leaf_a, keeps leaf_b"
    );
    assert_eq!(
        read_blocking_descendant_count(&pool, root).await,
        2,
        "reconcile reaches the grandparent, counting leaf_b and leaf_c"
    );
    assert_eq!(
        read_attention_descendant_count(&pool, root).await,
        1,
        "leaf_c still waits on the user"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
