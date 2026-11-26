use super::super::*;
use super::*;

#[tokio::test]
async fn test_session_started_updates_source_in_projection() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "fix something".into(),
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

    let source: Option<String> =
        sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        source.as_deref(),
        Some("chat"),
        "initial source should be chat"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            session_id: "test-session".into(),
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

    let source: Option<String> =
        sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        source.as_deref(),
        Some("claude_code"),
        "source should be updated to claude_code after SessionStarted"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_last_user_and_agent_action_attributed_separately() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    let fetch = |pool: PgPool| async move {
        sqlx::query_as::<_, (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>(
            "SELECT last_user_action, last_agent_action FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap()
    };

    let human_message = || BusEvent::Thread {
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
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    };

    bus.emit(human_message()).await.unwrap();
    let (u1, _a1) = fetch(pool.clone()).await;

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TextStreamed {
            text: "working...".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    let (u2, a2) = fetch(pool.clone()).await;
    assert_eq!(
        u2, u1,
        "agent streaming must NOT bump last_user_action (the drawer sort key)"
    );

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    bus.emit(human_message()).await.unwrap();
    let (u3, a3) = fetch(pool.clone()).await;
    assert!(
        u3 > u2,
        "a human follow-up must bump last_user_action (u3={u3} u2={u2})"
    );
    assert_eq!(a3, a2, "a user action must NOT bump last_agent_action");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_prompt_injected_is_a_user_action_only_from_a_human() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    type Stamp = chrono::DateTime<chrono::Utc>;

    let fetch = |pool: PgPool| async move {
        sqlx::query_as::<_, (Stamp, Stamp, Stamp)>(
            "SELECT last_user_action, last_agent_action, last_activity \
             FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let inject = |mode: ActorMode| BusEvent::Thread {
        thread_id,
        event: ThreadEvent::PromptInjected {
            text: "carry on".into(),
            mode,
            origin: None,
            injected_message_id: None,
            delivered_event_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    };

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
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    for mode in [ActorMode::Agent, ActorMode::Engine] {
        let (user_before, agent_before, activity_before) = fetch(pool.clone()).await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        bus.emit(inject(mode)).await.unwrap();
        let (user_after, agent_after, activity_after) = fetch(pool.clone()).await;
        assert!(
            activity_after > activity_before,
            "{mode:?} must bump last_activity"
        );
        assert_eq!(
            user_after, user_before,
            "{mode:?} must not bump last_user_action"
        );
        assert!(
            agent_after > agent_before,
            "{mode:?} must bump last_agent_action"
        );
    }

    let (user_before, agent_before, activity_before) = fetch(pool.clone()).await;
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    bus.emit(inject(ActorMode::Human)).await.unwrap();
    let (user_after, agent_after, activity_after) = fetch(pool.clone()).await;
    assert!(
        activity_after > activity_before,
        "a human prompt must bump last_activity"
    );
    assert!(
        user_after > user_before,
        "a human prompt must bump last_user_action"
    );
    assert_eq!(
        agent_after, agent_before,
        "a human prompt must not bump last_agent_action"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_session_started_does_not_update_last_activity() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "fix it".into(),
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

    let activity_after_msg: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT last_activity FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            session_id: "test-session".into(),
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

    let activity_after_session: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT last_activity FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(
            activity_after_msg, activity_after_session,
            "SessionStarted should not update last_activity — it's a technical event, not user activity"
        );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_session_ended_transitions_to_terminal_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "fix it".into(),
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

    bus.emit(BusEvent::Thread {
        thread_id,
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

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "idle",
        "SessionEnded {{ Panic }} must transition to terminal idle status"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_session_ended_stale_resume_keeps_status_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "include the ios suite too".into(),
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

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionEnded {
            reason: SessionEndReason::StaleResume,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let (status, has_response): (String, bool) =
        sqlx::query_as("SELECT status, has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "running",
        "SessionEnded {{ StaleResume }} must NOT flip status to terminal — \
         the chat handler is mid-retry against a fresh session and the frontend \
         would render the transient 'idle' as 'Aborted'"
    );
    assert!(
        !has_response,
        "SessionEnded {{ StaleResume }} must NOT set has_response — the user's \
         message hasn't actually been answered yet; the retry is still in flight"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_permission_request_transitions_status_to_waiting_for_user_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "edit my skill".into(),
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
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            session_id: "sess-1".into(),
            branch: "claude-code/branch".into(),
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

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "running");

    bus.emit(BusEvent::Thread {
        thread_id,
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

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "waiting_for_user_answer",
        "CodingAgentPermissionRequest must surface the thread in REVIEW"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionResolved {
            request_id: "req-1".into(),
            allowed: true,
            reason: None,
            persist_scope: None,
            standing_deny: None,
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
        "CodingAgentPermissionResolved must return the thread to running so CC can resume"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_session_started_stores_repo_id() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let repo_uuid = "550e8400-e29b-41d4-a716-446655440000";

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "analyze this repo".into(),
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
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            session_id: "s1".into(),
            branch: "claude-code/test".into(),
            repo_id: Some(repo_uuid.into()),
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

    let stored_repo_id: Option<String> =
        sqlx::query_scalar("SELECT cc_repo_id FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stored_repo_id.as_deref(),
        Some(repo_uuid),
        "cc_repo_id must be stored from SessionStarted"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            session_id: "s2".into(),
            branch: "claude-code/followup".into(),
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

    let stored_repo_id_after: Option<String> =
        sqlx::query_scalar("SELECT cc_repo_id FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored_repo_id_after.as_deref(), Some(repo_uuid),
            "cc_repo_id must persist across sessions — follow-up SessionStarted with no repo_id must not clear it");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_non_cc_child_callbacks_on_response_generated() {
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
            text: "parent task".into(),
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
            text: "child task".into(),
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
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Result.".into(),
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
    assert_eq!(
        callbacks.len(),
        1,
        "non-CC child should callback on ResponseGenerated"
    );
    assert_eq!(callbacks[0].parent_thread_id, parent_id);
    assert_eq!(callbacks[0].child_thread_id, child_id);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_newest_session_started_names_the_coding_agent_backend() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let session_started =
        |agent: crate::engine::thread_events::CodingAgent| ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: agent,
            session_id: "s".into(),
            branch: String::new(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        };
    let meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    bus.emit(BusEvent::Thread {
        thread_id,
        event: session_started(crate::engine::thread_events::CodingAgent::Codex),
        meta: meta.clone(),
    })
    .await
    .unwrap();

    let agent: Option<String> =
        sqlx::query_scalar("SELECT coding_agent FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        agent.as_deref(),
        Some("codex"),
        "first SessionStarted must stamp the backend"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: session_started(crate::engine::thread_events::CodingAgent::ClaudeCode),
        meta,
    })
    .await
    .unwrap();

    let agent: Option<String> =
        sqlx::query_scalar("SELECT coding_agent FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        agent.as_deref(),
        Some("claude-code"),
        "the newest SessionStarted names the backend"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn continuation_started(thread_id: Uuid, channel: EventChannel) -> BusEvent {
    BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ContinuationStarted {
            branch: String::new(),
            origin: None,
            reason: None,
            resumed_abort_id: None,
        },
        meta: EventMeta {
            channel: Some(channel),
            ..EventMeta::NONE
        },
    }
}

async fn read_thread_type(pool: &PgPool, thread_id: Uuid) -> (String, bool) {
    sqlx::query_as("SELECT source, is_coding_agent FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn chat_continuation_started_does_not_flip_thread_to_coding_agent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "what's the weather".into(),
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

    bus.emit(continuation_started(thread_id, EventChannel::Chat))
        .await
        .unwrap();

    let (source, is_coding_agent) = read_thread_type(&pool, thread_id).await;
    assert_eq!(
        source, "chat",
        "a chat Continue must not rewrite the thread's channel"
    );
    assert!(
        !is_coding_agent,
        "clicking Continue on a chat thread must not relabel it a coding-agent thread"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn trigger_continuation_started_keeps_trigger_source_and_flag_false() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            retry_of: None,
            provider: None,
            trigger_id: "morning-report".into(),
            trigger_name: Some("Morning report".into()),
            prompt: None,
            invocation: None,
            origin: None,
            go_to_review: false,
            model: None,
            reasoning_effort: None,
            continue_key: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    bus.emit(continuation_started(thread_id, EventChannel::Trigger))
        .await
        .unwrap();

    let (source, is_coding_agent) = read_thread_type(&pool, thread_id).await;
    assert_eq!(source, "trigger", "a trigger Continue keeps the channel");
    assert!(
        !is_coding_agent,
        "clicking Continue on a trigger thread must not relabel it a coding-agent thread"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn coding_agent_continuation_started_keeps_is_coding_agent_true() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            session_id: "s".into(),
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

    let (source, is_coding_agent) = read_thread_type(&pool, thread_id).await;
    assert_eq!(source, "claude_code");
    assert!(
        is_coding_agent,
        "SessionStarted is inherently a coding-agent event"
    );

    bus.emit(continuation_started(thread_id, EventChannel::ClaudeCode))
        .await
        .unwrap();

    let (source, is_coding_agent) = read_thread_type(&pool, thread_id).await;
    assert_eq!(source, "claude_code", "resume keeps the channel");
    assert!(
        is_coding_agent,
        "a coding-agent resume must keep the thread a coding-agent thread"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn chat_channel_continuation_started_never_clears_is_coding_agent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            session_id: "s".into(),
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

    bus.emit(continuation_started(thread_id, EventChannel::Chat))
        .await
        .unwrap();

    let (source, is_coding_agent) = read_thread_type(&pool, thread_id).await;
    assert!(
        is_coding_agent,
        "a non-coding-agent event must never clear the flag"
    );
    assert_eq!(
        source, "claude_code",
        "a non-coding-agent event must never rewrite an established channel"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn chat_continuation_started_preserves_the_stored_draft() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "what's the weather".into(),
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

    sqlx::query("UPDATE thread_summaries SET compose_text = $2 WHERE thread_id = $1")
        .bind(thread_id)
        .bind("half-typed follow-up")
        .execute(&pool)
        .await
        .unwrap();

    bus.emit(continuation_started(thread_id, EventChannel::Chat))
        .await
        .unwrap();

    let compose_text: String =
        sqlx::query_scalar("SELECT compose_text FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        compose_text, "half-typed follow-up",
        "a chat Continue must not wipe the user's draft"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
