//! Filed reports: I1, I2 and I3 of
//! `docs/plans/2026-10-11-sub-threads-wake-parents-only-when-needed.md`.
//!
//! A clean finish whose child said its parent need not act lands on the parent
//! as a `ChildReportFiled`, which starts no turn. Every other finish wakes.

use super::super::parent_callback::Delivery;
use super::*;
use crate::engine::thread_events::{AbortCause, CancelCause, ChildCompletionStatus};

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

fn reply(text: &str) -> ThreadEvent {
    ThreadEvent::ResponseGenerated {
        text: text.into(),
        images: vec![],
        model: None,
        reasoning_effort: None,
    }
}

async fn count_on(pool: &PgPool, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
           AND event_type = $2",
    )
    .bind(thread_id.to_string())
    .bind(event_type)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// What a wake would change on the parent's row.
async fn wake_columns(
    pool: &PgPool,
    thread_id: Uuid,
) -> (String, String, Option<chrono::DateTime<chrono::Utc>>) {
    sqlx::query_as(
        "SELECT status, archive_state, last_revived_at FROM thread_summaries \
          WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn callback_pending(pool: &PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT parent_callback_pending FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// I3 as a table: only a clean finish with an explicit no is filed.
#[test]
fn only_a_clean_finish_the_child_declined_is_filed() {
    use ChildCompletionStatus::*;
    for status in [Success, Failure, NoChanges, Canceled, Interrupted] {
        for decision in [None, Some(true), Some(false)] {
            let expected = if matches!(status, Success | NoChanges) && decision == Some(false) {
                Delivery::File
            } else {
                Delivery::Wake
            };
            assert_eq!(
                Delivery::for_report(status, None, decision, false),
                expected,
                "{status:?} with decision {decision:?}"
            );
        }
    }
}

/// I1: a filed report starts no turn and leaves the parent's row alone.
#[tokio::test]
async fn a_declined_chat_report_is_filed_without_waking_the_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    emit(&bus, parent_id, reply("Started a child for it.")).await;
    let before = wake_columns(&pool, parent_id).await;

    emit(&bus, child_id, ThreadEvent::ParentWakeNotRequested).await;
    emit(&bus, child_id, reply("The venue opens at nine.")).await;

    assert_eq!(count_on(&pool, parent_id, "ChildReportFiled").await, 1);
    assert_eq!(count_on(&pool, parent_id, "ChildThreadCompleted").await, 0);
    assert_eq!(drain_callbacks(&mut callback_rx), 0, "no parent turn");
    assert_eq!(
        wake_columns(&pool, parent_id).await,
        before,
        "status, archive state and revive stamp stay as they were"
    );
    assert!(
        !callback_pending(&pool, child_id).await,
        "the filed report settles what the child owed"
    );
    let summary: String = sqlx::query_scalar(
        "SELECT payload->>'summary' FROM events \
          WHERE aggregate_id = $1 AND event_type = 'ChildReportFiled'",
    )
    .bind(parent_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(summary, "The venue opens at nine.");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_report_the_child_wants_acted_on_still_wakes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;

    emit(&bus, child_id, ThreadEvent::ParentWakeRequested).await;
    emit(&bus, child_id, reply("Step one is done.")).await;

    assert_eq!(count_completion_cards(&pool, parent_id).await, 1);
    assert_eq!(count_on(&pool, parent_id, "ChildReportFiled").await, 0);
    assert_eq!(drain_callbacks(&mut callback_rx), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I4 at the fan-in: a turn that never decided wakes, as every child did.
#[tokio::test]
async fn an_undecided_report_wakes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;

    emit(&bus, child_id, reply("Done.")).await;

    assert_eq!(count_completion_cards(&pool, parent_id).await, 1);
    assert_eq!(drain_callbacks(&mut callback_rx), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I3: a no never quiets a finish that is not clean.
#[tokio::test]
async fn a_declined_report_still_wakes_for_every_unclean_finish() {
    let unclean = [
        ThreadEvent::ResponseFailed {
            error: "the API refused".into(),
        },
        ThreadEvent::ResponseCanceled {
            text: "partial".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: CancelCause::UserAction,
        },
    ];
    for terminal in unclean {
        let (pool, db_name) = setup_test_db().await;
        let (bus, mut callback_rx) = EventBus::new(pool.clone());
        let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
        let kind = terminal.event_type();

        emit(&bus, child_id, ThreadEvent::ParentWakeNotRequested).await;
        emit(&bus, child_id, terminal).await;

        assert_eq!(count_completion_cards(&pool, parent_id).await, 1, "{kind}");
        assert_eq!(
            count_on(&pool, parent_id, "ChildReportFiled").await,
            0,
            "{kind}"
        );
        assert_eq!(drain_callbacks(&mut callback_rx), 1, "{kind}");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

/// I3: a restart that cut the turn with no resume promised wakes over a no.
#[tokio::test]
async fn a_declined_report_cut_by_a_crash_still_wakes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;

    emit(&bus, child_id, ThreadEvent::ParentWakeNotRequested).await;
    emit(
        &bus,
        child_id,
        ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: AbortCause::RecoveryAfterRestart,
        },
    )
    .await;

    let (status, _) = newest_completion_card(&pool, parent_id).await;
    assert_eq!(status, "interrupted");
    assert_eq!(count_on(&pool, parent_id, "ChildReportFiled").await, 0);
    assert_eq!(drain_callbacks(&mut callback_rx), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_declined_coding_agent_idle_is_filed_with_its_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    emit(&bus, child_id, ThreadEvent::ParentWakeNotRequested).await;
    emit_cc_idled(&bus, child_id, false, None).await;
    // A second idle for the same turn owes the parent nothing more.
    emit_cc_idled(&bus, child_id, false, None).await;

    let status: String = sqlx::query_scalar(
        "SELECT payload->>'status' FROM events \
          WHERE aggregate_id = $1 AND event_type = 'ChildReportFiled'",
    )
    .bind(parent_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "no_changes");
    assert_eq!(count_completion_cards(&pool, parent_id).await, 0);
    assert_eq!(drain_callbacks(&mut callback_rx), 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I1 across a restart: a filed report that is a parent's last word is not
/// a lost wake, so the boot sweep re-fires nothing.
#[tokio::test]
async fn the_boot_sweep_never_refires_a_filed_report() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    emit(&bus, child_id, ThreadEvent::ParentWakeNotRequested).await;
    emit(&bus, child_id, reply("Filed.")).await;
    drop(bus);

    let (bus2, mut rx2) = EventBus::new(pool.clone());
    assert_eq!(bus2.refire_unprocessed_child_completions().await, 0);
    assert_eq!(drain_callbacks(&mut rx2), 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I2: a filed report after an unprocessed card is no reaction to it, so the
/// card's lost wake is still re-fired.
#[tokio::test]
async fn a_filed_report_does_not_hide_an_earlier_unprocessed_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent_id, waking_child) = spawn_parent_child(&bus, EventChannel::Chat).await;
    let filing_child = Uuid::new_v4();
    emit_thread_message(&bus, filing_child, Some(parent_id), "second task").await;

    emit(&bus, waking_child, reply("Needs you.")).await;
    emit(&bus, filing_child, ThreadEvent::ParentWakeNotRequested).await;
    emit(&bus, filing_child, reply("Just so you know.")).await;
    assert_eq!(count_completion_cards(&pool, parent_id).await, 1);
    assert_eq!(count_on(&pool, parent_id, "ChildReportFiled").await, 1);
    drop(bus);

    let (bus2, mut rx2) = EventBus::new(pool.clone());
    assert_eq!(bus2.refire_unprocessed_child_completions().await, 1);
    let cb = rx2.try_recv().expect("the card's wake is re-fired");
    assert_eq!(cb.child_thread_id, waking_child);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I5: a decision governs the one report it was made for. A chat child woken
/// by its own child's card starts its next turn with no start event, so it
/// must not reuse the spent no.
#[tokio::test]
async fn a_spent_no_never_files_the_next_report() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;

    emit(&bus, child_id, ThreadEvent::ParentWakeNotRequested).await;
    emit(&bus, child_id, reply("Waiting on my own child.")).await;
    assert_eq!(count_on(&pool, parent_id, "ChildReportFiled").await, 1);

    // The grandchild's card wakes the child, which writes no start event.
    emit(&bus, child_id, reply("Here is the combined result.")).await;

    assert_eq!(count_completion_cards(&pool, parent_id).await, 1);
    assert_eq!(count_on(&pool, parent_id, "ChildReportFiled").await, 1);
    assert_eq!(
        drain_callbacks(&mut callback_rx),
        1,
        "the result wakes the parent"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
