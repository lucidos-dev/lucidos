use super::super::*;
use super::*;

/// Bug: CodingAgentIdled(has_changes=false) from Default section was suppressed,
/// leaving the thread in ARCHIVE. First idle (no changes) must surface in REVIEW
/// so the user knows the Claude Code session completed.
#[tokio::test]
async fn cc_idle_no_changes_from_default_goes_to_review() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/explore", None).await;

    // CC completes with no file changes
    emit_cc_idle(&bus, thread_id, false, Some("sid-1")).await;

    let (section, status): (String, String) =
        sqlx::query_as("SELECT archive_state, status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(section, "inbox",
            "CodingAgentIdled(no changes) from Archived must set section to inbox (REVIEW), not stay archived (ARCHIVE)");
    assert_eq!(status, "idle");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Housekeeping: CodingAgentIdled(has_changes=false) when section is already 'inbox'
/// (after apply/discard) must not change section — it's already in REVIEW.
#[tokio::test]
async fn cc_idle_no_changes_from_inbox_stays_inbox() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/fix", None).await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    // Apply the change — section stays 'inbox' for Archive button
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: "c1".into(),
            description: Some("Fix".into()),
            files: vec!["a.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            change_id: "c1".into(),
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

    // Housekeeping idle — section already 'inbox', must stay 'inbox'
    emit_cc_idle(&bus, thread_id, false, Some("sid-1")).await;

    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "inbox",
        "Housekeeping CodingAgentIdled(no changes) must keep section inbox"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// When a parent chat thread has active CC children and ResponseGenerated fires,
/// the broadcast aggregate must carry both the section transition (to inbox) and
/// the children count, so the frontend updates both atomically — preventing a
/// transient REVIEW state (should be WAITING).
#[tokio::test]
async fn response_generated_on_parent_with_children_broadcasts_children_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    // Create parent chat thread
    let parent_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "dispatch work".into(),
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

    // Spawn two CC children
    for i in 0..2 {
        let child_id = Uuid::new_v4();
        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event: ThreadEvent::MessageReceived {
                text: format!("child task {}", i),
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
    }

    assert_active_children(&pool, parent_id, 2, "parent should have 2 active children").await;

    // Subscribe to capture events AFTER children are spawned
    let mut rx = bus.subscribe();

    // Parent finishes responding → ResponseGenerated → section transitions to inbox
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::ResponseGenerated {
            text: "I've started two Claude Code sessions.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // ResponseGenerated's aggregate snapshot carries section AND
    // active_children_count in one envelope — no follow-up section-change
    // event or ChildrenCountChanged broadcast is needed for the frontend to
    // render consistent state.
    let mut response_generated_aggregate: Option<crate::core::store::ThreadAggregate> = None;
    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::Thread {
            thread_id, event, ..
        } = &emitted.typed
        {
            if *thread_id == parent_id && event.event_type() == "ResponseGenerated" {
                response_generated_aggregate = emitted.aggregate.clone();
            }
        }
    }
    let agg =
        response_generated_aggregate.expect("ResponseGenerated must be broadcast with aggregate");
    assert_eq!(
        agg.section, "inbox",
        "aggregate carries the new section='inbox' (no separate section-change event needed)"
    );
    assert_eq!(
        agg.active_children_count, 2,
        "aggregate carries active_children_count=2 (replaces the legacy ChildrenCountChanged re-broadcast)"
    );
    assert_eq!(
        agg.total_children_count, 2,
        "aggregate carries total_children_count=2"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn emit_device_visible_writes_to_device_presence_projection() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());

    bus.emit(BusEvent::System(SystemEvent::DeviceVisible {
        device_id: "dev-1".into(),
    }))
    .await
    .unwrap();

    assert!(
        !crate::core::DevicePresenceStore::candidates(&pool)
            .await
            .unwrap()
            .is_empty(),
        "DeviceVisible should mark the device visible in device_presence"
    );

    // Transient — never persisted to events.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE event_type IN ('DeviceVisible', 'DeviceHidden')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 0,
        "DeviceVisible/DeviceHidden must not be persisted to events table"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn device_visible_heartbeat_does_not_broadcast() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let mut rx = bus.subscribe();

    bus.emit(BusEvent::System(SystemEvent::DeviceVisible {
        device_id: "dev-1".into(),
    }))
    .await
    .unwrap();
    assert!(rx.try_recv().is_ok(), "first DeviceVisible must broadcast");

    // Heartbeat — same device, already-recorded-visible. Should NOT broadcast
    // so we don't wake every SSE subscriber every 30s.
    bus.emit(BusEvent::System(SystemEvent::DeviceVisible {
        device_id: "dev-1".into(),
    }))
    .await
    .unwrap();
    assert!(
        rx.try_recv().is_err(),
        "heartbeat DeviceVisible must NOT broadcast"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn emit_device_hidden_clears_presence_and_no_op_when_already_hidden() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let mut rx = bus.subscribe();

    // Hide a device that was never visible — no-op, no broadcast.
    bus.emit(BusEvent::System(SystemEvent::DeviceHidden {
        device_id: "dev-never".into(),
    }))
    .await
    .unwrap();
    assert!(
        rx.try_recv().is_err(),
        "DeviceHidden on a non-visible device must not broadcast"
    );

    // Now visible → hidden → projection cleared.
    bus.emit(BusEvent::System(SystemEvent::DeviceVisible {
        device_id: "dev-1".into(),
    }))
    .await
    .unwrap();
    let _ = rx.try_recv();
    bus.emit(BusEvent::System(SystemEvent::DeviceHidden {
        device_id: "dev-1".into(),
    }))
    .await
    .unwrap();
    assert!(
        crate::core::DevicePresenceStore::candidates(&pool)
            .await
            .unwrap()
            .is_empty(),
        "DeviceHidden must remove the device_presence row"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// CC may emit a `Result` mid-session — e.g. when the model invokes a Skill
/// tool that triggers another model turn — making the engine emit
/// `CodingAgentIdled` before CC is actually done. The next `CodingAgentToolCalled`
/// (or text/result) proves CC is still working, so the projection must bump
/// status back to `running` to keep the thread out of REVIEW while work
/// continues. Without this, the thread shows in REVIEW with a stale "idle"
/// status while the agent is mid-tool-call.
#[tokio::test]
async fn test_cc_activity_after_idled_bumps_status_back_to_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    // Seed: MessageReceived → SessionStarted → CodingAgentIdled.
    // After Idled, status is 'idle' (no changes).
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "fix the bug".into(),
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
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sess-1".into(),
            branch: "claude-code/branch".into(),
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

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("sess-1".into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "idle", "post-Idled status should be idle");

    // Now: CC continues with a tool call (e.g. it invoked Skill and the
    // skill content triggers another model turn). The engine never emitted
    // a new MessageReceived/PromptSent — this is internal CC continuation.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentToolCalled {
            name: "Bash".into(),
            args: serde_json::json!({"command": "ls"}),
            description: String::new(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            tool_use_id: String::new(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "running",
        "CC activity event after premature Idled must bump status back to 'running' \
         so the thread leaves REVIEW while work continues"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Same recovery for `CodingAgentTextStreamed` arriving after Idled —
/// CC's response text streaming proves work is in progress.
#[tokio::test]
async fn test_cc_text_streamed_after_idled_bumps_status_back_to_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "do work".into(),
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
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("sess-1".into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentTextStreamed {
            text: "still working...".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "running");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// `events_pkey` is the structural reason the agentic loop's old inject
/// path silently dropped `UserPromptInjected` events: a row already
/// existed with the same id (the optimistic `MessageReceived`). Locking
/// this in here so a future schema change that relaxes the constraint
/// doesn't quietly resurrect that bug.
#[tokio::test]
async fn emit_with_existing_event_id_fails_with_pkey_violation() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let dup_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "first".into(),
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
            event_id: Some(dup_id),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("first emit ok");

    let result = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::UserPromptInjected {
                text: "second".into(),
                mode: ActorMode::Human,
                origin: None,
                injected_message_id: None,
                delivered_event_id: None,
            },
            meta: EventMeta {
                event_id: Some(dup_id),
                ..EventMeta::NONE
            },
        })
        .await;

    let err = match result {
        Ok(_) => panic!("second emit with same id must fail (events_pkey violation)"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("duplicate key") || msg.contains("events_pkey"),
        "error must mention the pkey violation: {}",
        msg
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: a failed CC turn emits `ResponseFailed` (status='failed') then
/// `CodingAgentIdled` in the same turn. The idle is CC-lifecycle bookkeeping and
/// must NOT downgrade 'failed' → 'idle' — otherwise the red error dot in the
/// thread list disappears (the originally-reported "this should have gotten an
/// error dot" bug, e.g. CC `Not logged in · Please run /login`).
#[tokio::test]
async fn cc_idle_after_failed_turn_preserves_failed_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/login-error", None).await;

    // CC turn ends in failure — terminal event sets status='failed'.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseFailed {
            error: "Unknown error".into(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let after_fail: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after_fail, "failed",
        "ResponseFailed must set status='failed'"
    );

    // The same turn's bookkeeping idle (no changes) follows.
    emit_cc_idle(&bus, thread_id, false, Some("sid-fail")).await;

    let after_idle: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after_idle, "failed",
        "CodingAgentIdled after a failed turn must preserve 'failed' (red error dot), \
         not downgrade to 'idle'"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Complement: a *successful* CC turn (ResponseGenerated → CodingAgentIdled) must
/// still settle to 'idle'. The failed-preservation CASE only fires on 'failed',
/// so a normal turn is unaffected.
#[tokio::test]
async fn cc_idle_after_successful_turn_settles_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/ok", None).await;

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
    emit_cc_idle(&bus, thread_id, false, Some("sid-ok")).await;

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "idle",
        "a successful CC turn's idle must settle to 'idle' (failed-preservation must not over-fire)"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
