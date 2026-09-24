//! A thread waiting on the user is never archived, by any path. See
//! `docs/plans/2026-09-23-a-thread-waiting-on-the-user-cannot-be-archived.md`.

use super::super::*;
use super::*;

async fn emit_unattended_trigger_start(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            provider: None,
            trigger_id: "t-hourly".into(),
            trigger_name: Some("hourly".into()),
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
}

async fn emit_question(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: "tu-1".into(),
            cc_session_id: String::new(),
            question: "Which angle tonight?".into(),
            options: vec![],
            worktree_path: None,
            multi_select: false,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn read_status_and_section(pool: &PgPool, thread_id: Uuid) -> (String, String) {
    sqlx::query_as("SELECT status, archive_state FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The live path: an unattended trigger run asked the user a question, and
/// the unattended guard rewrote the question's Inbox to Archived. The thread
/// then left Current and Needs attention with the question still open.
#[tokio::test]
async fn an_unattended_trigger_run_that_asks_the_user_stays_in_the_inbox() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    emit_unattended_trigger_start(&bus, thread_id).await;
    emit_question(&bus, thread_id).await;

    assert_eq!(
        read_status_and_section(&pool, thread_id).await,
        ("waiting_for_user_answer".to_string(), "inbox".to_string()),
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The EventBus backstop. Whatever route emits it, `ThreadArchived` on a
/// parked thread is refused: nothing is persisted and the row is untouched.
#[tokio::test]
async fn the_bus_refuses_to_archive_a_parked_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    emit_thread_message(&bus, thread_id, None, "plan the launch").await;
    emit_question(&bus, thread_id).await;

    let refused = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ThreadArchived,
            meta: EventMeta::NONE,
        })
        .await;
    assert!(
        refused.is_err(),
        "archiving a parked thread must be refused"
    );

    let archived_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1::text \
         AND event_type = 'ThreadArchived'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(archived_rows, 0, "a refused archive persists no event");
    assert_eq!(
        read_status_and_section(&pool, thread_id).await,
        ("waiting_for_user_answer".to_string(), "inbox".to_string()),
        "a refused archive leaves the question open and the thread in the inbox",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Threads a pre-fix engine archived while parked are repaired at boot, with
/// no manual step. Replaying their events through the fixed contract yields
/// `inbox`, so the migration only makes the projection agree with its events.
#[tokio::test]
async fn the_migration_unarchives_threads_stuck_waiting_for_the_user() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let stuck = Uuid::new_v4();
    let archived_idle = Uuid::new_v4();

    emit_thread_message(&bus, stuck, None, "ask me something").await;
    emit_question(&bus, stuck).await;
    emit_thread_message(&bus, archived_idle, None, "all done").await;
    sqlx::query(
        "UPDATE thread_summaries SET archive_state = 'archived' \
         WHERE thread_id = ANY($1)",
    )
    .bind(vec![stuck, archived_idle])
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE thread_summaries SET status = 'idle' WHERE thread_id = $1")
        .bind(archived_idle)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::raw_sql(include_str!(
        "../../../migrations/20260923162316_unarchive_threads_waiting_for_user_answer.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(
        read_status_and_section(&pool, stuck).await,
        ("waiting_for_user_answer".to_string(), "inbox".to_string()),
        "a parked thread leaves the archive",
    );
    assert_eq!(
        read_status_and_section(&pool, archived_idle).await,
        ("idle".to_string(), "archived".to_string()),
        "an idle archived thread stays archived",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
