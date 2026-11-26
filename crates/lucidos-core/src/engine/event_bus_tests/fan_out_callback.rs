use super::super::parent_callback::ParentWake;
use super::super::*;
use super::*;

#[tokio::test]
async fn test_fan_out_parent_callback() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "do three things".into(),
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

    for (i, &cid) in child_ids.iter().enumerate() {
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: format!("task {}", i + 1),
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
    }

    let children: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT thread_id FROM thread_summaries WHERE parent_thread_id = $1 ORDER BY thread_id",
    )
    .bind(parent_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(children.len(), 3, "should have 3 children in projection");

    let parent_parent: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT parent_thread_id FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(
        parent_parent,
        Some(None),
        "parent thread should have no parent_thread_id"
    );

    for &cid in &child_ids {
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::ResponseGenerated {
                text: "Done.".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::CodingAgentIdled {
                has_changes: true,
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
    }

    let mut callbacks = vec![];
    while let Ok(cb) = callback_rx.try_recv() {
        callbacks.push(cb);
    }
    assert_eq!(
        callbacks.len(),
        3,
        "should have 3 parent callbacks, not 6 (no double-reporting)"
    );
    for cb in &callbacks {
        assert_eq!(cb.parent_thread_id, parent_id);
        assert_ne!(
            cb.child_completed_event_id,
            uuid::Uuid::nil(),
            "ParentCallback must carry the typed event id"
        );
    }

    let mut callback_children: Vec<Uuid> = callbacks.iter().map(|cb| cb.child_thread_id).collect();
    callback_children.sort();
    let mut expected_children = child_ids.clone();
    expected_children.sort();
    assert_eq!(
        callback_children, expected_children,
        "each callback should reference a different child thread"
    );

    let cc_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ChildThreadCompleted'"
    )
    .bind(parent_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        cc_count, 3,
        "expected 3 ChildThreadCompleted events on parent (one per child)"
    );

    for cb in &callbacks {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(\
                SELECT 1 FROM events \
                WHERE id = $1 AND event_type = 'ChildThreadCompleted' AND aggregate_id = $2\
            )",
        )
        .bind(cb.child_completed_event_id)
        .bind(parent_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            exists,
            "ParentCallback carries event id {} which must be a persisted ChildThreadCompleted row on parent {}",
            cb.child_completed_event_id, parent_id
        );
    }

    let parent_mr_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'MessageReceived'",
    )
    .bind(parent_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        parent_mr_count, 1,
        "wake path must NOT persist a synthetic MessageReceived on the parent; \
         expected exactly the original prompt's MR, got {}",
        parent_mr_count
    );

    let row: (serde_json::Value,) = sqlx::query_as(
        "SELECT payload FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ChildThreadCompleted' \
         ORDER BY created LIMIT 1",
    )
    .bind(parent_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    let payload = row.0;
    let status = payload
        .get("status")
        .and_then(|v| v.as_str())
        .expect("status field present");
    assert!(
        matches!(status, "success" | "failure" | "no_changes"),
        "status must be one of the typed enum variants, got {:?}",
        status
    );
    let child_id_str = payload
        .get("child_thread_id")
        .and_then(|v| v.as_str())
        .expect("child_thread_id field present");
    let parsed: Uuid = child_id_str
        .parse()
        .expect("child_thread_id is a UUID string");
    assert!(
        child_ids.contains(&parsed),
        "child_thread_id must match one of the spawned children"
    );
    if let Some(v) = payload.get("pending_change_ids") {
        assert!(
            v.is_array(),
            "pending_change_ids must be an array when present, got {:?}",
            v
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_fan_out_chat_children_all_report_back() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "research crypto sectors".into(),
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

    for (i, &cid) in child_ids.iter().enumerate() {
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: format!("research sector {}", i + 1),
                user_image_hashes: vec![],
                device_id: None,
                image_description: None,
                parent_thread_id: Some(parent_id),
                spawning_event_id: None,
                mode: ActorMode::Agent,
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
    }

    assert_active_children(&pool, parent_id, 3, "parent should have 3 active children").await;

    for (i, &cid) in child_ids.iter().enumerate() {
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::ResponseGenerated {
                text: format!("Sector {} analysis complete.", i + 1),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }

    let mut callbacks = vec![];
    while let Ok(cb) = callback_rx.try_recv() {
        callbacks.push(cb);
    }
    assert_eq!(
        callbacks.len(),
        3,
        "all 3 chat children should report back via callback, got {}",
        callbacks.len()
    );

    let mut callback_children: Vec<Uuid> = callbacks.iter().map(|cb| cb.child_thread_id).collect();
    callback_children.sort();
    let mut expected_children = child_ids.clone();
    expected_children.sort();
    assert_eq!(
        callback_children, expected_children,
        "each callback should reference a different child thread"
    );

    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent should have 0 active children after all completed",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_top_relation_thread_does_not_callback_or_increment_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let spawning_id = Uuid::new_v4();
    let top_thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: spawning_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "kick off some research".into(),
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
        thread_id: top_thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "do the research independently".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Agent,
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

    assert_active_children(
        &pool,
        spawning_id,
        0,
        "top-thread spawn must not increment the spawning thread's active_children_count",
    )
    .await;

    bus.emit(BusEvent::Thread {
        thread_id: top_thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "all done — independent thread.".into(),
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
        "top-thread terminal must not produce a parent callback (got {} callbacks)",
        callbacks.len()
    );

    assert_active_children(
        &pool,
        spawning_id,
        0,
        "spawning thread's active_children_count must stay 0 after top-thread terminates",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_cc_child_session_ended_without_idle_sends_callback() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "research something".into(),
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
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "do subtask".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Agent,
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
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            session_id: "cc-session-1".into(),
            branch: String::new(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionEnded {
            reason: SessionEndReason::Panic,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let mut callbacks = vec![];
    while let Ok(cb) = callback_rx.try_recv() {
        callbacks.push(cb);
    }
    assert_eq!(
        callbacks.len(),
        1,
        "CC child ending via SessionEnded (no prior idle) should send callback, got {}",
        callbacks.len()
    );
    assert_eq!(callbacks[0].child_thread_id, child_id);
    assert_eq!(callbacks[0].parent_thread_id, parent_id);

    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent should have 0 active children after SessionEnded",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_cc_child_no_duplicate_callback_after_idle_then_session_ended() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "research something".into(),
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
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "do subtask".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Agent,
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
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            session_id: "cc-session-2".into(),
            branch: String::new(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

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

    assert_active_children(&pool, parent_id, 0, "parent should have 0 after idle").await;

    let mut callbacks = vec![];
    while let Ok(cb) = callback_rx.try_recv() {
        callbacks.push(cb);
    }
    assert_eq!(
        callbacks.len(),
        1,
        "should have exactly 1 callback from idle"
    );

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionEnded {
            reason: SessionEndReason::Shutdown,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let mut extra_callbacks = vec![];
    while let Ok(cb) = callback_rx.try_recv() {
        extra_callbacks.push(cb);
    }
    assert_eq!(
        extra_callbacks.len(),
        0,
        "SessionEnded after CodingAgentIdled should NOT send duplicate callback, got {}",
        extra_callbacks.len()
    );

    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent should still have 0 after SessionEnded",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn refire_reinjects_unprocessed_child_completion_after_restart() {
    let (pool, db_name) = setup_test_db().await;

    let (bus1, mut rx1) = EventBus::new(pool.clone());
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    start_cc_session(&bus1, parent_id, "claude-code/parent", None).await;
    bus1.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "do subtask".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Agent,
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
    emit_cc_session_started(&bus1, child_id).await;

    bus1.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
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

    let mut live = vec![];
    while let Ok(cb) = rx1.try_recv() {
        live.push(cb);
    }
    assert_eq!(
        live.len(),
        1,
        "the live wake must have fired before the restart"
    );
    let card_event_id = live[0].child_completed_event_id;
    drop(rx1);
    drop(bus1);

    let (bus2, mut rx2) = EventBus::new(pool.clone());
    let refired = bus2.refire_unprocessed_child_completions().await;
    assert_eq!(
        refired, 1,
        "the boot sweep must re-fire exactly the one stranded wake"
    );

    let mut recovered = vec![];
    while let Ok(cb) = rx2.try_recv() {
        recovered.push(cb);
    }
    assert_eq!(
        recovered.len(),
        1,
        "the re-fired wake must land on the fresh callback channel"
    );
    let cb = &recovered[0];
    assert_eq!(cb.parent_thread_id, parent_id);
    assert_eq!(cb.child_thread_id, child_id);
    assert!(
        cb.parent_is_coding_agent,
        "parent is a CC thread — the re-fired wake must carry that so the resume routes correctly"
    );
    assert_eq!(
        cb.child_completed_event_id, card_event_id,
        "a re-fired callback must anchor to the SAME persisted ChildThreadCompleted event"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn refire_skips_parent_that_already_resumed() {
    let (pool, db_name) = setup_test_db().await;

    let (bus1, mut rx1) = EventBus::new(pool.clone());
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    start_cc_session(&bus1, parent_id, "claude-code/parent-resumed", None).await;
    bus1.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "do subtask".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Agent,
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
    emit_cc_session_started(&bus1, child_id).await;
    bus1.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
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

    bus1.emit(BusEvent::Thread {
        thread_id: parent_id,
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
    while rx1.try_recv().is_ok() {}
    drop(rx1);
    drop(bus1);

    let (bus2, mut rx2) = EventBus::new(pool.clone());
    let refired = bus2.refire_unprocessed_child_completions().await;
    assert_eq!(
        refired, 0,
        "a parent whose resume already emitted a later terminal must not be re-fired"
    );
    assert!(
        rx2.try_recv().is_err(),
        "no wake should land on the channel for an already-resumed parent"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_child_thread_completed_projection_clears_pending_in_tx() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;

    let pending: bool = sqlx::query_scalar(
        "SELECT parent_callback_pending FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        pending,
        "baseline: a freshly spawned child owes its parent a card"
    );

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::ChildThreadCompleted {
            child_thread_id: child_id,
            child_thread_title: Some("child task".into()),
            status: crate::engine::thread_events::ChildCompletionStatus::Success,
            summary: "done".into(),
            pending_change_ids: vec![],
            sub_thread_pending_changes: vec![],
            integrated_branch: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let pending: bool = sqlx::query_scalar(
        "SELECT parent_callback_pending FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !pending,
        "ChildThreadCompleted projection must clear parent_callback_pending \
         in the same tx as the event insert: a post-emit UPDATE leaves a \
         crash window that duplicates the parent's completion card"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn fresh_coding_agent_child_reports_its_first_completion() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "baseline: one child in flight").await;

    emit_cc_idle(&bus, child_id, false, None).await;

    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        1,
        "a coding-agent child's FIRST idle must produce exactly one \
         ChildThreadCompleted on the parent"
    );
    let mut callbacks = 0;
    while callback_rx.try_recv().is_ok() {
        callbacks += 1;
    }
    assert_eq!(callbacks, 1, "and exactly one parent wake");
    assert_active_children(
        &pool,
        parent_id,
        0,
        "and the parent's count must come back down",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_childs_cache_keep_alive_turn_reports_nothing_to_the_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, false, None).await;
    while callback_rx.try_recv().is_ok() {}
    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        1,
        "baseline"
    );

    bus.mark_keep_alive_turn(child_id, true).await;
    let anchor = ThreadEvent::PromptInjected {
        text: "keep warm".into(),
        mode: ActorMode::Engine,
        origin: Some(crate::engine::thread_events::MessageOrigin::engine(
            crate::engine::thread_events::EngineReason::CacheKeepAlive,
        )),
        injected_message_id: None,
        delivered_event_id: None,
    };
    for event in [
        anchor,
        ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            reason: Some(crate::engine::thread_events::KEEP_ALIVE_IDLE_REASON.to_string()),
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        },
    ] {
        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }

    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        1,
        "a keep-alive turn adds no completion card"
    );
    assert!(callback_rx.try_recv().is_err(), "and wakes no parent");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn read_status(pool: &PgPool, thread_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_child_completion_wakes_its_parent_into_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::ResponseGenerated {
            text: "spawned it".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(
        read_status(&pool, parent_id).await.as_deref(),
        Some("idle"),
        "precondition: the parent is quiescent before its child completes"
    );

    emit_cc_idle(&bus, child_id, false, None).await;

    assert_eq!(
        read_status(&pool, parent_id).await.as_deref(),
        Some("running"),
        "the completion card must open the parent's exchange, so the parent \
         reads `running` from the moment the card commits"
    );
    assert!(
        callback_rx.try_recv().is_ok(),
        "and the wake it stands for is really on its way"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_wake_never_overwrites_a_parent_parked_on_a_question() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: "tu-1".into(),
            cc_session_id: "sess-1".into(),
            question: "which approach?".into(),
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
        read_status(&pool, parent_id).await.as_deref(),
        Some("waiting_for_user_answer"),
        "precondition: the parent is parked on its question"
    );

    emit_cc_idle(&bus, child_id, false, None).await;

    assert_eq!(
        read_status(&pool, parent_id).await.as_deref(),
        Some("waiting_for_user_answer"),
        "the wake must not demote a parked parent: the question is still \
         unanswered, and `running` would put out its needs-attention badge"
    );
    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        1,
        "the card still lands, only the status is preserved"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn undo_parent_wake_restores_the_previous_status_and_fences_on_a_revive() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::ResponseGenerated {
            text: "spawned it".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    emit_cc_idle(&bus, child_id, false, None).await;
    assert!(callback_rx.try_recv().is_ok());
    let (status, revived_at) = read_wake(&pool, parent_id).await;
    assert_eq!(status, "running", "precondition: the parent is awake");

    sqlx::query("UPDATE thread_summaries SET last_revived_at = NOW() WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();
    let stale = ParentWake {
        prev_status: "idle".into(),
        prev_revived_at: None,
        revived_at,
    };
    EventBus::undo_parent_wake(&pool, parent_id, Some(&stale)).await;
    assert_eq!(
        read_status(&pool, parent_id).await.as_deref(),
        Some("running"),
        "a turn that started after the wake keeps the row: the rollback is \
         fenced on the stamp it wrote, and that stamp is gone"
    );

    let (_, current_stamp) = read_wake(&pool, parent_id).await;
    let fresh = ParentWake {
        prev_status: "idle".into(),
        prev_revived_at: None,
        revived_at: current_stamp,
    };
    EventBus::undo_parent_wake(&pool, parent_id, Some(&fresh)).await;
    assert_eq!(
        read_status(&pool, parent_id).await.as_deref(),
        Some("idle"),
        "no turn will run, so the parent must not keep spinning"
    );

    EventBus::undo_parent_wake(&pool, parent_id, None).await;
    assert_eq!(read_status(&pool, parent_id).await.as_deref(), Some("idle"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_wake_reports_the_status_it_replaced() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    emit_thread_message(&bus, parent_id, None, "parent work").await;
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
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
    assert_eq!(read_status(&pool, parent_id).await.as_deref(), Some("idle"));

    let wake = bus
        .update_parent_after_child_terminal(parent_id)
        .await
        .expect("the parent row exists, so the wake lands");

    assert_eq!(
        wake.prev_status, "idle",
        "the wake must carry the status it replaced, since that is what the \
         rollback puts back"
    );
    assert_eq!(
        read_status(&pool, parent_id).await.as_deref(),
        Some("running"),
        "and the row itself is now awake"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn two_failed_wakes_on_one_parent_unwind_completely() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    emit_thread_message(&bus, parent_id, None, "parent work").await;
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
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
    assert_eq!(read_status(&pool, parent_id).await.as_deref(), Some("idle"));

    let first = bus
        .update_parent_after_child_terminal(parent_id)
        .await
        .expect("first wake");
    let second = bus
        .update_parent_after_child_terminal(parent_id)
        .await
        .expect("second wake");
    assert_eq!(first.prev_status, "idle");
    assert_eq!(
        second.prev_status, "running",
        "the second wake sees what the first one wrote"
    );

    EventBus::undo_parent_wake(&pool, parent_id, Some(&second)).await;
    EventBus::undo_parent_wake(&pool, parent_id, Some(&first)).await;

    assert_eq!(
        read_status(&pool, parent_id).await.as_deref(),
        Some("idle"),
        "neither turn will run, so the parent must end where it started"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn two_failed_wakes_unwinding_oldest_first_leave_the_boot_reset_to_finish() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    emit_thread_message(&bus, parent_id, None, "parent work").await;
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
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

    let first = bus
        .update_parent_after_child_terminal(parent_id)
        .await
        .expect("first wake");
    let second = bus
        .update_parent_after_child_terminal(parent_id)
        .await
        .expect("second wake");

    EventBus::undo_parent_wake(&pool, parent_id, Some(&first)).await;
    EventBus::undo_parent_wake(&pool, parent_id, Some(&second)).await;

    assert_eq!(
        read_status(&pool, parent_id).await.as_deref(),
        Some("running"),
        "the documented bound: this order needs the boot reset to finish, \
         which is what `main.rs` does for every orphaned `running`"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn read_wake(pool: &PgPool, thread_id: Uuid) -> (String, chrono::DateTime<chrono::Utc>) {
    sqlx::query_as("SELECT status, last_revived_at FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn read_callback_pending(pool: &PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT parent_callback_pending FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn top_level_thread_never_marks_a_pending_callback() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "top-level work").await;
    assert!(
        !read_callback_pending(&pool, thread_id).await,
        "a thread with no parent has no parent callback pending after its \
         first MessageReceived"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
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
    assert!(
        !read_callback_pending(&pool, thread_id).await,
        "nor after a terminal"
    );

    emit_thread_message(&bus, thread_id, None, "more work").await;
    assert!(
        !read_callback_pending(&pool, thread_id).await,
        "nor after a second MessageReceived, which routes through the revive \
         helper: its `parent_thread_id IS NOT NULL` conjunct keeps the write \
         off a parentless row"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn terminal_abort_clears_the_pending_callback() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert!(
        read_callback_pending(&pool, child_id).await,
        "baseline: the spawned child owes its parent a card"
    );

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::SafetyNet,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_active_children(&pool, parent_id, 0, "a terminal abort decrements").await;
    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        0,
        "a terminal abort sends no card by design"
    );
    assert!(callback_rx.try_recv().is_err(), "and no wake either");
    assert!(
        !read_callback_pending(&pool, child_id).await,
        "an abort owes the parent nothing further, so the marker settles to \
         FALSE until the next start event"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn missing_parent_row_leaves_the_callback_pending() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();

    emit_cc_idle(&bus, child_id, false, None).await;

    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        1,
        "the typed card is persisted before the parent row is inspected"
    );
    assert!(
        callback_rx.try_recv().is_err(),
        "but no wake is sent for a parent whose row is missing"
    );
    assert!(
        read_callback_pending(&pool, child_id).await,
        "the card exists and the wake does not, so the parent callback is \
         still pending and the next terminal must retry"
    );
    assert_eq!(
        read_status(&pool, parent_id).await,
        None,
        "waking a parent whose row is gone must write nothing at all"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_coding_agent_redirect_does_not_report_a_cancellation_to_the_parent() {
    use crate::engine::thread_events::CancelCause;
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    for cause in [
        CancelCause::SupersededByFollowup,
        CancelCause::EffortRestart,
    ] {
        let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
        emit_cc_session_started(&bus, child_id).await;

        emit_response_canceled_with_cause(&bus, child_id, cause).await;
        emit_cc_idled(&bus, child_id, false, None).await;

        assert_eq!(
            count_completion_cards(&pool, parent_id).await,
            0,
            "a {cause:?} cancel and its idle are a redirect, not a completion: \
             they must persist no ChildThreadCompleted on the parent"
        );
        assert!(
            callback_rx.try_recv().is_err(),
            "a {cause:?} cancel must not wake the parent"
        );
        assert_active_children(
            &pool,
            parent_id,
            1,
            "the redirected child is still in flight for the parent",
        )
        .await;
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_chat_child_redirect_does_not_report_a_cancellation_to_the_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;

    emit_response_canceled_with_cause(
        &bus,
        child_id,
        crate::engine::thread_events::CancelCause::SupersededByFollowup,
    )
    .await;

    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        0,
        "an urgent follow-up preempting a Lucidos Agent child is a redirect, not a \
         completion: it must persist no ChildThreadCompleted on the parent"
    );
    assert!(
        callback_rx.try_recv().is_err(),
        "and it must not wake the parent, which would have it act on a child that \
         is about to run the redirected turn"
    );
    assert_active_children(
        &pool,
        parent_id,
        1,
        "the redirected chat child stays in flight for the parent too",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_redirect_keeps_the_parent_count_while_the_child_is_between_turns() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "the child is working").await;

    emit_response_canceled_with_cause(
        &bus,
        child_id,
        crate::engine::thread_events::CancelCause::SupersededByFollowup,
    )
    .await;
    assert_active_children(
        &pool,
        parent_id,
        1,
        "no dip: the redirected child is still in flight",
    )
    .await;
    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        0,
        "and no card, so the parent is not told its child was canceled"
    );

    emit_cc_message_received(&bus, child_id, None, "go the other way").await;
    assert_active_children(&pool, parent_id, 1, "the redirect started").await;
    assert!(
        read_callback_pending(&pool, child_id).await,
        "and the redirected turn owes the parent a card"
    );

    emit_cc_idle(&bus, child_id, false, None).await;
    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        1,
        "exactly one card for the whole redirect, describing the work the \
         parent actually asked for"
    );
    assert_active_children(&pool, parent_id, 0, "and the count lands at ground truth").await;
    let mut wakes = 0;
    while callback_rx.try_recv().is_ok() {
        wakes += 1;
    }
    assert_eq!(wakes, 1, "and exactly one wake");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn emit_api_drop_failure(bus: &EventBus, thread_id: Uuid) {
    emit_response_failed(bus, thread_id, API_DROP_ERROR).await;
}

const API_DROP_ERROR: &str =
    "API Error: Connection lost mid-response. The response above may be incomplete.";

async fn emit_response_failed(bus: &EventBus, thread_id: Uuid, error: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseFailed {
            error: error.into(),
            spend_cap: None,
            agent_exit: None,
            provider_error: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn read_completion_status(pool: &PgPool, parent_id: Uuid) -> Option<String> {
    sqlx::query_scalar(
        "SELECT payload->>'status' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ChildThreadCompleted' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(parent_id.to_string())
    .fetch_optional(pool)
    .await
    .unwrap()
    .flatten()
}

async fn emit_api_error_continuation(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ContinuationRequested {
            reason: crate::engine::continuation_reason::AUTO_RESUME_AFTER_API_ERROR_REASON
                .to_string(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_held_terminal_is_not_announced_and_the_resumed_turn_reports_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    bus.auto_resume_holds()
        .hold(child_id, API_DROP_ERROR.into());

    emit_api_drop_failure(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, false, None).await;

    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        0,
        "a terminal the engine is about to resume must not be announced as a completion: \
         a parent that believes it spawns a duplicate onto the same files"
    );
    assert!(
        callback_rx.try_recv().is_err(),
        "and the parent must not be woken to react to it"
    );
    assert!(
        read_callback_pending(&pool, child_id).await,
        "the child still owes its parent a card, so the marker must survive the hold: \
         clearing it would send the real terminal into the dedup guard"
    );

    emit_api_error_continuation(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "the resumed child is working again").await;

    bus.auto_resume_holds().release(child_id);
    emit_cc_idle(&bus, child_id, true, None).await;

    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        1,
        "exactly one card for the whole episode, at the real terminal"
    );
    assert_eq!(
        read_completion_status(&pool, parent_id).await.as_deref(),
        Some("success"),
        "and it describes what the child actually did, not the drop it survived"
    );
    let mut wakes = 0;
    while callback_rx.try_recv().is_ok() {
        wakes += 1;
    }
    assert_eq!(wakes, 1, "and exactly one wake");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_unheld_api_drop_reports_the_failure_to_the_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    emit_api_drop_failure(&bus, child_id).await;

    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        1,
        "a parked child MUST report: nothing else is going to move this thread"
    );
    assert_eq!(
        read_completion_status(&pool, parent_id).await.as_deref(),
        Some("failure"),
        "and it reports the failure honestly"
    );
    assert!(callback_rx.try_recv().is_ok(), "and wakes the parent");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_non_transient_failure_still_reports_immediately() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    emit_response_failed(&bus, child_id, "error_max_turns").await;

    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        1,
        "a deterministic failure is a real completion and reaches the parent at once"
    );
    assert_eq!(
        read_completion_status(&pool, parent_id).await.as_deref(),
        Some("failure"),
    );
    assert!(callback_rx.try_recv().is_ok(), "and wakes the parent");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_resume_that_never_happened_announces_the_terminal_it_withheld() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    bus.auto_resume_holds()
        .hold(child_id, API_DROP_ERROR.into());
    emit_api_drop_failure(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, false, None).await;
    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        0,
        "withheld while the hold stands"
    );

    let withheld = bus.auto_resume_holds().release(child_id).unwrap();
    bus.announce_withheld_completion(child_id, withheld).await;

    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        1,
        "a decision nobody actuated must still reach the parent"
    );
    assert_eq!(
        read_completion_status(&pool, parent_id).await.as_deref(),
        Some("failure"),
        "reported as the failure it was, not as a completed turn"
    );
    let summary: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'summary' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ChildThreadCompleted' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(parent_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        summary.as_deref(),
        Some(API_DROP_ERROR),
        "carrying the error the hold kept, so the parent learns what went wrong"
    );
    assert!(callback_rx.try_recv().is_ok(), "and wakes the parent");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_hold_on_one_child_does_not_silence_its_sibling() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, held_child) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, held_child).await;
    let sibling = Uuid::new_v4();
    emit_cc_message_received(&bus, sibling, Some(parent_id), "the other half").await;
    emit_cc_session_started(&bus, sibling).await;

    bus.auto_resume_holds()
        .hold(held_child, API_DROP_ERROR.into());
    emit_api_drop_failure(&bus, held_child).await;
    emit_api_drop_failure(&bus, sibling).await;

    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        1,
        "only the held child stays quiet; the sibling nobody is resuming reports"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn spawn_cc_grandchild_with_change(bus: &EventBus, parent_id: Uuid) -> (Uuid, Uuid) {
    let (thread_id, change_id) = (Uuid::new_v4(), Uuid::new_v4());
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "a milestone".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Agent,
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
    emit_cc_session_started(bus, thread_id).await;
    emit_cc_idled(bus, thread_id, true, None).await;
    emit_pending_change(bus, thread_id, change_id).await;
    (thread_id, change_id)
}

async fn emit_pending_change(bus: &EventBus, thread_id: Uuid, change_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            content: Default::default(),
            change_id: change_id.to_string(),
            description: Some("milestone work".into()),
            files: vec!["a.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: format!("branch-{thread_id}"),
            repo_root: "/tmp".into(),
            hardened: false,
            incomplete: false,
            set_aside: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn card_pending_change_ids(pool: &PgPool, parent_id: Uuid) -> Vec<String> {
    let ids: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'pending_change_ids' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ChildThreadCompleted'",
    )
    .bind(parent_id.to_string())
    .fetch_one(pool)
    .await
    .expect("the child's card reached its parent");
    ids.map(|v| serde_json::from_value(v).unwrap())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_completion_card_lists_the_change_proposed_before_the_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (late_parent, late_child) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, late_child).await;
    emit_cc_idle(&bus, late_child, false, None).await;
    emit_pending_change(&bus, late_child, Uuid::new_v4()).await;
    assert_eq!(
        card_pending_change_ids(&pool, late_parent).await,
        Vec::<String>::new(),
        "a change proposed after the idle misses the card"
    );

    let (parent, child) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child).await;
    let change_id = Uuid::new_v4();
    emit_pending_change(&bus, child, change_id).await;
    emit_cc_idle(&bus, child, false, None).await;
    assert_eq!(
        card_pending_change_ids(&pool, parent).await,
        vec![change_id.to_string()],
        "a change proposed before the idle is on the card"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_completion_card_carries_the_pending_changes_of_its_sub_threads() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (master, orchestrator) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, orchestrator).await;
    let (settled_thread, settled_change) =
        spawn_cc_grandchild_with_change(&bus, orchestrator).await;
    let (working_thread, working_change) =
        spawn_cc_grandchild_with_change(&bus, orchestrator).await;
    sqlx::query("UPDATE thread_summaries SET status = 'running' WHERE thread_id = $1")
        .bind(working_thread)
        .execute(&pool)
        .await
        .unwrap();

    emit_cc_idle(&bus, orchestrator, false, None).await;

    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ChildThreadCompleted'",
    )
    .bind(master.to_string())
    .fetch_one(&pool)
    .await
    .expect("the orchestrator's card reached the master");
    assert!(
        payload.get("pending_change_ids").is_none(),
        "the orchestrator's own branch holds nothing: {payload}"
    );
    let entries: Vec<crate::engine::thread_events::SubThreadPendingChange> =
        serde_json::from_value(payload["sub_thread_pending_changes"].clone())
            .expect("the card lists its sub-threads' changes");
    let summary: Vec<_> = entries
        .iter()
        .map(|e| (e.change_id, e.thread_id, e.thread_unsettled))
        .collect();
    assert_eq!(
        summary,
        vec![
            (settled_change, settled_thread, false),
            (working_change, working_thread, true),
        ]
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_completion_card_reaches_a_change_two_levels_down() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (master, orchestrator) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, orchestrator).await;
    let (middle, _) = spawn_cc_grandchild_with_change(&bus, orchestrator).await;
    let (deep_thread, deep_change) = spawn_cc_grandchild_with_change(&bus, middle).await;

    emit_cc_idle(&bus, orchestrator, false, None).await;

    let entries: serde_json::Value = sqlx::query_scalar(
        "SELECT payload->'sub_thread_pending_changes' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ChildThreadCompleted'",
    )
    .bind(master.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        entries.as_array().is_some_and(|all| all.iter().any(|e| {
            e["change_id"] == deep_change.to_string() && e["thread_id"] == deep_thread.to_string()
        })),
        "the great-grandchild's change is on the card: {entries}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
