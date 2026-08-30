use super::super::*;
use super::*;

// ---- Initiator propagation tests ----
/// User-initiated chat thread has initiator='user'.
#[tokio::test]
async fn initiator_user_chat() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tid = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: tid,
        event: ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "hello".into(),
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

    let initiator: String =
        sqlx::query_scalar("SELECT initiator FROM thread_summaries WHERE thread_id = $1")
            .bind(tid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(initiator, "user");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Scheduled task thread has initiator='system'.
#[tokio::test]
async fn initiator_system_trigger() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tid = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: tid,
        event: ThreadEvent::TriggerStarted {
            trigger_id: "t-1".into(),
            trigger_name: Some("daily".into()),
            prompt: None,
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
            go_to_review: false,
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let initiator: String =
        sqlx::query_scalar("SELECT initiator FROM thread_summaries WHERE thread_id = $1")
            .bind(tid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(initiator, "system");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// CC sub-thread of a scheduled task inherits initiator='system'.
#[tokio::test]
async fn initiator_inherited_system_to_cc_child() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // Parent: trigger run (system-initiated)
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::TriggerStarted {
            trigger_id: "t-1".into(),
            trigger_name: Some("e2e tests".into()),
            prompt: None,
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
            go_to_review: false,
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Child: CC thread spawned by trigger run
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "run e2e tests".into(),
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

    let (child_init, child_source): (String, String) =
        sqlx::query_as("SELECT initiator, source FROM thread_summaries WHERE thread_id = $1")
            .bind(child_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        child_init, "system",
        "CC child of scheduled task must inherit system initiator"
    );
    assert_eq!(
        child_source, "claude_code",
        "CC child should have claude_code source"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// CC sub-thread of a user chat has initiator='user'.
#[tokio::test]
async fn initiator_inherited_user_to_cc_child() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // Parent: user chat
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "help me".into(),
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

    // Child: CC thread spawned by agentic loop
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "fix the bug".into(),
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

    let child_init: String =
        sqlx::query_scalar("SELECT initiator FROM thread_summaries WHERE thread_id = $1")
            .bind(child_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        child_init, "user",
        "CC child of user chat inherits user initiator"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// MessageReceived with source="system" creates system-initiated thread.
#[tokio::test]
async fn initiator_from_message_source_field() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tid = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: tid,
        event: ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "system message".into(),
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

    let initiator: String =
        sqlx::query_scalar("SELECT initiator FROM thread_summaries WHERE thread_id = $1")
            .bind(tid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(initiator, "system");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// SessionStarted ON CONFLICT does not overwrite initiator set by prior MessageReceived.
#[tokio::test]
async fn initiator_preserved_on_session_started_upsert() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent_id = Uuid::new_v4();
    let cc_thread_id = Uuid::new_v4();

    // Parent: trigger run (system)
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::TriggerStarted {
            trigger_id: "t-1".into(),
            trigger_name: Some("nightly".into()),
            prompt: None,
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
            go_to_review: false,
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // CC child: MessageReceived creates row with initiator=system (inherited)
    bus.emit(BusEvent::Thread {
        thread_id: cc_thread_id,
        event: ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "run tests".into(),
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

    // SessionStarted upserts the same row — must not reset initiator
    bus.emit(BusEvent::Thread {
        thread_id: cc_thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "s-1".into(),
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

    let initiator: String =
        sqlx::query_scalar("SELECT initiator FROM thread_summaries WHERE thread_id = $1")
            .bind(cc_thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        initiator, "system",
        "SessionStarted must not overwrite initiator"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// MessageReceived persists `spawning_event_id` into thread_summaries so a
/// system-spawned thread records the exact parent event that triggered it.
#[tokio::test]
async fn spawning_event_id_persists_for_system_spawn() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    let spawn_event = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "spawn a child".into(),
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
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "do work".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: Some(spawn_event),
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

    let stored: Option<Uuid> =
        sqlx::query_scalar("SELECT spawning_event_id FROM thread_summaries WHERE thread_id = $1")
            .bind(child_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stored,
        Some(spawn_event),
        "spawning_event_id must be persisted"
    );

    let parent_stored: Option<Uuid> =
        sqlx::query_scalar("SELECT spawning_event_id FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        parent_stored, None,
        "user-initiated thread must have NULL spawning_event_id"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ── Phase 0: Pre-refactor safety net ──
/// Cross-validate: verify that SystemEvent serialization round-trips correctly
/// for all persisted system event variants. If serialization breaks, events
/// written to the DB can't be read back on replay.
#[test]
fn persisted_system_events_round_trip_through_json() {
    let events = vec![
        SystemEvent::NotificationCreated {
            id: "n-1".into(),
            title: "Test".into(),
            message: "Hello".into(),
            task_id: Some("t-1".into()),
            app_id: Some("morning-brief".into()),
            thread_id: Some("33333333-3333-3333-3333-333333333333".into()),
            event_id: None,
            tap: crate::scheduler::notifications::Tap::Modal,
            actor: None,
        },
        SystemEvent::PreferencesChanged {
            key: "timezone".into(),
            value: Some("Europe/Oslo".into()),
            actor: None,
        },
    ];
    for event in &events {
        assert!(event.is_persisted(), "{:?} should be persisted", event);
        let payload = event.to_payload();
        // Verify payload has the expected envelope structure
        assert!(
            payload.get("type").is_some(),
            "Serialized payload for {:?} must have 'type' field",
            event
        );
        assert!(
            payload.get("data").is_some(),
            "Serialized payload for {:?} must have 'data' field",
            event
        );
    }
}
