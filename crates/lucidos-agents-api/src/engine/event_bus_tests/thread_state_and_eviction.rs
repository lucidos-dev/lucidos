use super::super::*;
use super::*;

#[tokio::test]
async fn trigger_started_creates_active_thread_not_composing() {
    use crate::engine::thread_state::ThreadState;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            retry_of: None,
            provider: None,
            trigger_id: "t-active".into(),
            trigger_name: Some("nightly".into()),
            prompt: Some("Run nightly".into()),
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
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

    let state: String =
        sqlx::query_scalar("SELECT state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, ThreadState::Active.as_str());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn trigger_started_does_not_resurrect_discarded_thread() {
    use crate::engine::thread_state::ThreadState;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            recovered_compose: None,
            agent_draft: None,
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadDiscarded { actor: None },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            retry_of: None,
            provider: None,
            trigger_id: "t-stale".into(),
            trigger_name: Some("nightly".into()),
            prompt: Some("Run nightly".into()),
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
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

    let state: String =
        sqlx::query_scalar("SELECT state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, ThreadState::Discarded.as_str());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn thread_discarded_sets_archive_state_to_archived() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            recovered_compose: None,
            agent_draft: None,
            mode: "claude_code".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let archive_state_before: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(archive_state_before, "inbox");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadDiscarded { actor: None },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let archive_state_after: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(archive_state_after, "archived");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn thread_summaries_state_column_defaults_to_active() {
    use crate::engine::thread_state::ThreadState;
    let (pool, db_name) = setup_test_db().await;
    let thread_id = Uuid::new_v4();

    sqlx::query("INSERT INTO thread_summaries (thread_id) VALUES ($1)")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();

    let state: String =
        sqlx::query_scalar("SELECT state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, ThreadState::Active.as_str());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn thread_started_with_claude_code_mode_sets_source_to_claude_code() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            recovered_compose: None,
            agent_draft: None,
            mode: "claude_code".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (source, compose_mode): (String, Option<String>) =
        sqlx::query_as("SELECT source, compose_mode FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(compose_mode.as_deref(), Some("claude_code"));
    assert_eq!(source, "claude_code");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn thread_started_with_lucidos_mode_sets_source_to_chat() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            recovered_compose: None,
            agent_draft: None,
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let source: String =
        sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(source, "chat");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn message_received_overrides_stale_compose_source_on_composing_to_active() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            recovered_compose: None,
            agent_draft: None,
            mode: "claude_code".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let pre_source: String =
        sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        pre_source, "claude_code",
        "draft source seeded as claude_code"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "send via lucidos".into(),
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

    let (state, source): (String, String) =
        sqlx::query_as("SELECT state, source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "active");
    assert_eq!(
        source, "chat",
        "composing → active must adopt the actual send's channel, got {source}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn stuck_thread_eviction_emits_aborted_with_system_actor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let request_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "first message".into(),
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
            event_id: Some(request_id),
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let agent_sessions = tokio::sync::Mutex::new(std::collections::HashMap::new());

    let active_threads =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    crate::engine::emit_stuck_thread_eviction_abort(
        &bus,
        &pool,
        &agent_sessions,
        &active_threads,
        thread_id,
    )
    .await;

    let (event_type, actor_kind, req_id, status): (String, Option<String>, Option<String>, String) =
        sqlx::query_as(
            "SELECT e.event_type, e.payload->'actor'->>'kind', \
                e.payload->>'request_event_id', ts.status \
             FROM events e JOIN thread_summaries ts ON ts.thread_id = e.aggregate_id::uuid \
             WHERE e.aggregate_id = $1 AND e.event_type IN \
                ('ResponseAborted','ResponseCanceled','ResponseFailed','ResponseGenerated') \
             ORDER BY e.sequence DESC LIMIT 1",
        )
        .bind(thread_id.to_string())
        .fetch_one(&pool)
        .await
        .expect("a terminal event must be persisted by the eviction");

    assert_eq!(event_type, "ResponseAborted");
    assert_eq!(actor_kind.as_deref(), Some("system"));
    assert_eq!(req_id.as_deref(), Some(request_id.to_string().as_str()));
    assert_eq!(status, "failed");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn stuck_thread_eviction_uses_child_thread_completed_as_req_id_for_chat() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let old_mr_id = Uuid::new_v4();
    let ctc_id = Uuid::new_v4();
    let child_thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "previous question".into(),
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
            event_id: Some(old_mr_id),
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "previous answer".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            request_event_id: Some(old_mr_id),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChildThreadCompleted {
            child_thread_id,
            child_thread_title: Some("child".into()),
            status: crate::engine::thread_events::ChildCompletionStatus::Success,
            summary: "child finished".into(),
            pending_change_ids: vec![],
            sub_thread_pending_changes: vec![],
            integrated_branch: None,
        },
        meta: EventMeta {
            event_id: Some(ctc_id),
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let agent_sessions = tokio::sync::Mutex::new(std::collections::HashMap::new());

    let active_threads =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    crate::engine::emit_stuck_thread_eviction_abort(
        &bus,
        &pool,
        &agent_sessions,
        &active_threads,
        thread_id,
    )
    .await;

    let req_id: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'request_event_id' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ResponseAborted' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("ResponseAborted must be persisted by the eviction");

    assert_eq!(
        req_id.as_deref(),
        Some(ctc_id.to_string().as_str()),
        "chat eviction must stamp the most recent originating event (CTC), \
         not the older MessageReceived — otherwise the abort routes to the \
         wrong exchange in the frontend and the pending question card stays clickable",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn originating_event_type_lists_include_child_thread_completed() {
    use crate::engine::thread_events::{CC_ORIGINATING_EVENT_TYPES, CHAT_ORIGINATING_EVENT_TYPES};
    assert!(
        CHAT_ORIGINATING_EVENT_TYPES.contains(&"ChildThreadCompleted"),
        "chat list missing CTC — restart/eviction/Continue would route aborts \
         to the wrong exchange for child re-entry turns"
    );
    assert!(
        CC_ORIGINATING_EVENT_TYPES.contains(&"ChildThreadCompleted"),
        "CC list missing CTC, same bug as the chat case for CC parents re-entered \
         from a finished child via notify_parent_of_child_completion"
    );
    assert!(
        CC_ORIGINATING_EVENT_TYPES.contains(&"CodingAgentUserMessageSent"),
        "CC list missing CCUMS — CC follow-up turns within an existing session \
         would not resolve to the right originating event"
    );
}

#[tokio::test]
async fn child_thread_completed_failure_summary_capped_at_200_chars() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "do thing".into(),
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
            text: "subtask".into(),
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

    let huge_error = "X".repeat(5000);
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseFailed {
            error: huge_error,
            spend_cap: None,
            agent_exit: None,
            provider_error: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let row: (serde_json::Value,) = sqlx::query_as(
        "SELECT payload FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ChildThreadCompleted' \
         ORDER BY created LIMIT 1",
    )
    .bind(parent_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("ChildThreadCompleted must be persisted on parent");
    let summary = row
        .0
        .get("summary")
        .and_then(|v| v.as_str())
        .expect("summary field present")
        .to_string();
    let status = row
        .0
        .get("status")
        .and_then(|v| v.as_str())
        .expect("status field present");

    assert_eq!(
        status, "failure",
        "status must be failure for ResponseFailed"
    );
    assert!(
        summary.ends_with("… (truncated)"),
        "long failure summary must carry the truncation marker, got len={}, summary={:?}",
        summary.len(),
        summary
    );
    assert!(
        summary.len() <= 250,
        "failure summary must be truncated at 200 chars (+ short suffix), got len={} bytes",
        summary.len()
    );
    assert!(
        summary.starts_with("XXX"),
        "summary must keep the prefix of the original error, got: {:?}",
        summary
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn system_actor_activity_event_does_not_resurrect_terminated_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "do a thing".into(),
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
        thread_id,
        event: ThreadEvent::ToolCalled {
            name: "run_python".into(),
            args: serde_json::json!({"code": "print(1)"}),
            description: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseAborted {
            text: "interrupted".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        meta: EventMeta::with_actor(Some(MessageOrigin::system())),
    })
    .await
    .unwrap();

    let after_abort: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after_abort, "failed",
        "a system-actor EngineShutdown abort is not the switch fingerprint, so a \
         thread with no proposed state settles at failed"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolResult {
            name: "run_python".into(),
            result: "[Tool execution interrupted by engine restart, original ToolCalled event_id: \
                     00000000-0000-0000-0000-000000000000]"
                .into(),
            images: vec![],
            success: false,
            tool_called_event_id: None,
        },
        meta: EventMeta::with_actor(Some(MessageOrigin::system())),
    })
    .await
    .unwrap();

    let after_synthetic: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after_synthetic, "failed",
        "system-actor synthetic ToolResult must not resurrect status to 'running'. \
         The recovery backfill is not live activity."
    );

    let live_thread = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: live_thread,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "live".into(),
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
        thread_id: live_thread,
        event: ThreadEvent::ToolResult {
            name: "search".into(),
            result: "ok".into(),
            images: vec![],
            success: true,
            tool_called_event_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    let live_status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(live_thread)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        live_status, "running",
        "live (no-actor) ToolResult must still bump status to 'running' — \
         the System-actor guard must not regress the live-activity path"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn interrupted_coding_agent_thread_keeps_paused_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let status_of = |thread_id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    for coding_agent in [
        crate::runtime::CodingAgent::ClaudeCode,
        crate::runtime::CodingAgent::Codex,
    ] {
        let agent = coding_agent.as_str();
        let thread_id = Uuid::new_v4();
        let cc_meta = || EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        };

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: "fix the thing".into(),
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
            meta: cc_meta(),
        })
        .await
        .unwrap();
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionStarted {
                sandbox: None,
                coding_agent,
                session_id: format!("{agent}-session"),
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
            meta: cc_meta(),
        })
        .await
        .unwrap();

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseAborted {
                text: String::new(),
                images: vec![],
                model: None,
                reasoning_effort: None,
                cause: crate::engine::thread_events::AbortCause::EngineShutdown,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                actor: Some(MessageOrigin::Device {
                    device_id: "dev-1".into(),
                }),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            "paused",
            "[{agent}] a device-attributed EngineShutdown abort is the user's own \
             switch, so it must settle the interrupted thread at paused, never at \
             the red failed"
        );

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentTextStreamed {
                text: "\n\nCommitting, then running the suite.".into(),
                coding_agent,
                parent_tool_use_id: None,
                progress_note: false,
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentToolResult {
                name: "Bash".into(),
                result: "ok".into(),
                coding_agent,
                tool_use_id: String::new(),
                parent_tool_use_id: None,
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            "paused",
            "[{agent}] trailing drain output must not resurrect the aborted turn \
             to 'running'. The abort is the turn's verdict."
        );

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: None,
                coding_agent,
                reason: Some("engine_restart_interrupt".into()),
                worktree_path: None,
                worktree_head_sha: None,
                bg_bash_pending: false,
                turn_end: None,
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            "paused",
            "[{agent}] CodingAgentIdled must not downgrade the interrupted \
             turn's paused verdict to 'idle'"
        );

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionEnded {
                reason: SessionEndReason::Shutdown,
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            "paused",
            "[{agent}] SessionEnded must not downgrade the interrupted turn's \
             paused verdict to 'idle'"
        );

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ContinuationRequested {
                reason: crate::engine::continuation_reason::USER_CLICKED_CONTINUE_REASON.into(),
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            "running",
            "[{agent}] a real start event must still clear the error status"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn interrupted_thread_with_a_pending_change_keeps_its_verdict() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let status_of = |thread_id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    let coding_agent = crate::runtime::CodingAgent::ClaudeCode;
    let cc_meta = || EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    let propose = |thread_id: Uuid| ThreadEvent::ChangeProposed {
        content: Default::default(),
        change_id: test_change_id(thread_id).to_string(),
        description: Some("Fix".into()),
        files: vec!["src/main.rs".into()],
        requires_restart: false,
        origin: None,
        commit_sha: None,
        branch_name: format!("claude-code/fix-{thread_id}"),
        repo_root: "/tmp".into(),
        hardened: false,
        incomplete: false,
        set_aside: false,
        path: String::new(),
        diff: String::new(),
    };

    for (case, actor, verdict) in [
        (
            "user switch",
            MessageOrigin::Device {
                device_id: "dev-1".into(),
            },
            "paused",
        ),
        (
            "teardown nobody asked for",
            MessageOrigin::system(),
            "failed",
        ),
    ] {
        let thread_id = Uuid::new_v4();

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionStarted {
                sandbox: None,
                coding_agent,
                session_id: "cc-session".into(),
                branch: "claude-code/fix".into(),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: String::new(),
                app_id: None,
                integrated: false,
                auto_compact_window: None,
                auto_compact_arm: None,
                continuity: None,
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();

        bus.emit(BusEvent::Thread {
            thread_id,
            event: propose(thread_id),
            meta: cc_meta(),
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            "running",
            "{case}: a mid-session proposal must not end the live turn"
        );

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseAborted {
                text: String::new(),
                images: vec![],
                model: None,
                reasoning_effort: None,
                cause: crate::engine::thread_events::AbortCause::EngineShutdown,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                actor: Some(actor),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            verdict,
            "{case}: the abort records its verdict whether or not a change is \
             pending. The change surfaces through the proposed change state."
        );

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentToolResult {
                name: "Bash".into(),
                result: "ok".into(),
                coding_agent,
                tool_use_id: String::new(),
                parent_tool_use_id: None,
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentTextStreamed {
                text: "\n\nCommitting.".into(),
                coding_agent,
                parent_tool_use_id: None,
                progress_note: false,
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            verdict,
            "{case}: trailing drain output must not resurrect the thread to \
             'running'. This is the spinner the user reported."
        );

        bus.emit(BusEvent::Thread {
            thread_id,
            event: propose(thread_id),
            meta: cc_meta(),
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            verdict,
            "{case}: a change proposed while the turn dies is a trailing event \
             like any other, and must leave the verdict alone"
        );

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentIdled {
                has_changes: true,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: None,
                coding_agent,
                reason: Some("engine_restart_interrupt".into()),
                worktree_path: None,
                worktree_head_sha: None,
                bg_bash_pending: false,
                turn_end: None,
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionEnded {
                reason: SessionEndReason::Shutdown,
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            verdict,
            "{case}: neither the idle nor the session teardown may downgrade it"
        );

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ContinuationRequested {
                reason: crate::engine::continuation_reason::AUTO_RESUME_AFTER_SWITCH_REASON.into(),
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            "running",
            "{case}: the resume is a start event and clears the verdict"
        );

        let proposed: bool = sqlx::query_scalar(
            "SELECT coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            proposed,
            "{case}: the change is still there to review. It was never the \
             status column's job to say so."
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_proposal_keeps_the_thread_on_its_open_question() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let status_of = |thread_id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let coding_agent = crate::runtime::CodingAgent::ClaudeCode;
    let cc_meta = || EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    let emit = |thread_id: Uuid, event: ThreadEvent| {
        let bus = &bus;
        async move {
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta: cc_meta(),
            })
            .await
            .unwrap();
        }
    };
    let idle = || ThreadEvent::CodingAgentIdled {
        has_changes: true,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: None,
        coding_agent,
        reason: None,
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
        turn_end: None,
    };
    let session_started = || ThreadEvent::SessionStarted {
        sandbox: None,
        coding_agent,
        session_id: "cc-session".into(),
        branch: "claude-code/fix".into(),
        repo_id: None,
        coding_agent_kind: Default::default(),
        coding_agent_folder: String::new(),
        app_id: None,
        integrated: false,
        auto_compact_window: None,
        auto_compact_arm: None,
        continuity: None,
    };
    let propose = |change_id: Uuid| ThreadEvent::ChangeProposed {
        content: Default::default(),
        change_id: change_id.to_string(),
        description: Some("Fix".into()),
        files: vec!["src/main.rs".into()],
        requires_restart: false,
        origin: Some(MessageOrigin::engine(
            crate::engine::thread_events::EngineReason::OrphanRecovery,
        )),
        commit_sha: None,
        branch_name: format!("claude-code/fix-{change_id}"),
        repo_root: "/tmp".into(),
        hardened: false,
        incomplete: true,
        set_aside: false,
        path: String::new(),
        diff: String::new(),
    };

    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    emit(thread_id, session_started()).await;
    emit(
        thread_id,
        ThreadEvent::UserQuestionAsked {
            tool_use_id: "tu-open".into(),
            cc_session_id: "cc-session".into(),
            question: "Where did you end up?".into(),
            options: vec![],
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            permission_request: None,
        },
    )
    .await;
    assert_eq!(status_of(thread_id).await, "waiting_for_user_answer");

    emit(thread_id, propose(change_id)).await;
    assert_eq!(
        status_of(thread_id).await,
        "waiting_for_user_answer",
        "a proposal must not put out the question the user still owes an answer to"
    );
    assert_eq!(
        crate::api::changes::change_action_refusal(
            &pool,
            change_id,
            crate::engine::thread_lifecycle::ThreadAction::Apply,
        )
        .await
        .unwrap(),
        Some(crate::api::changes::ChangeActionRefusal::ThreadParked),
        "a thread parked on a question offers no Apply, so nothing can start \
         a hardening run over the question"
    );
    assert!(
        crate::engine::question_park_sql::thread_has_unanswered_question(&pool, thread_id).await,
        "Apply Now refuses on this predicate, so the proposal must not unpark it"
    );

    let set_status = |status: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query("UPDATE thread_summaries SET status = $2 WHERE thread_id = $1")
                .bind(thread_id)
                .bind(status)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    set_status("idle").await;
    assert_eq!(
        crate::api::changes::change_action_refusal(
            &pool,
            change_id,
            crate::engine::thread_lifecycle::ThreadAction::Apply,
        )
        .await
        .unwrap(),
        Some(crate::api::changes::ChangeActionRefusal::ThreadParked),
        "the gate reads the open question, not only the status"
    );
    assert!(
        crate::core::changes::unsettled_thread_ids(&pool, std::iter::once(thread_id))
            .await
            .unwrap()
            .contains(&thread_id),
        "the bulk paths drop it too"
    );
    set_status("waiting_for_user_answer").await;

    emit(
        thread_id,
        ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: "tu-open".into(),
            quiet: false,
            answer: AnswerKind::FreeText {
                text: "somewhere older".into(),
                image_hashes: vec![],
            },
        },
    )
    .await;
    assert_eq!(status_of(thread_id).await, "running");
    assert!(
        !crate::engine::question_park_sql::thread_has_unanswered_question(&pool, thread_id).await,
        "an answered question no longer blocks Apply Now"
    );
    emit(thread_id, idle()).await;
    assert_eq!(status_of(thread_id).await, "idle");
    let later_change = Uuid::new_v4();
    emit(thread_id, propose(later_change)).await;
    assert_eq!(
        status_of(thread_id).await,
        "idle",
        "an answered question holds nothing"
    );
    assert_eq!(
        crate::api::changes::change_action_refusal(
            &pool,
            later_change,
            crate::engine::thread_lifecycle::ThreadAction::Apply,
        )
        .await
        .unwrap(),
        None,
        "nor does it block an Apply"
    );

    let carded = Uuid::new_v4();
    emit(carded, session_started()).await;
    emit(
        carded,
        ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-left".into(),
            tool_use_id: "tu-left".into(),
            tool_name: "Bash".into(),
            input: serde_json::json!({"command": "ls"}),
            summary: "Bash ls".into(),
            grant_offer: None,
        },
    )
    .await;
    assert_eq!(status_of(carded).await, "waiting_for_user_answer");
    emit(carded, propose(Uuid::new_v4())).await;
    assert_eq!(status_of(carded).await, "idle");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn only_a_user_switch_teardown_settles_a_thread_at_paused() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let device = MessageOrigin::Device {
        device_id: "dev-1".into(),
    };

    for (case, cause, actor, expected) in [
        (
            "user switch teardown",
            crate::engine::thread_events::AbortCause::EngineShutdown,
            device.clone(),
            "paused",
        ),
        (
            "system-actor shutdown fallback",
            crate::engine::thread_events::AbortCause::EngineShutdown,
            MessageOrigin::system(),
            "failed",
        ),
        (
            "boot recovery / withdrawn resume promise",
            crate::engine::thread_events::AbortCause::RecoveryAfterRestart,
            MessageOrigin::system(),
            "failed",
        ),
    ] {
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
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseAborted {
                text: "interrupted".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
                cause,
            },
            meta: EventMeta::with_actor(Some(actor)),
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
            status, expected,
            "{case}: {cause:?} must settle the thread at {expected}"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn shutdown_phantom_cancel_does_not_clear_the_abort_error_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let status_of = |thread_id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "summarize the log".into(),
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
        thread_id,
        event: ThreadEvent::ResponseAborted {
            text: "This response was interrupted by an engine shutdown.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        meta: EventMeta::with_actor(Some(MessageOrigin::system())),
    })
    .await
    .unwrap();
    assert_eq!(status_of(thread_id).await, "failed");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseCanceled {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(
        status_of(thread_id).await,
        "failed",
        "the loop's phantom ResponseCanceled must not clear the interrupted \
         thread's status. The abort is the turn's verdict."
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "try again".into(),
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
    assert_eq!(status_of(thread_id).await, "running");
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseCanceled {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(
        status_of(thread_id).await,
        "idle",
        "a real user Stop on a fresh turn must still settle the thread to idle"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn thread_discarded_clears_compose_selection() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            recovered_compose: None,
            agent_draft: None,
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    sqlx::query(
        "UPDATE thread_summaries \
         SET compose_text = 'half-typed', compose_selection = '{\"model\":\"claude-opus-4-8\"}'::jsonb \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadDiscarded { actor: None },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let compose_selection: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT compose_selection FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        compose_selection, None,
        "compose_selection must be cleared on discard, in lockstep with the other compose fields"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn seed_thread_awaiting_answer(bus: &EventBus, pool: &PgPool, draft: &str) -> Uuid {
    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            recovered_compose: None,
            agent_draft: None,
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    sqlx::query(
        "UPDATE thread_summaries \
         SET state = 'active', compose_text = $2, \
             compose_selection = '{\"scope\":{\"kind\":\"lucidos\"}}'::jsonb \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .bind(draft)
    .execute(pool)
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
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
    thread_id
}

async fn read_compose(pool: &PgPool, thread_id: Uuid) -> (String, Option<serde_json::Value>) {
    sqlx::query_as(
        "SELECT compose_text, compose_selection FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_draft_written_after_the_accept_survives_the_sends_later_events() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = seed_thread_awaiting_answer(&bus, &pool, "the next message").await;
    sqlx::query("UPDATE thread_summaries SET compose_epoch = 1 WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();
    let mut rx = bus.subscribe();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: "tu-1".into(),
            quiet: false,
            answer: AnswerKind::FreeText {
                text: "the next message".into(),
                image_hashes: vec![],
            },
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    emit_thread_message(&bus, thread_id, None, "the sent message").await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
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

    let (compose_text, compose_selection) = read_compose(&pool, thread_id).await;
    assert_eq!(
        compose_text, "the next message",
        "the draft typed after the accept survives"
    );
    assert!(compose_selection.is_some(), "and so do its picks");
    let epoch: i64 =
        sqlx::query_scalar("SELECT compose_epoch FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        epoch, 1,
        "one submission moves the epoch once, at its accept"
    );
    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::System(SystemEvent::ThreadComposeChanged { id, .. }) = &emitted.typed {
            assert_ne!(
                *id, thread_id,
                "nothing changed the draft, so nothing announces it"
            );
        }
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_message_that_creates_the_thread_broadcasts_no_compose_change() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let mut rx = bus.subscribe();

    emit_thread_message(&bus, thread_id, None, "straight to send").await;

    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::System(SystemEvent::ThreadComposeChanged { id, .. }) = &emitted.typed {
            assert_ne!(*id, thread_id, "the thread was created by this message");
        }
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_chat_continue_leaves_the_compose_epoch_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            recovered_compose: None,
            agent_draft: None,
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    sqlx::query(
        "UPDATE thread_summaries SET compose_text = 'half a follow-up' WHERE thread_id = $1",
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ContinuationStarted {
            branch: String::new(),
            origin: None,
            reason: Some(crate::engine::continuation_reason::USER_CLICKED_CONTINUE_REASON.into()),
            resumed_abort_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let row: (String, i64) = sqlx::query_as(
        "SELECT compose_text, compose_epoch FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "half a follow-up", "a Continue consumes no draft");
    assert_eq!(row.1, 0, "and therefore moves no epoch");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn restored_draft_is_atomic_and_retry_preserves_newer_edits() {
    use crate::engine::thread_events::RecoveredCompose;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let generation = Uuid::new_v4();
    sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) VALUES ($1, 'test', now(), 0)")
        .bind(generation).execute(&pool).await.unwrap();
    let thread_id = Uuid::new_v4();
    let event = || BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            mode: "lucidos".into(),
            actor: None,
            recovered_compose: Some(RecoveredCompose {
                generation,
                text: "preserved".into(),
                image_hashes: vec![],
                selection: Some(serde_json::json!({"model": "example-model"})),
            }),
            agent_draft: None,
        },
        meta: EventMeta::NONE,
    };
    bus.emit(event()).await.unwrap();
    let initial: (String, serde_json::Value) = sqlx::query_as(
        "SELECT compose_text, compose_selection FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        initial,
        (
            "preserved".into(),
            serde_json::json!({"model": "example-model"})
        )
    );
    sqlx::query("UPDATE thread_summaries SET compose_text = 'newer', compose_mode = 'claude_code', state = 'active' WHERE thread_id = $1")
        .bind(thread_id).execute(&pool).await.unwrap();
    assert!(bus.emit(event()).await.unwrap().is_none());
    let after: (String, String, String) = sqlx::query_as(
        "SELECT compose_text, compose_mode, state FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        after,
        ("newer".into(), "claude_code".into(), "active".into())
    );
    sqlx::query("UPDATE snapshot_restore_boundary SET restore_id = $1")
        .bind(Uuid::new_v4())
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        bus.emit(event()).await.is_err(),
        "old-generation retries must be refused"
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ThreadStarted'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "duplicate and refused events must not persist");
    pool.close().await;
    teardown_test_db(&db_name).await;
}
