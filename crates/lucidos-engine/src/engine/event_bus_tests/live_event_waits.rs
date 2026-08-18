//! `thread_summaries.live_event_waits` and `live_event_wait_count`, the durable
//! "this thread is watching" pair.
//!
//! The count says HOW MANY and drives the Waiting status dot. The list says
//! WHICH, and is the whole content of the subscription indicator: the reason,
//! the subscription chips, the countdown, the Stop waiting button.
//!
//! The frontend also folds its own list from a thread's `EventWait*` events.
//! That fold is empty on a drawer row nobody opened, and empty again after a
//! reload. Nothing reconciled it against the server either, so one missed
//! `EventWaitDelivered` stranded a resolved wait on screen forever. Both
//! columns survive all three, which is why they exist.
//!
//! **Every test asserts the two agree.** One statement per projection arm
//! writes both, with the count derived from the array's length. A test checking
//! only one of them would not be checking the invariant.
//!
//! Every test also asserts `status` stays `idle`. A subscription does not hold
//! the turn (ADR 0049), and neither column feeds a backend predicate: they
//! change what the client draws and nothing else.

use super::super::*;
use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::core::store::EventWaitSummary;
use crate::engine::thread_events::EventWaitCancelCause;

/// The reason text every wait in this module is armed with. A test asserts the
/// list carries the model's own words rather than a placeholder.
const WAIT_REASON: &str = "waiting for the release change";

