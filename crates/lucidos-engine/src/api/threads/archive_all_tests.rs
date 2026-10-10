//! Unarchive, the half of Archive all that needs no engine: it moves exactly
//! the archived ids back, and leaves every other row where it was.

use super::*;
use crate::engine::thread_events::{ActorMode, EventChannel};
use crate::test_support::{setup_test_db, teardown_test_db};

async fn spawn(bus: &EventBus, pool: &sqlx::PgPool) -> Uuid {
    spawn_under(bus, pool, None).await
}

async fn spawn_under(bus: &EventBus, pool: &sqlx::PgPool, parent: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "work".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: parent,
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
    sqlx::query("UPDATE thread_summaries SET status = 'idle' WHERE thread_id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn archive(bus: &EventBus, id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id: id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn section(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "test-device".into(),
    }
}

#[tokio::test]
async fn unarchive_moves_exactly_the_archived_ids_back() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let batch_a = spawn(&bus, &pool).await;
    let batch_b = spawn(&bus, &pool).await;
    let other_archived = spawn(&bus, &pool).await;
    let open = spawn(&bus, &pool).await;
    for id in [batch_a, batch_b, other_archived] {
        archive(&bus, id).await;
    }

    let back = unarchive_threads(
        &bus,
        &pool,
        &[batch_a, batch_b, open],
        UnarchiveScope::Exactly,
        device(),
    )
    .await
    .unwrap();
    assert_eq!(back.len(), 2);
    assert!(back.contains(&batch_a) && back.contains(&batch_b));
    for id in [batch_a, batch_b, open] {
        assert_eq!(section(&pool, id).await, "inbox");
    }
    assert_eq!(
        section(&pool, other_archived).await,
        "archived",
        "a thread outside the batch stays archived"
    );

    teardown_test_db(&db).await;
}

/// ADR 0312 keeps pinned and archived exclusive. An unarchived thread pins and
/// unpins like any inbox thread, and the constraint never fires.
#[tokio::test]
async fn an_unarchived_thread_pins_and_unpins_cleanly() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let id = spawn(&bus, &pool).await;
    archive(&bus, id).await;
    unarchive_threads(&bus, &pool, &[id], UnarchiveScope::Exactly, device())
        .await
        .unwrap();
    for event in [ThreadEvent::ThreadSaved, ThreadEvent::ThreadUnsaved] {
        bus.emit(BusEvent::Thread {
            thread_id: id,
            event,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }
    assert_eq!(section(&pool, id).await, "inbox");

    teardown_test_db(&db).await;
}

/// Move to Current brings the sub-threads back with the thread, as Archive took
/// them; Undo's exact scope leaves them where they are.
#[tokio::test]
async fn unarchive_with_sub_threads_restores_the_family() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = spawn(&bus, &pool).await;
    let child = spawn_under(&bus, &pool, Some(parent)).await;
    let grandchild = spawn_under(&bus, &pool, Some(child)).await;
    for id in [parent, child, grandchild] {
        archive(&bus, id).await;
    }

    let exact = unarchive_threads(&bus, &pool, &[parent], UnarchiveScope::Exactly, device())
        .await
        .unwrap();
    assert_eq!(exact, vec![parent]);
    assert_eq!(section(&pool, child).await, "archived");

    let family = unarchive_threads(
        &bus,
        &pool,
        &[parent],
        UnarchiveScope::WithSubThreads,
        device(),
    )
    .await
    .unwrap();
    assert_eq!(family, vec![child, grandchild], "parents first");
    for id in [parent, child, grandchild] {
        assert_eq!(section(&pool, id).await, "inbox");
    }

    teardown_test_db(&db).await;
}
