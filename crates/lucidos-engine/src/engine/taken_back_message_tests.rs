use crate::engine::event_bus::{BusEvent, EmitResult, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::test_support::{seed_chat_thread, setup_test_db, teardown_test_db};
use uuid::Uuid;

async fn remove(bus: &EventBus, thread_id: Uuid, message: Uuid) -> EmitResult {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::QueuedMessageRemoved {
            removed_message_id: message,
            restore_to_compose: false,
            compose: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap()
    .unwrap()
}

#[tokio::test]
async fn a_taken_back_user_only_message_completes_without_posting() {
    use crate::engine::thread_users::{post_user_only_message, UserOnlyMessage};
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let thread = seed_chat_thread(&bus, "active").await;
    let message = Uuid::new_v4();
    remove(&bus, thread, message).await;
    post_user_only_message(
        &bus,
        thread,
        UserOnlyMessage {
            text: "taken back".into(),
            image_hashes: vec![],
            device_id: None,
            mentions: vec![],
            event_id: Some(message),
        },
        None,
    )
    .await
    .unwrap();
    let posted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'UserOnlyMessagePosted'",
    )
    .bind(thread)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(posted, 0);
    pool.close().await;
    teardown_test_db(&db).await;
}
