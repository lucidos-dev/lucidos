use super::*;
use crate::engine::thread_events::{ActorMode, EventChannel};
use crate::test_support::{setup_test_db, teardown_test_db};

fn device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "my-iphone".into(),
    }
}

async fn count_events(pool: &PgPool, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = $2")
        .bind(thread_id.to_string())
        .bind(event_type)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn summary_version(pool: &PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT summary_version FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn see(pool: &PgPool, bus: &EventBus, thread_id: Uuid) -> ReplySeen {
    let version = summary_version(pool, thread_id).await;
    record_reply_seen(pool, bus, thread_id, version, device())
        .await
        .unwrap()
}

async fn start_chat_thread(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "write me the report".into(),
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
}

/// `ThreadReplySeen` is recorded only on a real flip, so a second device
/// seeing the same reply adds nothing.
#[tokio::test]
async fn a_sighting_is_recorded_once_per_request() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_chat_thread(&bus, thread_id).await;

    assert_eq!(see(&pool, &bus, thread_id).await, ReplySeen::NothingPending);

    record_agent_read_request(&bus, thread_id).await.unwrap();
    assert_eq!(see(&pool, &bus, thread_id).await, ReplySeen::Cleared);
    assert_eq!(see(&pool, &bus, thread_id).await, ReplySeen::NothingPending);
    assert_eq!(count_events(&pool, thread_id, "ThreadReplySeen").await, 1);

    assert_eq!(
        record_reply_seen(&pool, &bus, Uuid::new_v4(), 0, device())
            .await
            .unwrap(),
        ReplySeen::UnknownThread
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A sighting taken before a newer request is stale: it records nothing and
/// leaves the newer request pending.
#[tokio::test]
async fn a_sighting_older_than_a_new_request_clears_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_chat_thread(&bus, thread_id).await;

    record_agent_read_request(&bus, thread_id).await.unwrap();
    let seen_at = summary_version(&pool, thread_id).await;
    // A newer request rides a newer reply, which moves the summary on.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "A newer report.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    record_agent_read_request(&bus, thread_id).await.unwrap();

    assert_eq!(
        record_reply_seen(&pool, &bus, thread_id, seen_at, device())
            .await
            .unwrap(),
        ReplySeen::Stale
    );
    assert_eq!(count_events(&pool, thread_id, "ThreadReplySeen").await, 0);
    let pending: bool =
        sqlx::query_scalar("SELECT read_requested FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(pending);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The request names the agent thread that asked, as an agent archive does.
#[tokio::test]
async fn a_request_names_the_asking_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_chat_thread(&bus, thread_id).await;

    record_agent_read_request(&bus, thread_id).await.unwrap();
    let source: Option<String> = sqlx::query_scalar(
        "SELECT payload->'actor'->>'source_thread_id' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ThreadReadRequested'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(source, Some(thread_id.to_string()));

    pool.close().await;
    teardown_test_db(&db_name).await;
}
