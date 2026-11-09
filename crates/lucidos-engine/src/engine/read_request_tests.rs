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
    record_reply_seen(pool, bus, thread_id, version, device(), &Reader::Team)
        .await
        .unwrap()
}

async fn start_chat_thread(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
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

    record_agent_read_decision(&bus, thread_id, ReadDecision::Requested)
        .await
        .unwrap();
    assert_eq!(see(&pool, &bus, thread_id).await, ReplySeen::Cleared);
    assert_eq!(see(&pool, &bus, thread_id).await, ReplySeen::NothingPending);
    assert_eq!(count_events(&pool, thread_id, "ThreadReplySeen").await, 1);

    assert_eq!(
        record_reply_seen(&pool, &bus, Uuid::new_v4(), 0, device(), &Reader::Team)
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

    record_agent_read_decision(&bus, thread_id, ReadDecision::Requested)
        .await
        .unwrap();
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
    record_agent_read_decision(&bus, thread_id, ReadDecision::Requested)
        .await
        .unwrap();

    assert_eq!(
        record_reply_seen(&pool, &bus, thread_id, seen_at, device(), &Reader::Team)
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

    record_agent_read_decision(&bus, thread_id, ReadDecision::Requested)
        .await
        .unwrap();
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

fn person(device_id: &str, devices: &[&str]) -> Reader {
    Reader::Person {
        device_id: device_id.into(),
        devices: devices.iter().map(|d| d.to_string()).collect(),
    }
}

async fn see_as(pool: &PgPool, bus: &EventBus, thread_id: Uuid, reader: &Reader) -> ReplySeen {
    let Reader::Person { device_id, .. } = reader else {
        panic!("a person reads here");
    };
    let version = summary_version(pool, thread_id).await;
    let actor = MessageOrigin::Device {
        device_id: device_id.clone(),
    };
    record_reply_seen(pool, bus, thread_id, version, actor, reader)
        .await
        .unwrap()
}

async fn seen_devices(pool: &PgPool, thread_id: Uuid) -> (bool, Vec<String>) {
    sqlx::query_as(
        "SELECT read_requested, reply_seen_devices FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// On a team gateway a sighting is one person's: Sam reading on one device
/// clears the request on Sam's other device, and leaves it for Kim.
#[tokio::test]
async fn a_person_sees_a_reply_for_themselves_only() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_chat_thread(&bus, thread_id).await;
    let sam_phone = person("sam-phone", &["sam-phone", "sam-laptop"]);
    let sam_laptop = person("sam-laptop", &["sam-phone", "sam-laptop"]);
    let kim = person("kim-phone", &["kim-phone"]);

    record_agent_read_decision(&bus, thread_id, ReadDecision::Requested)
        .await
        .unwrap();
    assert_eq!(
        see_as(&pool, &bus, thread_id, &sam_phone).await,
        ReplySeen::Cleared
    );
    assert_eq!(
        seen_devices(&pool, thread_id).await,
        (true, vec!["sam-phone".to_string()])
    );
    assert_eq!(
        see_as(&pool, &bus, thread_id, &sam_laptop).await,
        ReplySeen::NothingPending
    );
    assert_eq!(
        see_as(&pool, &bus, thread_id, &kim).await,
        ReplySeen::Cleared
    );
    assert_eq!(
        seen_devices(&pool, thread_id).await,
        (true, vec!["sam-phone".to_string(), "kim-phone".to_string()])
    );

    // A new request is unread for everyone again.
    record_agent_read_decision(&bus, thread_id, ReadDecision::Requested)
        .await
        .unwrap();
    assert_eq!(seen_devices(&pool, thread_id).await, (true, vec![]));

    pool.close().await;
    teardown_test_db(&db_name).await;
}