/// The count and the list, read together.
///
/// One helper rather than two, because reading either alone is what let them
/// drift in the first place. Returns `(count, waits)`.
async fn live_waits(pool: &PgPool, thread_id: Uuid) -> (i32, Vec<EventWaitSummary>) {
    let (count, waits): (i32, sqlx::types::Json<Vec<EventWaitSummary>>) = sqlx::query_as(
        "SELECT live_event_wait_count, live_event_waits FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        count as usize,
        waits.0.len(),
        "the count and the list are written by one statement and must never disagree"
    );
    (count, waits.0)
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
            reason: WAIT_REASON.into(),
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
async fn registering_a_wait_records_it_without_touching_the_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, wait_id) = idle_thread_with_wait(&bus, &pool).await;

    let (count, waits) = live_waits(&pool, thread_id).await;
    assert_eq!(
        count, 1,
        "an idle thread that registered a wait must count it, or the drawer \
         cannot tell it apart from a finished thread"
    );
    assert_eq!(waits[0].wait_id, wait_id);
    assert_eq!(
        waits[0].reason, WAIT_REASON,
        "the panel renders the model's own words, so the reason has to survive"
    );
    assert_eq!(waits[0].on.len(), 1);
    assert_eq!(waits[0].on[0].event_type, "ChangeProposed");
    assert_eq!(
        status_of(&pool, thread_id).await,
        "idle",
        "a subscription does not hold the turn (ADR 0049)"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn delivery_clears_the_wait() {
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

    assert_eq!(live_waits(&pool, thread_id).await, (0, vec![]));
    assert_eq!(status_of(&pool, thread_id).await, "idle");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn expiry_clears_the_wait() {
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

    assert_eq!(live_waits(&pool, thread_id).await, (0, vec![]));
    assert_eq!(status_of(&pool, thread_id).await, "idle");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn cancel_clears_the_wait() {
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

    assert_eq!(live_waits(&pool, thread_id).await, (0, vec![]));
    assert_eq!(status_of(&pool, thread_id).await, "idle");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Several waits can be live at once, so resolution is by `wait_id` and never
/// "drop the last one". Resolving one must leave the other fully intact, dot
/// and panel row alike.
#[tokio::test]
async fn resolving_one_of_two_waits_leaves_the_other() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, first) = idle_thread_with_wait(&bus, &pool).await;
    let second = Uuid::new_v4();
    emit_wait_started(&bus, thread_id, second).await;
    assert_eq!(live_waits(&pool, thread_id).await.0, 2);

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitExpired { wait_id: first },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (count, waits) = live_waits(&pool, thread_id).await;
    assert_eq!(
        count, 1,
        "the thread is still watching for the second event"
    );
    assert_eq!(
        waits[0].wait_id, second,
        "the SURVIVING wait must be the one that was not resolved"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A resolution is emittable with no matching start: a wait registered before
/// this column family's backfill, or a hand-emitted row. It must leave both
/// columns untouched rather than moving one of them.
///
/// This is the case that made deriving the count from the list worth it. The
/// old arithmetic decremented the count on its own here. So a thread watching
/// one event, handed a stray resolution for another, lost its Waiting dot while
/// the panel kept drawing the wait.
#[tokio::test]
async fn a_resolution_with_no_matching_start_changes_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, _wait_id) = idle_thread_with_wait(&bus, &pool).await;
    assert_eq!(live_waits(&pool, thread_id).await.0, 1);

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitExpired {
            wait_id: Uuid::new_v4(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (count, waits) = live_waits(&pool, thread_id).await;
    assert_eq!(count, 1, "the thread's own wait is still live");
    assert_eq!(waits.len(), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A thread with no waits at all, handed a resolution. The count must floor at
/// zero rather than going negative, which would pin the Waiting dot on forever.
#[tokio::test]
async fn a_resolution_on_a_thread_with_no_waits_floors_at_zero() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "hello").await;
    assert_eq!(live_waits(&pool, thread_id).await, (0, vec![]));

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitExpired {
            wait_id: Uuid::new_v4(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(live_waits(&pool, thread_id).await, (0, vec![]));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Re-emitting a start for the same `wait_id` must not double-count it. The arm
/// filters by `wait_id` before appending, so a replay converges rather than
/// stacking a second identical panel row.
#[tokio::test]
async fn a_replayed_start_replaces_rather_than_duplicating() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, wait_id) = idle_thread_with_wait(&bus, &pool).await;
    emit_wait_started(&bus, thread_id, wait_id).await;

    let (count, waits) = live_waits(&pool, thread_id).await;
    assert_eq!(count, 1);
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].wait_id, wait_id);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The projection and the dispatcher must name the same live waits, including
/// when a `wait_id` is armed again AFTER a resolution carrying it.
///
/// `LIVE_WAITS_SQL` is the authority, and it matches a resolution only at a
/// LATER sequence than the start. A derivation that drops that ordering reads
/// the re-armed wait as already resolved, so the dispatcher holds a wait the
/// panel cannot draw. The runtime arms are order-free (filter, then append), so
/// this pins the agreement itself rather than one query's phrasing.
#[tokio::test]
async fn a_wait_id_armed_again_after_its_resolution_is_live_in_both_views() {
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
    assert_eq!(live_waits(&pool, thread_id).await.0, 0);

    emit_wait_started(&bus, thread_id, wait_id).await;

    let (count, waits) = live_waits(&pool, thread_id).await;
    assert_eq!(count, 1, "the re-armed wait is live again");
    assert_eq!(waits[0].wait_id, wait_id);

    let cache = crate::engine::event_wait::LiveWaits::new();
    crate::engine::event_wait::rebuild_live_waits(&pool, &cache)
        .await
        .unwrap();
    let rebuilt: Vec<Uuid> = cache
        .for_thread(thread_id)
        .await
        .iter()
        .map(|w| w.wait_id)
        .collect();
    assert_eq!(
        rebuilt,
        vec![wait_id],
        "the dispatcher's rebuild and the projection must name the same waits"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An expiry and an arm land from genuinely independent tasks: the deadline
/// sweeper on one side, the agent's `await_event` on the other. So the two
/// transactions overlap on one row, and each rewrites the WHOLE array.
///
/// The arm must not write back a snapshot it read before the expiry committed.
/// That array still holds the expired wait, and resurrecting one is the exact
/// bug this column family exists to remove. What keeps it honest is that both
/// `SET` expressions read `live_event_waits` off the row: under READ COMMITTED
/// the blocked writer re-evaluates them against the row the first writer left.
/// Reading the array into a CTE and joining it back would pin it to the older
/// snapshot instead.
///
/// Deterministic rather than timing-based: the arm is held until Postgres
/// itself reports it waiting on a lock.
#[tokio::test]
async fn an_arm_racing_a_resolution_does_not_resurrect_the_resolved_wait() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, expiring) = idle_thread_with_wait(&bus, &pool).await;
    let surviving = Uuid::new_v4();
    emit_wait_started(&bus, thread_id, surviving).await;
    assert_eq!(live_waits(&pool, thread_id).await.0, 2);

    // The sweeper expires one wait and holds the row lock, uncommitted.
    let mut sweeper = pool.begin().await.unwrap();
    bus.update_thread_projection(
        &mut sweeper,
        thread_id,
        &ThreadEvent::EventWaitExpired { wait_id: expiring },
        &EventMeta::NONE,
    )
    .await
    .unwrap();

    // The agent arms a third wait on its own connection. It blocks on the lock.
    let armed = Uuid::new_v4();
    let arm_pool = pool.clone();
    let arm = tokio::spawn(async move {
        let (arm_bus, _arm_rx) = EventBus::new(arm_pool.clone());
        emit_wait_started(&arm_bus, thread_id, armed).await;
    });
    wait_until_blocked_on_a_lock(&pool).await;

    sweeper.commit().await.unwrap();
    arm.await.unwrap();

    let (count, waits) = live_waits(&pool, thread_id).await;
    let ids: Vec<Uuid> = waits.iter().map(|w| w.wait_id).collect();
    assert_eq!(count, 2);
    assert!(
        !ids.contains(&expiring),
        "the arm wrote back an array it read before the expiry committed, \
         resurrecting a resolved wait: {ids:?}"
    );
    assert!(ids.contains(&surviving), "an untouched wait was dropped");
    assert!(ids.contains(&armed), "the arm's own wait was lost");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Block until this database has a backend waiting on a lock. That is the
/// signal that the racing statement reached its `UPDATE`, rather than merely
/// having been spawned. Scoped to `current_database()`, so a concurrent test's
/// disposable database cannot satisfy it.
///
/// Panics rather than returning on timeout: a race that never materialised
/// means the test proved nothing, and passing quietly is how it would rot.
async fn wait_until_blocked_on_a_lock(pool: &PgPool) {
    for _ in 0..200 {
        let blocked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity \
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if blocked > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("no backend ever blocked on the row lock, so the race never happened");
}

/// Both columns have to reach the client on BOTH paths, or the panel is right
/// in one place and wrong in the other: the per-event SSE aggregate keeps an
/// open client live, and the thread-list summary is what seeds every drawer row
/// and survives a reload.
#[tokio::test]
async fn the_waits_reach_both_the_aggregate_and_the_thread_summary() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, wait_id) = idle_thread_with_wait(&bus, &pool).await;

    let aggregate = crate::core::store::fetch_thread_aggregate(&pool, thread_id)
        .await
        .unwrap()
        .expect("thread row exists");
    assert_eq!(aggregate.live_event_wait_count, 1);
    assert_eq!(aggregate.live_event_waits.len(), 1);
    assert_eq!(aggregate.live_event_waits[0].wait_id, wait_id);

    let store = crate::core::EventStore::new(pool.clone());
    let summary = store
        .get_recent_threads(50)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.thread_id == thread_id.to_string())
        .expect("the thread is in the drawer list");
    assert_eq!(summary.live_event_wait_count, 1);
    assert_eq!(summary.live_event_waits, aggregate.live_event_waits);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
