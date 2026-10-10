//! A pinned thread is never archived (ADR 0312). Every path that archives or
//! pins keeps the pair apart, and the table refuses it outright.

use super::super::*;
use super::*;

const REPAIR: &str =
    include_str!("../../../migrations/20260927162831_pinned_thread_is_never_archived.sql");

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn emit_reply(bus: &EventBus, thread_id: Uuid) {
    emit(
        bus,
        thread_id,
        ThreadEvent::ResponseGenerated {
            text: "Done.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
    )
    .await;
}

/// `(archive_state, is_saved)` for one thread.
async fn retention(pool: &PgPool, thread_id: Uuid) -> (String, bool) {
    sqlx::query_as("SELECT archive_state, is_saved FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn pinning_an_archived_thread_moves_it_to_the_inbox() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    emit_thread_message(&bus, thread_id, None, "hello").await;
    emit_reply(&bus, thread_id).await;
    emit(&bus, thread_id, ThreadEvent::ThreadArchived).await;
    assert_eq!(
        retention(&pool, thread_id).await,
        ("archived".into(), false)
    );

    emit(&bus, thread_id, ThreadEvent::ThreadSaved).await;
    assert_eq!(
        retention(&pool, thread_id).await,
        ("inbox".into(), true),
        "the pin brings the thread back to the inbox in the same emit"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn archiving_a_pinned_thread_unpins_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    emit_thread_message(&bus, thread_id, None, "hello").await;
    emit_reply(&bus, thread_id).await;
    emit(&bus, thread_id, ThreadEvent::ThreadSaved).await;
    assert_eq!(retention(&pool, thread_id).await, ("inbox".into(), true));

    emit(&bus, thread_id, ThreadEvent::ThreadArchived).await;
    assert_eq!(
        retention(&pool, thread_id).await,
        ("archived".into(), false),
        "the user's Archive unpins in the same emit"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The automatic archive of an unattended trigger run skips a pinned thread.
/// An unpinned run of the same shape still archives, which the first half
/// shows before the pin.
#[tokio::test]
async fn an_unattended_trigger_run_never_archives_a_pinned_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            provider: None,
            trigger_id: "t-pinned".into(),
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
    emit_reply(&bus, thread_id).await;
    assert_eq!(
        retention(&pool, thread_id).await,
        ("archived".into(), false),
        "an unpinned unattended run still archives"
    );

    emit(&bus, thread_id, ThreadEvent::ThreadSaved).await;
    assert_eq!(retention(&pool, thread_id).await, ("inbox".into(), true));

    emit_reply(&bus, thread_id).await;
    assert_eq!(
        retention(&pool, thread_id).await,
        ("inbox".into(), true),
        "a pinned trigger thread stays in the inbox, still pinned"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn discarding_a_pinned_draft_unpins_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    emit(
        &bus,
        thread_id,
        ThreadEvent::ThreadStarted {
            mode: "chat".into(),
            actor: None,
        },
    )
    .await;
    emit(&bus, thread_id, ThreadEvent::ThreadSaved).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::ThreadDiscarded { actor: None },
    )
    .await;
    assert_eq!(
        retention(&pool, thread_id).await,
        ("archived".into(), false)
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_table_refuses_a_pinned_archived_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    emit_thread_message(&bus, thread_id, None, "hello").await;
    emit(&bus, thread_id, ThreadEvent::ThreadSaved).await;

    let err =
        sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
            .bind(thread_id)
            .execute(&pool)
            .await
            .expect_err("a pinned archived row must not be storable");
    assert!(
        err.to_string()
            .contains("thread_summaries_pinned_is_not_archived"),
        "refused by the named constraint, got: {err}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The migration already ran on this database, so the test turns the
/// constraint off, seeds the rows it repairs, and runs the same file again.
#[tokio::test]
async fn the_repair_keeps_the_pin_and_moves_the_thread_to_the_inbox() {
    let (pool, db_name) = setup_test_db().await;
    let pinned = Uuid::new_v4();
    let discarded = Uuid::new_v4();
    let archived = Uuid::new_v4();

    sqlx::query(
        "ALTER TABLE thread_summaries DROP CONSTRAINT thread_summaries_pinned_is_not_archived",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, state, archive_state, is_saved) VALUES \
         ($1, 'active', 'archived', TRUE), \
         ($2, 'discarded', 'archived', TRUE), \
         ($3, 'active', 'archived', FALSE)",
    )
    .bind(pinned)
    .bind(discarded)
    .bind(archived)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::raw_sql(REPAIR).execute(&pool).await.unwrap();

    assert_eq!(
        retention(&pool, pinned).await,
        ("inbox".into(), true),
        "the pin wins: the thread moves to the inbox"
    );
    assert_eq!(
        retention(&pool, discarded).await,
        ("archived".into(), false),
        "a discarded draft is unpinned instead"
    );
    assert_eq!(
        retention(&pool, archived).await,
        ("archived".into(), false),
        "an unpinned archived thread is untouched"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
