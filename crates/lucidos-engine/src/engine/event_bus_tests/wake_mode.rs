//! A child's *wake mode* at the fan-in: I1, I2, I3 and I6 of
//! `docs/plans/2026-10-12-the-spawn-declares-the-wake-mode.md`.

use super::super::parent_callback::Delivery;
use super::*;
use crate::engine::thread_events::{AbortCause, ChildCompletionStatus};

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

/// Spawn one more chat child of `parent_id` under `wake_mode`.
async fn spawn_child(bus: &EventBus, parent_id: Uuid, wake_mode: WakeMode) -> Uuid {
    let child_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: Some(wake_mode),
            text: "child task".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Agent,
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
    child_id
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

async fn parent_row(pool: &PgPool, thread_id: Uuid) -> (String, String) {
    sqlx::query_as("SELECT status, archive_state FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// I2 and I3 as a table: a declared mode decides only a clean finish, and a
/// recorded yes wakes under every mode.
#[test]
fn a_wake_mode_decides_only_a_clean_finish() {
    use ChildCompletionStatus::*;
    for status in [Success, Failure, NoChanges, Canceled, Interrupted] {
        let clean = matches!(status, Success | NoChanges);
        for mode in WakeMode::ALL {
            for batch_still_working in [false, true] {
                for recorded in [None, Some(false), Some(true)] {
                    let files = clean
                        && recorded != Some(true)
                        && match mode {
                            WakeMode::NoWake => true,
                            WakeMode::Wake => false,
                            WakeMode::WakeWhenAllDone => batch_still_working,
                        };
                    let expected = if files {
                        Delivery::File
                    } else {
                        Delivery::Wake
                    };
                    assert_eq!(
                        Delivery::for_report(status, Some(mode), recorded, batch_still_working),
                        expected,
                        "{status:?} {mode:?} batch={batch_still_working} recorded={recorded:?}"
                    );
                }
            }
        }
    }
}

/// I1: a no-wake child's clean report is filed, starts no parent turn, and
/// leaves the parent's row alone, with no decision recorded at all.
#[tokio::test]
async fn a_no_wake_report_is_filed_without_a_decision() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) =
        spawn_parent_child_with_mode(&bus, EventChannel::Chat, Some(WakeMode::NoWake)).await;
    emit(&bus, parent_id, reply("Started a thread for it.")).await;
    let before = parent_row(&pool, parent_id).await;

    emit(&bus, child_id, reply("Here is the research.")).await;

    assert_eq!(count_on(&pool, parent_id, "ChildReportFiled").await, 1);
    assert_eq!(count_on(&pool, parent_id, "ChildThreadCompleted").await, 0);
    assert_eq!(drain_callbacks(&mut callback_rx), 0, "no parent turn");
    assert_eq!(parent_row(&pool, parent_id).await, before);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I2: a no-wake child that fails still wakes its parent.
#[tokio::test]
async fn a_failed_no_wake_child_still_wakes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) =
        spawn_parent_child_with_mode(&bus, EventChannel::Chat, Some(WakeMode::NoWake)).await;

    emit(
        &bus,
        child_id,
        ThreadEvent::ResponseFailed {
            error: "boom".into(),
            agent_exit: None,
        },
    )
    .await;

    assert_eq!(count_on(&pool, parent_id, "ChildThreadCompleted").await, 1);
    assert_eq!(drain_callbacks(&mut callback_rx), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I3: the engine's own yes (a cut-short turn) wakes even a no-wake parent.
#[tokio::test]
async fn an_engine_recorded_wake_beats_no_wake() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) =
        spawn_parent_child_with_mode(&bus, EventChannel::Chat, Some(WakeMode::NoWake)).await;

    emit(&bus, child_id, ThreadEvent::ParentWakeRequested).await;
    emit(&bus, child_id, reply("Stopped at the iteration cap.")).await;

    assert_eq!(count_on(&pool, parent_id, "ChildThreadCompleted").await, 1);
    assert_eq!(drain_callbacks(&mut callback_rx), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I6: the batch files each report while a sibling works, and the last wakes.
#[tokio::test]
async fn the_last_child_of_a_batch_wakes_its_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, first) =
        spawn_parent_child_with_mode(&bus, EventChannel::Chat, Some(WakeMode::WakeWhenAllDone))
            .await;
    let second = spawn_child(&bus, parent_id, WakeMode::WakeWhenAllDone).await;

    emit(&bus, first, reply("First half.")).await;
    assert_eq!(count_on(&pool, parent_id, "ChildReportFiled").await, 1);
    assert_eq!(
        drain_callbacks(&mut callback_rx),
        0,
        "a sibling still works"
    );

    emit(&bus, second, reply("Second half.")).await;
    assert_eq!(count_on(&pool, parent_id, "ChildThreadCompleted").await, 1);
    assert_eq!(drain_callbacks(&mut callback_rx), 1, "the last one wakes");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I6: two batch terminals that both commit before either fan-in runs both
/// wake. The second child's committed terminal is modelled by its idle row.
#[tokio::test]
async fn two_batch_children_finishing_together_both_wake() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, first) =
        spawn_parent_child_with_mode(&bus, EventChannel::Chat, Some(WakeMode::WakeWhenAllDone))
            .await;
    let second = spawn_child(&bus, parent_id, WakeMode::WakeWhenAllDone).await;
    sqlx::query("UPDATE thread_summaries SET status = 'idle' WHERE thread_id = $1")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();

    emit(&bus, first, reply("First half.")).await;
    emit(&bus, second, reply("Second half.")).await;

    assert_eq!(count_on(&pool, parent_id, "ChildThreadCompleted").await, 2);
    assert_eq!(count_on(&pool, parent_id, "ChildReportFiled").await, 0);
    assert_eq!(drain_callbacks(&mut callback_rx), 2, "never both filed");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I6: a sibling outside the batch does not hold it, and a batch sibling
