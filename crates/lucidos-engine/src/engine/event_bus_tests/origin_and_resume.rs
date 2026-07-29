use super::super::*;
use super::*;

/// Verify that events with valid request_event_id are accepted without warnings,
/// and that the validation path runs for events with request_event_id set.
#[tokio::test]
async fn test_valid_request_event_id_accepted() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    // Emit a MessageReceived event (this will be the origin)
    let origin = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                text: "fix this".into(),
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
        .unwrap()
        .unwrap();

    // Emit ResponseGenerated with valid request_event_id pointing to the origin
    let response = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseGenerated {
                text: "done".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta {
                request_event_id: Some(origin.event_id),
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .unwrap();

    // Verify both events are in DB
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "both events should be persisted");

    // Verify the response event has the correct request_event_id
    let req_id: Option<String> =
        sqlx::query_scalar("SELECT payload->>'request_event_id' FROM events WHERE id = $1")
            .bind(response.event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        req_id.as_deref(),
        Some(&origin.event_id.to_string()[..]),
        "response event should reference the origin event"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Verify that automated CC prompts create a valid origin event chain:
/// CodingAgentPromptSent is persisted and can be referenced by subsequent events.
#[tokio::test]
async fn test_automated_prompt_creates_valid_origin() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    // First create the thread with a MessageReceived (required for thread_summaries)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "initial".into(),
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

    // Emit CodingAgentPromptSent (simulating emit_automated_prompt)
    let prompt_result = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentPromptSent {
                text: "Run /harden now.".into(),
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .unwrap();

    // Now emit ResponseGenerated referencing the prompt
    let response = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseGenerated {
                text: "hardened".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta {
                request_event_id: Some(prompt_result.event_id),
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .unwrap();

    // Verify the prompt event exists in DB
    let prompt_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events WHERE id = $1)")
            .bind(prompt_result.event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(prompt_exists, "CodingAgentPromptSent must be persisted");

    // Verify the response references the prompt
    let req_id: Option<String> =
        sqlx::query_scalar("SELECT payload->>'request_event_id' FROM events WHERE id = $1")
            .bind(response.event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        req_id.as_deref(),
        Some(&prompt_result.event_id.to_string()[..]),
        "ResponseGenerated should reference CodingAgentPromptSent"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An empty `CodingAgentPromptSent` carries no agent intent and must not
/// flip thread status back to `running`. Real prompts always carry text
/// (user follow-up audit trail, automated Claude Code sessions for hardening /
/// recovery / conflict resolution).
#[tokio::test]
async fn empty_coding_agent_prompt_sent_does_not_flip_status_to_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "do the thing".into(),
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
        event: ThreadEvent::ResponseGenerated {
            text: "did the thing".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
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
            cc_session_id: Some("sid-empty-prompt".into()),
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

    let status_after_idle: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status_after_idle, "idle",
        "sanity: idle with no changes leaves thread idle",
    );

    bus.emit(BusEvent::Thread {
        thread_id,
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

    let status_after_empty_prompt: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status_after_empty_prompt, "idle",
        "an empty CodingAgentPromptSent carries no user/agent intent — it must \
         not flip status back to 'running'. Real prompts (audit trail for user \
         follow-ups, automated Claude Code sessions) always carry text."
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// After CC exits idle, a follow-up message can auto-resume via the persisted
/// cc_session_id in the CodingAgentIdled event.
#[tokio::test]
async fn cc_follow_up_after_exit_resumes_via_db() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    // 1. Claude Code session begins
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "s1".into(),
            branch: "claude-code/test".into(),
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

    // 2. CC finishes work, goes idle with a cc_session_id
    let cc_session_id = "test-session-abc123".to_string();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some(cc_session_id.clone()),
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
    // Replay the production lifecycle: ChangeProposed sets coding_agent_proposed
    // but keeps status='idle' (Option B — a proposed change is an artifact, not
    // a parked loop).
    emit_change_proposed(&bus, thread_id, "claude-code/feat", false).await;

    // 3. Thread status stays 'idle'; the pending-review state lives on
    //    `coding_agent_proposed`.
    let (status, proposed): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "idle");
    assert!(proposed, "ChangeProposed sets coding_agent_proposed");

    // 4. cc_session_id is recoverable from events (auto-resume detection query)
    let recovered: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'cc_session_id' FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentIdled' \
             ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(recovered, Some(cc_session_id.clone()));

    // 5. Follow-up message arrives (what chat_submit does)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "follow-up message".into(),
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

    // 6. Thread should be 'running' after follow-up
    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "running");

    // 7. Auto-resume query would find the session ID (CodingAgentIdled is most recent lifecycle event)
    let q = format!(
        "SELECT event_type, payload->>'cc_session_id' FROM events \
             WHERE thread_id = $1 AND event_type IN ({}) \
             ORDER BY sequence DESC LIMIT 1",
        crate::engine::agent_session::CC_TURN_CLOSER_EVENTS,
    );
    let (event_type, sid): (String, Option<String>) = sqlx::query_as(&q)
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(event_type, "CodingAgentIdled");
    assert_eq!(sid.as_deref(), Some("test-session-abc123"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// cc_session_id persisted in CodingAgentIdled events survives a simulated engine restart.
#[tokio::test]
async fn cc_session_id_survives_engine_restart() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let cc_session_id = "persistent-session-xyz".to_string();

    // Session starts, does work, goes idle
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "s1".into(),
            branch: "claude-code/test".into(),
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
            cc_session_id: Some(cc_session_id.clone()),
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

    // "Restart" — create a new EventBus against the same DB
    let (_bus2, _callback_rx2) = EventBus::new(pool.clone());

    // cc_session_id is still recoverable
    let q = format!(
        "SELECT event_type, payload->>'cc_session_id' FROM events \
             WHERE thread_id = $1 AND event_type IN ({}) \
             ORDER BY sequence DESC LIMIT 1",
        crate::engine::agent_session::CC_TURN_CLOSER_EVENTS,
    );
    let (event_type, sid): (String, Option<String>) = sqlx::query_as(&q)
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(event_type, "CodingAgentIdled");
    assert_eq!(sid.as_deref(), Some("persistent-session-xyz"));

    // Thread status survived
    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "idle"); // CodingAgentIdled with has_changes=false → idle

    pool.close().await;
    teardown_test_db(&db_name).await;
}
