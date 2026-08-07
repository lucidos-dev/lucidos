//! `thread_summaries.live_event_wait_count`, the durable "this thread is
//! watching" fact behind the Waiting status dot.
//!
//! The frontend has its own `meta.liveEventWaits` list, but that is folded from
//! a thread's loaded events, so it is empty on a drawer row nobody opened and
//! empty again after a reload. This counter is what survives both, which is
//! the whole reason it exists: without it a thread asleep on an event wait
//! renders exactly like one that finished.
//!
//! Every test here also asserts `status` stays `idle`. A subscription does not
//! hold the turn (ADR 0049), and this column feeds no backend predicate: it
//! changes what the dot draws and nothing else.

use super::super::*;
use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::engine::thread_events::EventWaitCancelCause;

async fn live_waits(pool: &PgPool, thread_id: Uuid) -> i32 {
    sqlx::query_scalar("SELECT live_event_wait_count FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn status_of(pool: &PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn emit_wait_started(bus: &EventBus, thread_id: Uuid, wait_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitStarted {
            wait_id,
            tool_use_id: format!("toolu_{wait_id}"),
            on: vec![EventSubscription {
                event_type: "ChangeProposed".into(),
                condition: None,
            }],
            reason: "waiting for the release change".into(),
            armed_at: Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            watermark: 0,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// A thread that has settled (`ResponseGenerated` → `idle`) and then
/// registered a wait: the exact shape that used to read as finished.
async fn idle_thread_with_wait(bus: &EventBus, pool: &PgPool) -> (Uuid, Uuid) {
    let thread_id = Uuid::new_v4();
    emit_thread_message(bus, thread_id, None, "watch for the release").await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "subscribing now".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    let wait_id = Uuid::new_v4();
    emit_wait_started(bus, thread_id, wait_id).await;
    assert_eq!(status_of(pool, thread_id).await, "idle");
    (thread_id, wait_id)
}

#[tokio::test]
async fn registering_a_wait_counts_it_without_touching_the_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, _wait_id) = idle_thread_with_wait(&bus, &pool).await;

    assert_eq!(
        live_waits(&pool, thread_id).await,
        1,
        "an idle thread that registered a wait must count it, or the drawer \
         cannot tell it apart from a finished thread"
    );
    assert_eq!(
        status_of(&pool, thread_id).await,
        "idle",
        "a subscription does not hold the turn (ADR 0049)"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn delivery_clears_the_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, wait_id) = idle_thread_with_wait(&bus, &pool).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitDelivered {
            wait_id,
            event_id: Uuid::new_v4(),
            event_type: "ChangeProposed".into(),
            payload: serde_json::json!({ "summary": "the release change" }),
            matched_index: 0,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(live_waits(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn expiry_clears_the_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, wait_id) = idle_thread_with_wait(&bus, &pool).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitExpired { wait_id },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(live_waits(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn cancel_clears_the_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, wait_id) = idle_thread_with_wait(&bus, &pool).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitCanceled {
            wait_id,
            cause: EventWaitCancelCause::UserStop,
            on: vec![],
            reason: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(live_waits(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Up to five waits can be live at once, so the counter has to track them
/// individually. Resolving one must leave the dot on for the others.
#[tokio::test]
async fn resolving_one_of_two_waits_leaves_the_other_counted() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, first) = idle_thread_with_wait(&bus, &pool).await;
    let second = Uuid::new_v4();
    emit_wait_started(&bus, thread_id, second).await;
    assert_eq!(live_waits(&pool, thread_id).await, 2);

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitExpired { wait_id: first },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(
        live_waits(&pool, thread_id).await,
        1,
        "the thread is still watching for the second event"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The floor exists because a resolution is emittable with no matching start:
/// a wait registered before this column's backfill, or a hand-emitted row. An
/// unsigned counter that went negative would pin the Waiting dot on forever,
/// which is the exact failure the column was added to remove.
#[tokio::test]
async fn a_resolution_with_no_matching_start_floors_at_zero() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "hello").await;
    assert_eq!(live_waits(&pool, thread_id).await, 0);

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitExpired {
            wait_id: Uuid::new_v4(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(live_waits(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The count has to reach the client on BOTH paths, or the dot is right in one
/// place and wrong in the other: the per-event SSE aggregate keeps an open
/// client live, and the thread-list summary is what seeds every drawer row and
/// survives a reload.
#[tokio::test]
async fn the_count_reaches_both_the_aggregate_and_the_thread_summary() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, _wait_id) = idle_thread_with_wait(&bus, &pool).await;

    let aggregate = crate::core::store::fetch_thread_aggregate(&pool, thread_id)
        .await
        .unwrap()
        .expect("thread row exists");
    assert_eq!(aggregate.live_event_wait_count, 1);

    let store = crate::core::EventStore::new(pool.clone());
    let summary = store
        .get_recent_threads(50)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.thread_id == thread_id.to_string())
        .expect("the thread is in the drawer list");
    assert_eq!(summary.live_event_wait_count, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
