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

/// `relation: "top"` on `run_thread` / `run_coding_agent` produces a top-thread:
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

/// ADR 0011, B1 (durability): a child completes, fires its in-memory
/// `ParentCallback`, and the engine restarts before the listener consumes it —
/// the wake is lost (the channel is recreated empty) but the
/// `ChildThreadCompleted` is durably persisted on the parent. The boot-recovery
/// sweep `refire_unprocessed_child_completions` must re-derive the lost wake from
/// the persisted event and re-inject it, so the parent still resumes.
#[tokio::test]
async fn refire_reinjects_unprocessed_child_completion_after_restart() {
    let (pool, db_name) = setup_test_db().await;

    // --- before restart: bus1 with its own callback channel ---
    let (bus1, mut rx1) = EventBus::new(pool.clone());
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // CC parent (top-level) + CC child spawned with parent_thread_id.
    start_cc_session(&bus1, parent_id, "claude-code/parent", None).await;
    bus1.emit(BusEvent::Thread {
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
    emit_cc_session_started(&bus1, child_id).await;

    // Child idles with changes → fires the live wake on rx1 + persists
    // ChildThreadCompleted on the parent.
    bus1.emit(BusEvent::Thread {
        thread_id: child_id,
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

    // The live wake fired — then it's "lost" to the restart (we drain rx1 and
    // drop the bus without the listener ever consuming it).
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

    // --- after restart: fresh bus, empty channel (the wake is gone) ---
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

/// ADR 0011, B1 idempotency: a parent that already reacted to the completion
/// (its resume emitted a later terminal event) must NOT be re-fired by the boot
/// sweep — the `ChildThreadCompleted` is no longer the thread's latest event.
/// Without this the sweep would re-resume a parent on every restart forever.
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
    emit_cc_session_started(&bus1, child_id).await;
    bus1.emit(BusEvent::Thread {
        thread_id: child_id,
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

    // The parent RESUMED and idled — a terminal event on the parent AFTER the
    // completion card. (The parent has no parent_thread_id, so this idle fires
    // no further callback of its own.)
    bus1.emit(BusEvent::Thread {
        thread_id: parent_id,
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
    while rx1.try_recv().is_ok() {}
    drop(rx1);
    drop(bus1);

    // Restart: the sweep must find nothing to do — the card is not the latest event.
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

/// Regression for the parent-callback duplicate window: the
/// `parent_callback_pending` marker must be written by the projection of the
/// `ChildThreadCompleted` emit itself, in the same transaction as the event
/// INSERT, not by a separate post-emit UPDATE. The old shape (emit, then a
/// standalone marker UPDATE) left a crash window where the
/// typed event committed but the marker didn't; the next terminal event then
/// re-fired the whole fan-in and handed the parent a duplicate completion
/// card. Emitting the typed event (exactly as `notify_parent_if_child` does)
/// must therefore be sufficient on its own to flip the marker.
///
/// The baseline assertion is also the first thing that fails if the spawn
/// branch of the `MessageReceived` arm forgets its explicit TRUE write:
/// `spawn_parent_child` goes through exactly that branch.
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

/// Count the persisted `ChildThreadCompleted` cards sitting on `parent_id`.
async fn count_completion_cards(pool: &PgPool, parent_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ChildThreadCompleted'",
    )
    .bind(parent_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Emit a `ResponseCanceled` with an explicit cause on `thread_id`.
async fn emit_response_canceled_with_cause(
    bus: &EventBus,
    thread_id: Uuid,
    cause: crate::engine::thread_events::CancelCause,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseCanceled {
            text: "partial work".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// The flip's one dangerous failure mode, and it is silent. Under
/// `parent_callback_pending` a freshly spawned child owes its parent a card,
/// which is TRUE, and the storage default is FALSE (a top-level thread owes
/// nothing). If the spawn branch of the `MessageReceived` arm does not write
/// the TRUE explicitly, a fresh coding-agent child sits at FALSE, the dedup
/// early-return in `notify_parent_if_child` matches its very first
/// `CodingAgentIdled`, and its first card is never sent. Nothing else fails:
/// every other fixture passes through the same default.
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

/// Read the child's `parent_callback_pending` marker.
async fn read_callback_pending(pool: &PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT parent_callback_pending FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The non-child default, asserted rather than assumed. A `DEFAULT TRUE` would
/// claim every top-level thread in the workspace owes some parent a card, so
/// the storage default stays FALSE and the TRUE is written explicitly, only
/// for a thread that has a parent.
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

/// The terminal-abort write, which no test read before. A coding-agent child
/// whose terminal is a crash (a terminal-cause `ResponseAborted`) decrements
/// the parent and deliberately sends no card: the user is already looking at
/// the child's error state. So nothing further is owed and the marker settles
/// to FALSE. That is correct, not a leak.
///
/// `clear_pending_parent_callback` logs and continues on a query error, so a
/// typo'd column name inside its SQL string produces a `[FanOut] Failed to …`
/// line and no other test failure.
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

/// The missing-parent retry write, which no test reached before. When the
/// parent row is gone from `thread_summaries` the card has already committed
/// and its projection has already cleared the child's marker, but the wake
/// never reached `parent_callback_tx`. The parent callback therefore genuinely
/// IS still pending, so the marker goes back to TRUE and the next terminal
/// event retries the kick.
///
/// This write also log-and-continues, so a typo'd column name in its SQL
/// string is silent at runtime.
#[tokio::test]
async fn missing_parent_row_leaves_the_callback_pending() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    // Drop the parent's summary row, leaving the child's `parent_thread_id`
    // dangling: the self-join's `p.is_coding_agent` comes back NULL, which is
    // the corruption branch under test.
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

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A mid-turn redirect is not a completion. Interrupt-and-redirect ends the
/// child's live turn with `ResponseCanceled { cause: SupersededByFollowup }`,
/// which means the caller steered rather than abandoned
/// (`thread_events/cause.rs`). Reporting it to the parent wakes the parent with
/// "your child was canceled" while the child is in fact running the redirected
/// turn, and the parent may spawn a replacement.
///
/// Coding-agent lane: a Codex follow-up (always) or an urgent Claude Code one,
/// both through `arm_followup_redirect`. The chat lane is the companion test
/// below; the discrimination itself is cause-only, so the two must agree.
#[tokio::test]
async fn a_coding_agent_redirect_does_not_report_a_cancellation_to_the_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    emit_response_canceled_with_cause(
        &bus,
        child_id,
        crate::engine::thread_events::CancelCause::SupersededByFollowup,
    )
    .await;

    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        0,
        "a SupersededByFollowup cancel is a redirect, not a completion: it \
         must persist no ChildThreadCompleted on the parent"
    );
    assert!(
        callback_rx.try_recv().is_err(),
        "a SupersededByFollowup cancel must not wake the parent"
    );
    // The in-tx reconcile is cause-agnostic and still runs, so no counter
    // drifts while the child is between turns; the follow-up's own
    // `MessageReceived` re-increments when the redirected turn starts.
    assert_active_children(
        &pool,
        parent_id,
        0,
        "the in-tx reconcile still runs for a redirect cancel",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The Lucidos Agent lane reaches the same cause by a different road: no
/// coding-agent session at all, just `cancel_thread_for_followup` cancelling the
/// per-thread token and `cancel_cause_for_turn` labelling the terminal. The
/// exclusion in `notify_parent_if_child` matches on cause alone, so this must
/// behave identically to the coding-agent case above. Asserted rather than
/// assumed: a chat child has no `SessionStarted`, so it takes a different arm of
/// `should_callback`, and "it is cause-only" is exactly the kind of claim that
/// silently stops being true.
#[tokio::test]
async fn a_chat_child_redirect_does_not_report_a_cancellation_to_the_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    // Chat channel and NO emit_cc_session_started: this is a Lucidos Agent
    // child, the lane whose redirect is new.
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
        0,
        "the in-tx reconcile is cause-agnostic and still runs on the chat lane too",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Companion to the redirect test: the discrimination must not swallow a real
/// user Stop, which is still a completion the parent has to hear about.
#[tokio::test]
async fn user_stop_still_reports_a_cancellation_to_the_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    emit_response_canceled_with_cause(
        &bus,
        child_id,
        crate::engine::thread_events::CancelCause::UserStop,
    )
    .await;

    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        1,
        "a user Stop is a real completion and must reach the parent"
    );
    assert!(
        callback_rx.try_recv().is_ok(),
        "a user Stop must wake the parent"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The redirect's transient counter window, pinned so it is understood rather
/// than rediscovered.
///
/// A `SupersededByFollowup` cancel sends no card (the test above), but its
/// projection arm is cause-agnostic: it settles the child to idle and
/// reconciles the parent from ground truth, so the parent's
/// `active_children_count` really does read 0 between the interrupt landing and
/// the redirected turn's `MessageReceived`. On the Codex lane that window can
/// be up to `REDIRECT_INTERRUPT_MAX_WAIT`, because the lane waits for the
/// interrupted turn to reach a boundary before emitting.
///
/// What the window costs and does not cost: the parent can end its own turn
/// inside it and show idle rather than "waiting for children" until the
/// redirected turn starts. Nothing is lost. The message is not dropped, the
/// card is not skipped, and the counter is not permanently wrong: the
/// redirected turn's start re-increments, its terminal reports, and every
/// terminal reconciles from ground truth rather than by delta. The parent is
/// still woken by the redirected turn's own completion.
///
/// Recorded as an accepted transient in ADR 0043 rather than closed here.
/// Closing it would mean not settling the child to idle on this cause, which
/// is a change to a contract-tested lifecycle transition and is deliberately
/// outside the phase that introduced the discrimination.
#[tokio::test]
async fn a_codex_redirect_dips_the_parent_count_then_restores_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    assert_active_children(&pool, parent_id, 1, "the child is working").await;

    // The interrupt lands. The child settles to idle and the parent's count
    // reconciles from ground truth, which is the dip.
    emit_response_canceled_with_cause(
        &bus,
        child_id,
        crate::engine::thread_events::CancelCause::SupersededByFollowup,
    )
    .await;
    assert_active_children(
        &pool,
        parent_id,
        0,
        "the dip: the interrupted turn settled the child before the redirected \
         one started",
    )
    .await;
    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        0,
        "and no card, so the parent is not told its child was canceled"
    );

    // The lane reaches the boundary and emits the follow-up. The child is back
    // in flight and the parent's count is restored.
    emit_cc_message_received(&bus, child_id, None, "go the other way").await;
    assert_active_children(
        &pool,
        parent_id,
        1,
        "restored the moment the redirect starts",
    )
    .await;
    assert!(
        read_callback_pending(&pool, child_id).await,
        "and the redirected turn owes the parent a card"
    );

    // The redirected turn completes, and THAT is the report.
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
