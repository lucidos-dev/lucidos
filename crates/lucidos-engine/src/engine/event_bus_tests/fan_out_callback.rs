use super::super::*;
use super::*;

#[tokio::test]
async fn test_fan_out_parent_callback() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();

    // Parent thread
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "do three things".into(),
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

    // Three children with parent_thread_id
    for (i, &cid) in child_ids.iter().enumerate() {
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::MessageReceived {
                text: format!("task {}", i + 1),
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
    }

    // Verify projection — all 3 children have correct parent_thread_id
    let children: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT thread_id FROM thread_summaries WHERE parent_thread_id = $1 ORDER BY thread_id",
    )
    .bind(parent_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(children.len(), 3, "should have 3 children in projection");

    // Verify parent has no parent_thread_id
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

    // Children complete — emit ResponseGenerated THEN CodingAgentIdled for each
    // (mirrors real CC flow). Only CodingAgentIdled should trigger the callback
    // because these are CC threads (source = 'claude_code').
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
    }

    // Verify exactly 3 callbacks (not 6) — ResponseGenerated is skipped for CC threads
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
        // Phase-4 child-completion-card refactor: ParentCallback no longer
        // carries a synthesized `wake_text` chat bubble. The structured
        // outcome (status, title, summary, pending changes) lives in the
        // typed `ChildThreadCompleted` event already persisted on the
        // parent's history; the callback just carries that event id so the
        // resume path can attribute the parent LLM's response back to it
        // (request_event_id → response panel of the same exchange — see
        // docs/plans/2026-05-12-child-completion-card-design.md). Assert
        // the linkage exists and the typed event is on the parent.
        assert_ne!(
            cb.child_completed_event_id,
            uuid::Uuid::nil(),
            "ParentCallback must carry the typed event id"
        );
    }

    // Verify each callback references a different child
    let mut callback_children: Vec<Uuid> = callbacks.iter().map(|cb| cb.child_thread_id).collect();
    callback_children.sort();
    let mut expected_children = child_ids.clone();
    expected_children.sort();
    assert_eq!(
        callback_children, expected_children,
        "each callback should reference a different child thread"
    );

    // Verify the typed ChildThreadCompleted events were actually persisted
    // on the parent — every callback carries the event id of one of these
    // rows, so a missing row would mean the parent's resume path attributes
    // its response to a phantom card.
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

    // Each callback's child_completed_event_id must be a real persisted
    // event row on the parent — the resume path stamps it as
    // `request_event_id` on the parent LLM's response, so a phantom id
    // would have the response panel grouping under nothing.
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

    // Phase 4 child-completion-card refactor: the wake path must NOT
    // persist a synthetic MessageReceived ("Child thread X completed.
    // See the [CHILD THREAD COMPLETED] block in your conversation
    // history") on the parent — that bubble is what the typed event +
    // ChildCompletionCard replaces. The parent gets exactly one
    // MessageReceived (the original "do three things") plus the three
    // typed ChildThreadCompleted rows; no extra MessageReceived per
    // wake-up.
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

    // Spot-check one row's payload for the expected typed shape: status,
    // child_thread_id, and pending_change_ids must round-trip the event.
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
    // `pending_change_ids` is `skip_serializing_if = "Vec::is_empty"` (chat
    // children and no-changes CC idles produce []), so just assert the
    // shape: when present, it's an array. The CC-with-changes path is
    // exercised by the changes-projection-aware integration tests.
    if let Some(v) = payload.get("pending_change_ids") {
        assert!(
            v.is_array(),
            "pending_change_ids must be an array when present, got {:?}",
            v
        );
    }

    // Cleanup
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Three chat children all complete with ResponseGenerated.
/// Verify all 3 callbacks are received (no lost callbacks).
#[tokio::test]
async fn test_fan_out_chat_children_all_report_back() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();

    // Parent thread
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "research crypto sectors".into(),
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

    // Three chat children with parent_thread_id
    for (i, &cid) in child_ids.iter().enumerate() {
        bus.emit(BusEvent::Thread {
            thread_id: cid,
            event: ThreadEvent::MessageReceived {
                text: format!("research sector {}", i + 1),
                user_image_hashes: vec![],
                device_id: None,
                device: None,
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

    // Verify parent has 3 active children
    assert_active_children(&pool, parent_id, 3, "parent should have 3 active children").await;

    // Children complete one by one with ResponseGenerated
    for (i, &cid) in child_ids.iter().enumerate() {
        // Emit a ResponseGenerated for the child (this is its response text)
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

    // Verify all 3 callbacks were received
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

    // Verify each callback references a different child
    let mut callback_children: Vec<Uuid> = callbacks.iter().map(|cb| cb.child_thread_id).collect();
    callback_children.sort();
    let mut expected_children = child_ids.clone();
    expected_children.sort();
    assert_eq!(
        callback_children, expected_children,
        "each callback should reference a different child thread"
    );

    // Verify parent's active_children_count is 0 after all children completed
    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent should have 0 active children after all completed",
    )
    .await;

    // Cleanup
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// `relation: "top"` on `run_thread` / `run_claude` produces a top-thread:
/// the spawned thread carries `parent_thread_id = NULL` in its
/// MessageReceived. `notify_parent_if_child` must early-return on the NULL
/// projection lookup, so the fan-out callback channel must stay empty and
/// the (would-be) spawning thread's `active_children_count` must stay at
/// zero. This is the projection-side contract that makes top-relation
/// fire-and-forget — without it, top spawns would silently behave like
/// child spawns whenever the wiring layer regressed.
#[tokio::test]
async fn test_top_relation_thread_does_not_callback_or_increment_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let spawning_id = Uuid::new_v4();
    let top_thread_id = Uuid::new_v4();

    // Spawning thread (the one that "would have been" the parent).
    bus.emit(BusEvent::Thread {
        thread_id: spawning_id,
        event: ThreadEvent::MessageReceived {
            text: "kick off some research".into(),
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

    // Top-thread the agent spawns: parent_thread_id is None on purpose.
    bus.emit(BusEvent::Thread {
        thread_id: top_thread_id,
        event: ThreadEvent::MessageReceived {
            text: "do the research independently".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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

    // Top-thread reaches a terminal state. Sub-thread regression test
    // covers the "callback fires" path; here we assert the inverse: no
    // callback for a thread with NULL parent.
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

/// CC children that end via SessionEnded without ever emitting CodingAgentIdled
/// (e.g., crash, shutdown, user-ended) should still send a callback to the parent
/// and decrement active_children_count.
#[tokio::test]
async fn test_cc_child_session_ended_without_idle_sends_callback() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // Parent thread
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "research something".into(),
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

    // CC child with parent_thread_id
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "do subtask".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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

    // Mark child as CC via SessionStarted
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "cc-session-1".into(),
            branch: String::new(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    // CC child ends via SessionEnded WITHOUT ever emitting CodingAgentIdled
    // (simulates crash/shutdown/user-ended scenario). Phase 4 leaves only
    // terminal-only reasons; Panic stands in for the prior `Completed` here.
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

    // Verify callback was received
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

    // Verify parent's active_children_count decremented to 0
    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent should have 0 active children after SessionEnded",
    )
    .await;

    // Cleanup
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// CC children that DO emit CodingAgentIdled should NOT send a duplicate callback
/// when SessionEnded fires afterward.
#[tokio::test]
async fn test_cc_child_no_duplicate_callback_after_idle_then_session_ended() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // Parent thread
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "research something".into(),
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

    // CC child with parent_thread_id
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "do subtask".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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

    // Mark child as CC
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "cc-session-2".into(),
            branch: String::new(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    assert_active_children(&pool, parent_id, 1, "parent should have 1 active child").await;

    // CC child idles normally (this sends callback + decrements)
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

    assert_active_children(&pool, parent_id, 0, "parent should have 0 after idle").await;

    // Drain the callback from CodingAgentIdled
    let mut callbacks = vec![];
    while let Ok(cb) = callback_rx.try_recv() {
        callbacks.push(cb);
    }
    assert_eq!(
        callbacks.len(),
        1,
        "should have exactly 1 callback from idle"
    );

    // Now SessionEnded fires (terminal: thread closed/shutdown after idle)
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

    // Should NOT get a second callback
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

    // active_children_count should still be 0 (not -1)
    assert_active_children(
        &pool,
        parent_id,
        0,
        "parent should still have 0 after SessionEnded",
    )
    .await;

    // Cleanup
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression for the parent-callback duplicate window: the
/// `parent_callback_sent` marker must be written by the projection of the
/// `ChildThreadCompleted` emit itself — in the same transaction as the event
/// INSERT — not by a separate post-emit UPDATE. The old shape (emit, then a
/// standalone `mark_parent_callback_sent`) left a crash window where the
/// typed event committed but the marker didn't; the next terminal event then
/// re-fired the whole fan-in and handed the parent a duplicate completion
/// card. Emitting the typed event (exactly as `notify_parent_if_child` does)
/// must therefore be sufficient on its own to flip the marker.
#[tokio::test]
async fn test_child_thread_completed_projection_marks_callback_sent_in_tx() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;

    let sent: bool = sqlx::query_scalar(
        "SELECT parent_callback_sent FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!sent, "baseline: parent_callback_sent starts FALSE");

    // Emit the typed fan-in event onto the parent thread, exactly as
    // notify_parent_if_child does after a child terminal event.
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::ChildThreadCompleted {
            child_thread_id: child_id,
            child_thread_title: Some("child task".into()),
            status: crate::engine::thread_events::ChildCompletionStatus::Success,
            summary: "done".into(),
            pending_change_ids: vec![],
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let sent: bool = sqlx::query_scalar(
        "SELECT parent_callback_sent FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        sent,
        "ChildThreadCompleted projection must set parent_callback_sent=TRUE \
         in the same tx as the event insert — a post-emit UPDATE leaves a \
         crash window that duplicates the parent's completion card"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
