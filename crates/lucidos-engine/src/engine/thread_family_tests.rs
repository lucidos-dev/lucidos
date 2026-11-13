use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::test_support::{message_received, seed_chat_thread, setup_test_db, teardown_test_db};

pub(crate) async fn seed_child(bus: &EventBus, parent: Uuid) -> Uuid {
    let child = Uuid::new_v4();
    let mut event = message_received("child task");
    if let ThreadEvent::MessageReceived {
        parent_thread_id, ..
    } = &mut event
    {
        *parent_thread_id = Some(parent);
    }
    bus.emit(BusEvent::Thread {
        thread_id: child,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    child
}

#[tokio::test]
async fn every_descendant_resolves_to_the_first_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let root = seed_chat_thread(&bus, "root").await;
    let child = seed_child(&bus, root).await;
    let grandchild = seed_child(&bus, child).await;

    for thread in [root, child, grandchild] {
        assert_eq!(family_root(&pool, thread).await.unwrap(), root);
    }
    let arm = crate::engine::auto_compact_trial::arm_for_family;
    assert_eq!(
        arm(family_root(&pool, grandchild).await.unwrap()),
        arm(root),
        "a sub-thread runs in its root's arm"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_thread_with_no_row_is_its_own_root() {
    let (pool, db) = setup_test_db().await;
    let stranger = Uuid::new_v4();
    assert_eq!(family_root(&pool, stranger).await.unwrap(), stranger);
    teardown_test_db(&db).await;
}
