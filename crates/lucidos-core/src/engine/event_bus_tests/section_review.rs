use super::super::*;
use super::*;

#[tokio::test]
async fn test_chat_parent_stays_default_on_child_spawn() {
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
            text: "do something".into(),
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

    let status: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "inbox");

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

    let status: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "inbox");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_section_inbox_on_child_complete() {
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

    let status: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "inbox");

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
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

    let status: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "inbox",
        "parent should be 'inbox' after child completes"
    );

    let child_status: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(child_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        child_status, "inbox",
        "a finished chat sub-thread must not be archived by the contract layer"
    );

    let archive_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1::text AND event_type = 'ThreadArchived'",
    )
    .bind(child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        archive_events, 0,
        "nothing archived the child, so nothing may record that it did"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_section_marked_read() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "hello".into(),
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
        event: ThreadEvent::ResponseGenerated {
            text: "Reply.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "inbox", "thread should be 'inbox' after completion");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "archived",
        "should be 'archived' after ThreadArchived"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn trigger_threads_skip_review() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            retry_of: None,
            provider: None,
            trigger_id: "t-1".into(),
            trigger_name: Some("daily".into()),
            prompt: None,
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

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Report done.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "archived",
        "scheduled task should stay 'archived', not appear in review"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn trigger_with_go_to_review_surfaces_first_response_in_review() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            retry_of: None,
            provider: None,
            trigger_id: "t-review".into(),
            trigger_name: Some("daily summary".into()),
            prompt: None,
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
            go_to_review: true,
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

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Here is your summary.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (archive_state, flag): (String, bool) = sqlx::query_as(
        "SELECT archive_state, trigger_go_to_review FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(flag, "trigger_go_to_review must be persisted on the row");
    assert_eq!(
        archive_state, "inbox",
        "trigger with go_to_review=true must surface in REVIEW on first response"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn trigger_followup_response_goes_to_review() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            retry_of: None,
            provider: None,
            trigger_id: "t-1".into(),
            trigger_name: Some("daily".into()),
            prompt: None,
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

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Report done.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "archived",
        "initial scheduled task response should stay in archive"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "Can you elaborate on the report?".into(),
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
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Here are the details...".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "inbox",
        "followup response on scheduled task thread should go to review"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn engine_message_received_does_not_promote_trigger_thread_to_review() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            retry_of: None,
            provider: None,
            trigger_id: "t-event".into(),
            trigger_name: Some("dashboard re-gen".into()),
            prompt: None,
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Event {
                event_type: "MorningLogged".into(),
                event_id: None,
                thread_id: None,
            }),
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

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "## Triggering Event\n\n```json\n{\"date\":\"2026-05-11\"}\n```".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Engine,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Dashboard regenerated.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "archived",
        "trigger thread without go_to_review must stay in ARCHIVE even when an engine-driven MessageReceived was emitted on it"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn message_received_destructures_parent_thread_id() {
    let parent_id = Uuid::new_v4();
    let thread_id = Uuid::new_v4();

    let event = ThreadEvent::MessageReceived {
        provider: None,
        voice_session_id: None,
        wake_mode: None,
        text: "fan-out message".into(),
        user_image_hashes: vec![],
        device_id: None,
        image_description: None,
        parent_thread_id: Some(parent_id),
        spawning_event_id: None,
        mode: ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    };

    if let ThreadEvent::MessageReceived {
        text,
        parent_thread_id,
        ..
    } = &event
    {
        assert_eq!(text, "fan-out message");
        assert_eq!(*parent_thread_id, Some(parent_id));
    } else {
        panic!("Expected MessageReceived");
    }

    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: Some(1),
        created: Utc::now(),
        typed: BusEvent::Thread {
            thread_id,
            event: event.clone(),
            meta: EventMeta::NONE,
        },
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
    };
    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(
        json["data"]["event"]["parent_thread_id"],
        parent_id.to_string()
    );

    let event_no_parent = ThreadEvent::MessageReceived {
        provider: None,
        voice_session_id: None,
        wake_mode: None,
        text: "follow-up".into(),
        user_image_hashes: vec![],
        device_id: None,
        image_description: None,
        parent_thread_id: None,
        spawning_event_id: None,
        mode: ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    };
    let emitted_no_parent = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: Some(2),
        created: Utc::now(),
        typed: BusEvent::Thread {
            thread_id,
            event: event_no_parent,
            meta: EventMeta::NONE,
        },
        aggregate: None,
        depth: 0,
        emitting_trigger_id: None,
    };
    let json2: serde_json::Value = serde_json::from_str(&emitted_no_parent.to_sse_json()).unwrap();
    assert!(json2["data"]["event"].get("parent_thread_id").is_none()
            || json2["data"]["event"]["parent_thread_id"].is_null(),
            "None parent_thread_id should be absent or null — follow-up messages must not clear it in projection");
}
