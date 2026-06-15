use super::super::*;
use super::*;

#[tokio::test]
async fn test_session_started_updates_source_in_projection() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    // Thread initially created as "chat" (e.g. by spawn_thread)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "fix something".into(),
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

    // Verify source is "chat"
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

    // SessionStarted fires when CC starts — should update source to "claude_code"
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "test-session".into(),
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

    // Verify source is now "claude_code"
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

/// `last_user_action` and `last_agent_action` are attributed independently: a
/// user-typed message bumps ONLY the user column; agent streaming bumps ONLY the
/// agent column. The unchanged-column equality checks are the load-bearing ones —
/// they prove the drawer's sort key (last_user_action) does not move on agent
/// churn, which is the whole point of the split.
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
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    };

    // 1. User message creates the row — both columns seed to ~now.
    bus.emit(human_message()).await.unwrap();
    let (u1, _a1) = fetch(pool.clone()).await;

    // 2. Agent streams text — bumps last_agent_action ONLY.
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

    // 3. User follow-up — bumps last_user_action ONLY.
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    bus.emit(human_message()).await.unwrap();
    let (u3, a3) = fetch(pool.clone()).await;
    assert!(
        u3 > u2,
        "a human follow-up must bump last_user_action (u3={u3} u2={u2})"
    );
    assert_eq!(
        a3, a2,
        "a user action must NOT bump last_agent_action"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_session_started_does_not_update_last_activity() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    // Create thread with MessageReceived
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "fix it".into(),
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

    // Record last_activity after MessageReceived
    let activity_after_msg: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT last_activity FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    // Small delay so timestamps differ
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // SessionStarted should NOT update last_activity
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "test-session".into(),
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

/// SessionEnded is mostly terminal: Shutdown, Panic, Closed, plus the
/// LegacyNonTerminal catch-all for old DB rows must transition the thread to a
/// terminal status with `has_response = TRUE`. The one exception is
/// StaleResume — see `test_session_ended_stale_resume_keeps_status_running`
/// for that case.
#[tokio::test]
async fn test_session_ended_transitions_to_terminal_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "fix it".into(),
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

/// Regression for "Drafts always in Drafts" report (2026-05-01): a CC follow-up
/// triggered a stale resume and the user saw a transient "Aborted" exchange
/// before the engine's internal retry produced a fresh `SessionStarted`.
///
/// Stale resume happens when CC's `--resume <sid>` returns an empty Result —
/// the prior session expired. `run_session` emits
/// `SessionEnded { StaleResume }` so restart-recovery's auto-detect resolver
/// doesn't try to use the dead sid; the chat handler then retries the user's
/// message against a fresh session within the same request.
///
/// During that retry window the thread MUST stay `running`. If the projection
/// flips to `idle`, the frontend's `threadIdle && !isComplete && hasSteps`
/// stale-exchange guard fires and the user sees "Aborted" until the retry's
/// `SessionStarted` lands seconds later.
#[tokio::test]
async fn test_session_ended_stale_resume_keeps_status_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "include the ios suite too".into(),
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
            session_id: "stale-sid".into(),
            branch: "claude-code/stale".into(),
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

/// CC permission prompt must move the thread out of 'running' into
/// 'waiting_for_user_answer' so the drawer surfaces it in REVIEW with the
/// question-mark badge. CodingAgentPermissionResolved returns it to 'running'
/// so the in-flight CC tool call can resume.
#[tokio::test]
async fn test_permission_request_transitions_status_to_waiting_for_user_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "edit my skill".into(),
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

    // Create thread with MessageReceived
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "analyze this repo".into(),
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

    // SessionStarted with repo_id should store it in thread_summaries
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "s1".into(),
            branch: "claude-code/test".into(),
            repo_id: Some(repo_uuid.into()),
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

    // A subsequent SessionStarted WITHOUT repo_id must NOT clear the stored repo_id
    // (this is the follow-up scenario where the frontend doesn't send repo_id)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "s2".into(),
            branch: "claude-code/followup".into(),
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

    // Parent thread
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "parent task".into(),
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

    // Non-CC child thread (source = "chat")
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "child task".into(),
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
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Non-CC child completes with ResponseGenerated — should trigger callback
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
async fn session_started_locks_coding_agent_backend_in_projection() {
    // First SessionStarted stamps `thread_summaries.coding_agent`; any later
    // SessionStarted (resume, replay, drift) must NOT flip it — the other
    // backend has no session to resume, so a flip silently loses context.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let session_started = |agent: crate::runtime::CodingAgent| ThreadEvent::SessionStarted {
        coding_agent: agent,
        session_id: "s".into(),
        branch: String::new(),
        repo_id: None,
        coding_agent_kind: Default::default(),
        coding_agent_folder: String::new(),
        app_id: None,
    };
    let meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    bus.emit(BusEvent::Thread {
        thread_id,
        event: session_started(crate::runtime::CodingAgent::Codex),
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

    // A later SessionStarted claiming ClaudeCode must not overwrite.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: session_started(crate::runtime::CodingAgent::ClaudeCode),
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
        Some("codex"),
        "backend is locked at first SessionStarted (COALESCE keeps existing)"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
