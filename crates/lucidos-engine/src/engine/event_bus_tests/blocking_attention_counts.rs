use super::super::*;
use super::*;

// -----------------------------------------------------------------------
// blocking_descendant_count — projection wiring tests.
//
// The materialized count lives on `thread_summaries.blocking_descendant_count`
// and is maintained by the function-boundary sampling wrapper in
// `event_bus_projection.rs`. Every event arm that can flip a thread's
// `is_blocking` predicate (status → Running/WFUA, ChangeProposed +/- the
// pending flag, ThreadArchived clearing the section) must propagate the
// resulting delta up the ancestor chain via the recursive CTE. The
// integration tests below exercise each flip direction end-to-end.
// -----------------------------------------------------------------------
/// A CC sub-thread that's spawned + Running but has not yet fired a
/// section-transitioning event (CodingAgentIdled / ChangeProposed /
/// UserQuestionAsked) must already count against the parent. The column
/// default 'inbox' keeps fresh rows distinguishable from user-archived ones,
/// and `is_blocking` treats Running as always blocking regardless.
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
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
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

/// Defense in depth: any legacy row with status='running' AND
/// archive_state='archived' (a pre-migration row, manual DB edit, race with a
/// cascade) must still count as blocking after `rebuild_blocking_descendant_count`
/// — the SQL mirror has to agree with the Rust predicate.
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

    // Follow-up MessageReceived on the CC child transitions it back to Running.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "follow up".into(),
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

    // Bring the child back to Running so we have something to decrement.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "ping".into(),
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
    assert_eq!(read_blocking_descendant_count(&pool, parent_id).await, 1);

    // ResponseGenerated drops the CC child to Idle (no pending changes).
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

    // ChangeProposed sets coding_agent_proposed=TRUE on the CC child.
    // is_blocking flips false → true via the (has_pending_changes && CC) clause.
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

    // ThreadArchived flips archive_state to 'archived' and clears the CC flags.
    // is_blocking returns false the moment archive_state is Archived.
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

    // UserQuestionAsked → status='waiting_for_user_answer' → blocking.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: "tu-1".into(),
            cc_session_id: "sess-1".into(),
            question: "Proceed?".into(),
            options: vec![],
            worktree_path: None,
            multi_select: false,
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

    // UserQuestionAnswered → status='running' → still blocking (no change),
    // but no longer needing attention (Running ≠ attention).
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: "tu-1".into(),
            answer: AnswerKind::FreeText { text: "yes".into() },
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

    // ResponseGenerated → Idle → not blocking, count returns to 0.
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

/// CC permission lifecycle mirrors UserQuestion: Asked bumps attention,
/// Resolved drops it. Drives the parent's Current-bubble routing through
/// the CC-specific event pair, which has the same WFUA semantics as the
/// chat-side UserQuestion pair (per the status-transition table in
/// `thread_lifecycle.rs`).
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

/// CC `ChangeProposed` puts the child into a state where the user must
/// Apply or Discard. The parent must bubble to Current. Mirror is
/// `ChangeApplied` / `ChangeDiscarded` clearing the attention counter
/// (already exercised indirectly via the existing blocking-count tests;
/// this test pins the attention split explicitly).
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

    // ThreadArchived clears the proposal flag and decrements both counters.
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

// Note: external-repo + pending-changes is not a reachable state — the
// runtime never emits `ChangeProposed` for external repos, so
// `coding_agent_proposed` and `coding_agent_is_external_repo` are
// mutually exclusive in practice. The external-repo carve-out in
// `is_attention_needing` is structural parity with `is_blocking`
// clause 3 (and inherits its correctness guarantee from
// `external_repo_idle_with_changes_never_shows_apply_discard`); it
// requires no separate attention-count test.
/// Mixed-siblings: when one CC child is paused on a permission and
/// another is still running, the parent's `attention_descendant_count`
/// reflects only the WFUA child (1), while `blocking_descendant_count`
/// counts both (2). This is the case the Current-bubble rule was
/// designed for — the parent surfaces in Current even though sibling
/// work continues. Pinning it here so a future regression on the
/// attention/blocking split shows up as a test failure rather than a
/// drawer bug.
#[tokio::test]
async fn mixed_siblings_attention_counts_only_wfua_not_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    // Spawn parent with one running CC child via the standard helper.
    let (parent_id, running_child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, running_child_id).await;
    // Don't idle this child — it stays Running.

    // Spawn a second CC child under the same parent.
    let wfua_child_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: wfua_child_id,
        event: ThreadEvent::MessageReceived {
            text: "second child".into(),
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
    emit_cc_session_started(&bus, wfua_child_id).await;

    // Baseline: both children Running → blocking=2, attention=0.
    assert_eq!(read_blocking_descendant_count(&pool, parent_id).await, 2);
    assert_eq!(read_attention_descendant_count(&pool, parent_id).await, 0);

    // Drive the second child into WFUA via a permission request.
    let request_id = Uuid::new_v4().to_string();
    bus.emit(BusEvent::Thread {
        thread_id: wfua_child_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id,
            tool_use_id: "tu-mixed".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({}),
            summary: "Edit file".into(),
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

    // Grandparent (chat) → Parent (chat sub-thread) → CC grandchild.
    // Build the chain manually so each ancestor link is set at MessageReceived.
    let grandparent_id = Uuid::new_v4();
    let parent_id = Uuid::new_v4();
    let grandchild_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: grandparent_id,
        event: ThreadEvent::MessageReceived {
            text: "root task".into(),
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

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "mid task".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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

    // The two MessageReceived events above already each flipped is_blocking
    // (Chat threads in Inbox + Running are blocking). Settle them to Idle
    // with ResponseGenerated so the baseline is a clean zero on both
    // grandparent and parent.
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

    // Spawn the CC grandchild idle.
    bus.emit(BusEvent::Thread {
        thread_id: grandchild_id,
        event: ThreadEvent::MessageReceived {
            text: "cc task".into(),
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

    // Now flip the grandchild to Running via MessageReceived. Both ancestors
    // must see their count bump.
    bus.emit(BusEvent::Thread {
        thread_id: grandchild_id,
        event: ThreadEvent::MessageReceived {
            text: "wake".into(),
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

    // Parent + two CC children: one ends Running (blocking), one ends Idle (not).
    let parent_id = Uuid::new_v4();
    let running_child = Uuid::new_v4();
    let idle_child = Uuid::new_v4();

    // Parent row.
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "parent".into(),
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

    // Running CC child — start, idle (to surface in Inbox), then follow-up
    // MessageReceived to put it back in Running. This is the only state that
    // satisfies is_blocking: Running + Inbox + CC.
    bus.emit(BusEvent::Thread {
        thread_id: running_child,
        event: ThreadEvent::MessageReceived {
            text: "running".into(),
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
    emit_cc_session_started(&bus, running_child).await;
    emit_cc_idle(&bus, running_child, false, None).await;
    bus.emit(BusEvent::Thread {
        thread_id: running_child,
        event: ThreadEvent::MessageReceived {
            text: "wake".into(),
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

    // Idle CC child — start + CodingAgentIdled surfaces it to Inbox in Idle.
    bus.emit(BusEvent::Thread {
        thread_id: idle_child,
        event: ThreadEvent::MessageReceived {
            text: "idle".into(),
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
    emit_cc_session_started(&bus, idle_child).await;
    emit_cc_idle(&bus, idle_child, false, None).await;

    // Sanity: incremental projection has parent at 1 (running_child blocks,
    // idle_child does not). Wipe the count so we can prove rebuild restores it.
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
