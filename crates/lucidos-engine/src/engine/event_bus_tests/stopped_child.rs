//! A user Stop on a child thread makes a *stopped child* (ADR 0252).
//!
//! The parent gets a `ChildThreadStopped` note and no turn. It is still owed
//! a `ChildThreadCompleted`, which the child's next finished turn or a settle
//! (Archive, Discard) delivers. Each test pins one invariant of
//! `docs/plans/2026-09-23-stopped-child.md`.

use super::super::ChildSettle;
use super::*;
use crate::engine::thread_events::CancelCause;

async fn count_events(pool: &PgPool, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = $2")
        .bind(thread_id.to_string())
        .bind(event_type)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// `(is_stopped_child, parent_callback_pending)` for `thread_id`.
async fn read_stop_state(pool: &PgPool, thread_id: Uuid) -> (bool, bool) {
    sqlx::query_as(
        "SELECT is_stopped_child, parent_callback_pending FROM thread_summaries \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// `(blocking_descendant_count, attention_descendant_count)` for `thread_id`.
async fn read_descendant_counts(pool: &PgPool, thread_id: Uuid) -> (i32, i32) {
    sqlx::query_as(
        "SELECT blocking_descendant_count, attention_descendant_count \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn read_status(pool: &PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Settle the parent's own turn, so a wake would show as a status flip.
async fn settle_parent(bus: &EventBus, parent_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::ResponseGenerated {
            text: "spawned the child".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// The incident. A Stop on a coding-agent child, with the idle the interrupt
/// emits right after it, must leave the parent asleep with one note.
#[tokio::test]
async fn a_user_stop_on_a_coding_agent_child_notes_the_parent_without_waking_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    settle_parent(&bus, parent_id).await;

    emit_response_canceled_with_cause(&bus, child_id, CancelCause::UserStop).await;
    // The interrupt's own idle. The marker is still set, so without the
    // stopped-child gate this is what reached the parent as "completed".
    emit_cc_idle(&bus, child_id, false, None).await;

    assert_eq!(
        count_events(&pool, parent_id, "ChildThreadCompleted").await,
        0,
        "a user Stop must not send the parent a completion card"
    );
    assert_eq!(
        count_events(&pool, parent_id, "ChildThreadStopped").await,
        1,
        "it sends exactly one ChildThreadStopped note"
    );
    assert_eq!(
        drain_callbacks(&mut callback_rx),
        0,
        "and it must not wake the parent"
    );
    assert_eq!(
        read_status(&pool, parent_id).await,
        "idle",
        "the note writes no status on the parent"
    );
    assert_eq!(
        read_stop_state(&pool, child_id).await,
        (true, true),
        "the child is a stopped child, and the parent is still owed its card"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Same contract on the Lucidos Agent lane, which reaches `should_callback`
/// through a different arm.
#[tokio::test]
async fn a_user_stop_on_a_chat_child_notes_the_parent_without_waking_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    emit_response_canceled_with_cause(&bus, child_id, CancelCause::UserStop).await;

    assert_eq!(
        count_events(&pool, parent_id, "ChildThreadCompleted").await,
        0
    );
    assert_eq!(
        count_events(&pool, parent_id, "ChildThreadStopped").await,
        1
    );
    assert_eq!(drain_callbacks(&mut callback_rx), 0);
    assert_eq!(read_stop_state(&pool, child_id).await, (true, true));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A cancel that Apply, Discard or Archive carried still reports as before:
/// only `UserStop` pauses.
#[tokio::test]
async fn a_user_action_cancel_still_reports_a_cancellation_to_the_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_response_canceled_with_cause(&bus, child_id, CancelCause::UserAction).await;

    assert_eq!(
        count_events(&pool, parent_id, "ChildThreadCompleted").await,
        1
    );
    assert_eq!(newest_completion_card(&pool, parent_id).await.0, "canceled");
    assert_eq!(
        count_events(&pool, parent_id, "ChildThreadStopped").await,
        0
    );
    assert_eq!(drain_callbacks(&mut callback_rx), 1);
    assert!(!read_stop_state(&pool, child_id).await.0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A Stop the parent is owed nothing for says nothing. The marker is clear
/// because this turn already reported, so there is no stopped child to make.
#[tokio::test]
async fn a_user_stop_after_the_turn_already_reported_is_silent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, false, None).await;
    assert_eq!(
        drain_callbacks(&mut callback_rx),
        1,
        "baseline: the turn reported once"
    );

    emit_response_canceled_with_cause(&bus, child_id, CancelCause::UserStop).await;

    assert_eq!(
        count_events(&pool, parent_id, "ChildThreadCompleted").await,
        1
    );
    assert_eq!(
        count_events(&pool, parent_id, "ChildThreadStopped").await,
        0
    );
    assert_eq!(drain_callbacks(&mut callback_rx), 0);
    assert_eq!(read_stop_state(&pool, child_id).await, (false, false));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The user types into the stopped child and it keeps working. Its next
/// finished turn is what reports, exactly once, with the real status.
#[tokio::test]
async fn a_stopped_child_that_continues_reports_its_next_turn_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_response_canceled_with_cause(&bus, child_id, CancelCause::UserStop).await;
    emit_cc_idle(&bus, child_id, false, None).await;

    emit_thread_message(&bus, child_id, None, "do it this way instead").await;
    assert_eq!(
        read_stop_state(&pool, child_id).await,
        (false, true),
        "new work ends the stopped state, and its turn owes the parent a card"
    );
    emit_cc_idle(&bus, child_id, false, None).await;

    assert_eq!(
        count_events(&pool, parent_id, "ChildThreadCompleted").await,
        1
    );
    assert_eq!(
        newest_completion_card(&pool, parent_id).await.0,
        "no_changes"
    );
    assert_eq!(
        drain_callbacks(&mut callback_rx),
        1,
        "one wake, for the finished turn"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Archive and Discard are the acts that mean "done". Each sends the one
/// canceled card the parent was owed and wakes it. A second settle is a no-op.
#[tokio::test]
async fn settling_a_stopped_child_sends_one_canceled_card_and_wakes_the_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    settle_parent(&bus, parent_id).await;
    emit_response_canceled_with_cause(&bus, child_id, CancelCause::UserStop).await;
    emit_cc_idle(&bus, child_id, false, None).await;

    bus.settle_child(child_id, ChildSettle::Archived).await;

    assert_eq!(
        count_events(&pool, parent_id, "ChildThreadCompleted").await,
        1
    );
    let (status, summary) = newest_completion_card(&pool, parent_id).await;
    assert_eq!(status, "canceled");
    assert!(
        summary.contains("archived"),
        "the card says what the user did, got {summary:?}"
    );
    assert_eq!(
        drain_callbacks(&mut callback_rx),
        1,
        "the settle wakes the parent"
    );
    assert_eq!(read_status(&pool, parent_id).await, "running");
    assert_eq!(
        read_stop_state(&pool, child_id).await,
        (false, false),
        "the card settles both the stopped state and the marker"
    );

    bus.settle_child(child_id, ChildSettle::Discarded).await;
    assert_eq!(
        count_events(&pool, parent_id, "ChildThreadCompleted").await,
        1,
        "a settled child is owed nothing more"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A settle on a thread that is not a stopped child does nothing, so the
/// archive and discard paths can call it for every thread.
#[tokio::test]
async fn settling_a_thread_that_is_not_a_stopped_child_is_a_no_op() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    bus.settle_child(child_id, ChildSettle::Archived).await;
    bus.settle_child(parent_id, ChildSettle::Discarded).await;

    assert_eq!(
        count_events(&pool, parent_id, "ChildThreadCompleted").await,
        0
    );
    assert_eq!(drain_callbacks(&mut callback_rx), 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A stopped child needs the user, so it counts toward its parent's
/// attention. It must not count toward blocking, or archiving the parent
/// would be refused. Both new work and a settle take it back out.
#[tokio::test]
async fn a_stopped_child_counts_toward_attention_and_never_toward_blocking() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_response_canceled_with_cause(&bus, child_id, CancelCause::UserStop).await;
    emit_cc_idle(&bus, child_id, false, None).await;

    assert_eq!(
        read_descendant_counts(&pool, parent_id).await,
        (0, 1),
        "stopped: attention 1, blocking 0"
    );

    emit_thread_message(&bus, child_id, None, "keep going").await;
    assert_eq!(
        read_descendant_counts(&pool, parent_id).await.1,
        0,
        "new work takes the child out of attention"
    );

    emit_response_canceled_with_cause(&bus, child_id, CancelCause::UserStop).await;
    assert_eq!(read_descendant_counts(&pool, parent_id).await, (0, 1));
    bus.settle_child(child_id, ChildSettle::Archived).await;
    assert_eq!(
        read_descendant_counts(&pool, parent_id).await.1,
        0,
        "the settle, a write on the child inside the parent's event, still \
         reconciles the parent's attention count"
    );

    // The boot rebuild is a separate SQL mirror of the same predicate.
    emit_thread_message(&bus, child_id, None, "once more").await;
    emit_response_canceled_with_cause(&bus, child_id, CancelCause::UserStop).await;
    sqlx::query("UPDATE thread_summaries SET attention_descendant_count = 0")
        .execute(&pool)
        .await
        .unwrap();
    EventBus::rebuild_blocking_descendant_count(&pool)
        .await
        .unwrap();
    assert_eq!(
        read_descendant_counts(&pool, parent_id).await,
        (0, 1),
        "the rebuild mirror agrees with the live predicate"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A Stop on a top-thread has no parent to tell, and changes nothing.
#[tokio::test]
async fn a_user_stop_on_a_top_thread_changes_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "just me").await;
    emit_response_canceled_with_cause(&bus, thread_id, CancelCause::UserStop).await;

    assert_eq!(read_stop_state(&pool, thread_id).await, (false, false));
    let notes: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = 'ChildThreadStopped'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(notes, 0);
    assert_eq!(drain_callbacks(&mut callback_rx), 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// ADR 0011's boot sweep re-fires a card that is the parent's last event. A
/// sibling's later stopped note wakes nothing, so it must not hide that card.
#[tokio::test]
async fn a_stopped_note_does_not_hide_an_unprocessed_card_from_the_boot_sweep() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, finished_child) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    let stopped_child = Uuid::new_v4();
    emit_thread_message(&bus, stopped_child, Some(parent_id), "second task").await;
    emit_cc_session_started(&bus, finished_child).await;

    emit_cc_idle(&bus, finished_child, false, None).await;
    emit_response_canceled_with_cause(&bus, stopped_child, CancelCause::UserStop).await;
    assert_eq!(
        drain_callbacks(&mut callback_rx),
        1,
        "baseline: one live wake, then lost"
    );
    drop(callback_rx);
    drop(bus);

    let (bus2, mut rx2) = EventBus::new(pool.clone());
    assert_eq!(
        bus2.refire_unprocessed_child_completions().await,
        1,
        "the unprocessed card is still re-fired past the later note"
    );
    assert_eq!(drain_callbacks(&mut rx2), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An agent cancelling its own child records the same `user_stop` cause as a
/// person's Stop. It expects the canceled card and gets it: its actor says it
/// is not the user.
#[tokio::test]
async fn an_agents_cancel_of_its_own_child_still_reports_a_cancellation() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseCanceled {
            text: "partial work".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: CancelCause::UserStop,
        },
        meta: EventMeta::with_actor(Some(MessageOrigin::Api {
            user_agent: None,
            mode: ActorMode::Agent,
            source_thread_id: Some(parent_id),
        })),
    })
    .await
    .unwrap();

    assert_eq!(count_completion_cards(&pool, parent_id).await, 1);
    assert_eq!(newest_completion_card(&pool, parent_id).await.0, "canceled");
    assert_eq!(
        count_events(&pool, parent_id, "ChildThreadStopped").await,
        0
    );
    assert_eq!(drain_callbacks(&mut callback_rx), 1);
    assert!(!read_stop_state(&pool, child_id).await.0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A cascade from the parent archives a stopped child without settling it.
/// The archive clears the flag, so nothing keeps counting a child the user
/// put away: not the badge, not the notice, not the worktree hold.
#[tokio::test]
async fn archiving_a_stopped_child_clears_the_flag() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (_parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_response_canceled_with_cause(&bus, child_id, CancelCause::UserStop).await;
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(
        read_stop_state(&pool, child_id).await,
        (false, true),
        "the flag clears; the marker stays, so a direct archive still settles"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A discard leaves a stopped child's event wait alone, and the wait wakes
/// the child again. So a discard settles nothing then. An archive ends the
/// wait, and settles.
#[tokio::test]
async fn a_discard_does_not_settle_a_stopped_child_that_holds_a_wait() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::EventWaitStarted {
            wait_id: Uuid::new_v4(),
            tool_use_id: "toolu_wait".into(),
            on: vec![crate::core::event_subscription::EventSubscription {
                event_type: "BenchSlotFreed".into(),
                condition: None,
            }],
            reason: "waiting for the bench slot".into(),
            armed_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            watermark: 0,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    emit_response_canceled_with_cause(&bus, child_id, CancelCause::UserStop).await;
    drain_callbacks(&mut callback_rx);

    bus.settle_child(child_id, ChildSettle::Discarded).await;
    assert_eq!(count_completion_cards(&pool, parent_id).await, 0);

    bus.settle_child(child_id, ChildSettle::Archived).await;
    assert_eq!(count_completion_cards(&pool, parent_id).await, 1);
    assert_eq!(drain_callbacks(&mut callback_rx), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Deleting a stopped child is the user ending it too. The parent is told,
/// in words that say what the user did.
#[tokio::test]
async fn deleting_a_stopped_child_sends_the_parent_a_canceled_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_response_canceled_with_cause(&bus, child_id, CancelCause::UserStop).await;

    let card = bus
        .owed_child_card(child_id, ChildSettle::Deleted)
        .await
        .expect("a stopped child is owed a card");
    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(child_id)
        .execute(&pool)
        .await
        .unwrap();
    bus.deliver_owed_child_card(card).await;

    let (status, summary) = newest_completion_card(&pool, parent_id).await;
    assert_eq!(status, "canceled");
    assert!(summary.contains("deleted"), "got {summary:?}");
    assert_eq!(drain_callbacks(&mut callback_rx), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
