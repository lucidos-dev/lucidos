use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::core::store::EventWaitSummary;
use crate::engine::thread_events::{BackgroundWaitTiming, EventWaitCancelCause};

const WAIT_REASON: &str = "waiting for the release change";

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
            background_timing: None,
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
async fn timing_updates_preserve_the_wait_and_cannot_revive_a_canceled_wait() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (thread_id, wait_id) = idle_thread_with_wait(&bus, &pool).await;
    let (_, original) = live_waits(&pool, thread_id).await;
    let updated = || BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitTimingUpdated {
            wait_id,
            reason: "a build slot".into(),
            expires_at: Utc::now() + chrono::Duration::hours(24),
            background_timing: BackgroundWaitTiming::Queued,
        },
        meta: EventMeta::NONE,
    };
    bus.emit(updated()).await.unwrap();
    let (count, waits) = live_waits(&pool, thread_id).await;
    assert_eq!(count, 1);
    assert_eq!(waits[0].wait_id, wait_id);
    assert_eq!(waits[0].on[0].event_type, original[0].on[0].event_type);
    assert_eq!(
        waits[0].background_timing,
        Some(BackgroundWaitTiming::Queued)
    );

    let cancel = bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitCanceled {
            wait_id,
            cause: EventWaitCancelCause::UserStop,
            on: vec![],
            reason: String::new(),
        },
        meta: EventMeta::NONE,
    });
    let (cancel, update) = tokio::join!(cancel, bus.emit(updated()));
    cancel.unwrap();
    update.unwrap();
    bus.emit(updated()).await.unwrap();
    assert_eq!(live_waits(&pool, thread_id).await, (0, vec![]));
    assert_eq!(status_of(&pool, thread_id).await, "idle");

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

#[tokio::test]
async fn an_arm_racing_a_resolution_does_not_resurrect_the_resolved_wait() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, expiring) = idle_thread_with_wait(&bus, &pool).await;
    let surviving = Uuid::new_v4();
    emit_wait_started(&bus, thread_id, surviving).await;
    assert_eq!(live_waits(&pool, thread_id).await.0, 2);

    let mut sweeper = pool.begin().await.unwrap();
    bus.update_thread_projection(
        &mut sweeper,
        thread_id,
        &ThreadEvent::EventWaitExpired { wait_id: expiring },
        &EventMeta::NONE,
    )
    .await
    .unwrap();

    let armed = Uuid::new_v4();
    let arm_pool = pool.clone();
    let arm = tokio::spawn(async move {
        let (arm_bus, _arm_rx) = EventBus::new(arm_pool.clone());
        emit_wait_started(&arm_bus, thread_id, armed).await;
    });
    wait_until_blocked_on_locks(&pool, 1).await;

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

#[tokio::test]
async fn a_non_uuid_domain_row_cannot_break_the_boot_rebuild() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, wait_id) = idle_thread_with_wait(&bus, &pool).await;

    sqlx::query(
        "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id) \
         VALUES ($1, 'EventWaitStarted', $2, 'domain', 'EventWaitStarted')",
    )
    .bind(Uuid::new_v4())
    .bind(serde_json::json!({ "summary": "an app made this up" }))
    .execute(&pool)
    .await
    .unwrap();

    let cache = crate::engine::event_wait::LiveWaits::new();
    let loaded = crate::engine::event_wait::rebuild_live_waits(&pool, &cache)
        .await
        .expect("a non-uuid domain aggregate_id must not fail the rebuild");

    assert_eq!(loaded, 1, "the real wait is still loaded");
    let rebuilt: Vec<Uuid> = cache
        .for_thread(thread_id)
        .await
        .iter()
        .map(|w| w.wait_id)
        .collect();
    assert_eq!(rebuilt, vec![wait_id]);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
