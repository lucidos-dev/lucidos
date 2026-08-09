use super::super::*;
use super::*;

#[tokio::test]
async fn response_aborted_surfaces_chat_thread_to_inbox() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    // User sends message
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
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(section, "inbox");

    // ResponseAborted (engine crash recovery)
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseAborted {
            text: "This response was interrupted by an engine restart.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Verify thread is now in inbox
    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "inbox",
        "ResponseAborted should surface chat thread to inbox"
    );

    // Verify has_response is true
    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        has_response,
        "ResponseAborted should set has_response = true"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn response_canceled_sets_has_response_true() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    // User sends message — creates thread_summaries row with has_response=false
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
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!has_response, "has_response should start as false");

    // User cancels the response
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseCanceled {
            text: "partial".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // has_response must be true — a canceled response is still a response.
    // Without this, the thread won't appear in get_recent_threads (which
    // filters has_response=TRUE) and becomes invisible after page reload.
    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        has_response,
        "ResponseCanceled should set has_response = true"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn trigger_completed_sets_has_response_true() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    // TriggerStarted creates the thread_summaries row
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            trigger_id: "t-1".into(),
            trigger_name: Some("job-tracker".into()),
            prompt: Some("Check jobs".into()),
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

    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        !has_response,
        "has_response should start as false after TriggerStarted"
    );

    // TriggerCompleted — the trigger ran and finished
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerCompleted {
            trigger_id: "t-1".into(),
            trigger_name: Some("job-tracker".into()),
            result_summary: Some("Found 3 jobs".into()),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // has_response must be true — a completed trigger run should appear in
    // get_recent_threads (which filters has_response=TRUE).
    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        has_response,
        "TriggerCompleted should set has_response = true"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn claude_code_idled_sets_has_response_true() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "", None).await;

    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        !has_response,
        "has_response should start as false after SessionStarted"
    );

    emit_cc_idle(&bus, thread_id, false, None).await;

    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        has_response,
        "CodingAgentIdled should set has_response = true"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn session_ended_sets_has_response_true() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "", None).await;

    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!has_response, "has_response should start as false");

    // Session ends terminally (e.g. shutdown, panic, or user closed thread).
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionEnded {
            reason: SessionEndReason::Shutdown,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let has_response: bool =
        sqlx::query_scalar("SELECT has_response FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        has_response,
        "SessionEnded (terminal) should set has_response = true"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