/// holding a live event wait does.
#[tokio::test]
async fn only_working_batch_siblings_hold_the_batch() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, first) =
        spawn_parent_child_with_mode(&bus, EventChannel::Chat, Some(WakeMode::WakeWhenAllDone))
            .await;
    spawn_child(&bus, parent_id, WakeMode::Wake).await;
    spawn_child(&bus, parent_id, WakeMode::NoWake).await;

    emit(&bus, first, reply("Done.")).await;
    assert_eq!(
        drain_callbacks(&mut callback_rx),
        1,
        "no batch sibling works"
    );

    let waiting = spawn_child(&bus, parent_id, WakeMode::WakeWhenAllDone).await;
    sqlx::query(
        "UPDATE thread_summaries SET status = 'idle', live_event_wait_count = 1 \
          WHERE thread_id = $1",
    )
    .bind(waiting)
    .execute(&pool)
    .await
    .unwrap();
    let third = spawn_child(&bus, parent_id, WakeMode::WakeWhenAllDone).await;
    emit(&bus, third, reply("Done too.")).await;
    assert_eq!(count_on(&pool, parent_id, "ChildReportFiled").await, 1);
    assert_eq!(
        drain_callbacks(&mut callback_rx),
        0,
        "a waiting sibling holds it"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A batch sibling that handed off holds the batch until its successor's row
/// exists, since the Thread Queue may hold the spawn. Then the successor
/// counts as the sibling, so its idle releases the batch.
#[tokio::test]
async fn a_handed_off_sibling_holds_the_batch_until_its_successor_lands() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, first) =
        spawn_parent_child_with_mode(&bus, EventChannel::Chat, Some(WakeMode::WakeWhenAllDone))
            .await;
    let handed_off = spawn_child(&bus, parent_id, WakeMode::WakeWhenAllDone).await;
    let successor = Uuid::new_v4();
    sqlx::query(
        "UPDATE thread_summaries SET status = 'idle', successor_thread_id = $2 \
          WHERE thread_id = $1",
    )
    .bind(handed_off)
    .bind(successor)
    .execute(&pool)
    .await
    .unwrap();

    emit(&bus, first, reply("Done.")).await;
    assert_eq!(count_on(&pool, parent_id, "ChildReportFiled").await, 1);
    assert_eq!(
        drain_callbacks(&mut callback_rx),
        0,
        "the queued successor holds the batch"
    );

    let landed = spawn_child(&bus, parent_id, WakeMode::WakeWhenAllDone).await;
    sqlx::query("UPDATE thread_summaries SET successor_thread_id = $2 WHERE thread_id = $1")
        .bind(handed_off)
        .bind(landed)
        .execute(&pool)
        .await
        .unwrap();
    emit(&bus, landed, reply("Carried on and done.")).await;
    assert_eq!(drain_callbacks(&mut callback_rx), 1, "the successor wakes");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I6: the last batch child can end in an abort that sends no card of its own.
/// It then sends a failure card, so the filed reports reach a woken parent.
#[tokio::test]
async fn a_card_less_abort_ending_a_batch_still_wakes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, first) =
        spawn_parent_child_with_mode(&bus, EventChannel::Chat, Some(WakeMode::WakeWhenAllDone))
            .await;
    let second = spawn_child(&bus, parent_id, WakeMode::WakeWhenAllDone).await;
    emit(&bus, first, reply("First half.")).await;
    assert_eq!(drain_callbacks(&mut callback_rx), 0);

    emit(
        &bus,
        second,
        ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: AbortCause::SafetyNet,
        },
    )
    .await;

    let (status, summary) = newest_completion_card(&pool, parent_id).await;
    assert_eq!(status, "failure");
    assert!(summary.contains("wake-when-all-done"), "{summary}");
    assert_eq!(drain_callbacks(&mut callback_rx), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
