//! A custom folder only files a pin: `pin_folder` is set while the thread is
//! pinned, and every path that unpins clears it.

use super::super::*;
use super::*;

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

fn move_to(folder: Option<&str>) -> ThreadEvent {
    ThreadEvent::ThreadMovedToFolder {
        folder: folder.map(str::to_string),
    }
}

async fn pin_folder(pool: &PgPool, thread_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT pin_folder FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn pinned_thread_in_folder(bus: &EventBus, pool: &PgPool) -> Uuid {
    let thread_id = Uuid::new_v4();
    emit_thread_message(bus, thread_id, None, "hello").await;
    emit(bus, thread_id, ThreadEvent::ThreadSaved).await;
    emit(bus, thread_id, move_to(Some("w48"))).await;
    assert_eq!(pin_folder(pool, thread_id).await.as_deref(), Some("w48"));
    thread_id
}

#[tokio::test]
async fn a_move_files_and_unfiles_a_pinned_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = pinned_thread_in_folder(&bus, &pool).await;

    emit(&bus, thread_id, move_to(None)).await;
    assert_eq!(pin_folder(&pool, thread_id).await, None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn unpin_and_archive_clear_the_folder() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let unpinned = pinned_thread_in_folder(&bus, &pool).await;
    emit(&bus, unpinned, ThreadEvent::ThreadUnsaved).await;
    assert_eq!(pin_folder(&pool, unpinned).await, None);

    let archived = pinned_thread_in_folder(&bus, &pool).await;
    emit(&bus, archived, ThreadEvent::ThreadArchived).await;
    assert_eq!(pin_folder(&pool, archived).await, None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A replayed move on an unpinned thread writes nothing rather than tripping
/// the CHECK and failing the whole emit.
#[tokio::test]
async fn a_move_on_an_unpinned_thread_writes_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "hello").await;

    emit(&bus, thread_id, move_to(Some("w48"))).await;
    assert_eq!(pin_folder(&pool, thread_id).await, None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_table_refuses_a_folder_on_an_unpinned_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "hello").await;

    let err = sqlx::query("UPDATE thread_summaries SET pin_folder = 'w48' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .expect_err("a folder on an unpinned row must not be storable");
    assert!(
        err.to_string()
            .contains("thread_summaries_folder_is_pinned"),
        "refused by the named constraint, got: {err}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
