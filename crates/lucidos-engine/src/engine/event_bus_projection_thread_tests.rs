//! Naming a thread in the `thread_summaries` projection.
//!
//! The contract these pin is an ordering one: a title event updates a row and
//! never creates it, so it has to follow the thread's first `MessageReceived`.
//! ADR 0240 holds the reasoning and the rejected upsert.

use super::write_thread_title;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};
use uuid::Uuid;

/// Emit a thread's first message, which is what creates its projection row.
async fn first_message(bus: &EventBus, thread_id: Uuid, parent: Option<Uuid>) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "one prompt-wording change in the engine".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: parent,
            spawning_event_id: None,
            mode: ActorMode::Agent,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn stored_title(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT title FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_optional(pool)
        .await
        .unwrap()
        .flatten()
}

/// A name with no row to land in is reported, not swallowed.
///
/// The `false` is the whole point. Its caller turns it into a log line naming
/// the thread, so a mis-ordered emit site is found on its first spawn.
#[tokio::test]
async fn a_name_arriving_before_the_thread_row_reports_that_it_was_dropped() {
    let (pool, db_name) = setup_test_db().await;
    let mut tx = pool.begin().await.unwrap();

    let applied = write_thread_title(&mut tx, Uuid::new_v4(), "Ask-card rule for dangling items")
        .await
        .unwrap();

    assert!(
        !applied,
        "a title for a thread with no row must report the drop, not claim success"
    );

    tx.commit().await.unwrap();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The ordinary case: the row is there, and it takes the name.
#[tokio::test]
async fn a_name_arriving_after_the_first_message_is_written() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    first_message(&bus, thread_id, None).await;

    let mut tx = pool.begin().await.unwrap();
    let applied = write_thread_title(&mut tx, thread_id, "Ask-card rule for dangling items")
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(applied, "a title for an existing thread must apply");
    assert_eq!(
        stored_title(&pool, thread_id).await.as_deref(),
        Some("Ask-card rule for dangling items")
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Why the title arm must never become an upsert.
///
/// `MessageReceived` writes `parent_thread_id`, `depth` and `initiator` on its
/// INSERT path only. A row conjured by an earlier title event would send the
/// real message down the conflict arm instead. The sub-thread would lose its
/// parent while the parent's child count still went up.
#[tokio::test]
async fn an_early_title_does_not_cost_a_sub_thread_its_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    first_message(&bus, parent_id, None).await;

    // The mis-ordered emit this whole change exists to prevent, replayed
    // deliberately: a title for a thread that does not exist yet.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ThreadTitleGenerated {
            title: "Ask-card rule for dangling items".into(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    first_message(&bus, child_id, Some(parent_id)).await;

    let (parent, depth): (Option<Uuid>, i32) =
        sqlx::query_as("SELECT parent_thread_id, depth FROM thread_summaries WHERE thread_id = $1")
            .bind(child_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(
        parent,
        Some(parent_id),
        "the child must keep its parent linkage, so the title event must not have created its row"
    );
    assert_eq!(depth, 1, "a child of a top-level thread sits at depth 1");

    pool.close().await;
    teardown_test_db(&db_name).await;
}
