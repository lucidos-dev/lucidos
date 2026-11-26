use super::super::*;
use super::*;

#[tokio::test]
async fn test_valid_request_event_id_accepted() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    let origin = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: "fix this".into(),
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
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .unwrap();

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

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "both events should be persisted");

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

#[tokio::test]
async fn test_automated_prompt_creates_valid_origin() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "initial".into(),
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
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let prompt_result = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentPromptSent {
                input_event_id: None,
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

    let prompt_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events WHERE id = $1)")
            .bind(prompt_result.event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(prompt_exists, "CodingAgentPromptSent must be persisted");

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

#[tokio::test]
async fn empty_coding_agent_prompt_sent_does_not_flip_status_to_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "do the thing".into(),
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
            turn_end: None,
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
            input_event_id: None,
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

#[tokio::test]
async fn cc_follow_up_after_exit_resumes_via_db() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "s1".into(),
            branch: "claude-code/test".into(),
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
            turn_end: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    emit_change_proposed(&bus, thread_id, "claude-code/feat", false).await;

    let (status, proposed): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "idle");
    assert!(proposed, "ChangeProposed makes the thread proposed");

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

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "follow-up message".into(),
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

#[tokio::test]
async fn cc_session_id_survives_engine_restart() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let cc_session_id = "persistent-session-xyz".to_string();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "s1".into(),
            branch: "claude-code/test".into(),
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
            turn_end: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (_bus2, _callback_rx2) = EventBus::new(pool.clone());

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

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "idle");

    pool.close().await;
    teardown_test_db(&db_name).await;
}
