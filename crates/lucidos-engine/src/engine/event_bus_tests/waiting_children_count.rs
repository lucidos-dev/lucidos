//! `thread_summaries.waiting_children_count`: the direct children that are not
//! in flight and hold a live event wait.
//!
//! Such a child has not finished (ADR 0254), so its parent is waiting on it.
//! The count is what lets the parent's status dot and waiting indicator say so.
//! It is kept apart from `active_children_count`, which means "running".
//! See `docs/plans/2026-09-24-a-parent-waits-on-a-child-that-waits.md`.

use super::super::*;
use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::engine::thread_events::EventWaitCancelCause;

async fn waiting_children(pool: &PgPool, parent_id: Uuid) -> i32 {
    sqlx::query_scalar("SELECT waiting_children_count FROM thread_summaries WHERE thread_id = $1")
        .bind(parent_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn emit_wait_started(bus: &EventBus, thread_id: Uuid) -> Uuid {
    let wait_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitStarted {
            wait_id,
            tool_use_id: format!("toolu_{wait_id}"),
            on: vec![EventSubscription {
                event_type: "BenchSlotReleased".into(),
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
    wait_id
}

async fn emit_wait_canceled(bus: &EventBus, thread_id: Uuid, wait_id: Uuid) {
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
}

/// A coding-agent child that ran, armed a wait, and idled: the reported shape.
async fn spawn_parent_with_waiting_child(bus: &EventBus) -> (Uuid, Uuid, Uuid) {
    let (parent_id, child_id) = spawn_parent_child(bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(bus, child_id).await;
    let wait_id = emit_wait_started(bus, child_id).await;
    emit_cc_idle(bus, child_id, false, None).await;
    (parent_id, child_id, wait_id)
}

#[tokio::test]
async fn a_child_idling_on_a_wait_counts_as_waiting_on_its_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, _child_id, _wait_id) = spawn_parent_with_waiting_child(&bus).await;

    assert_eq!(waiting_children(&pool, parent_id).await, 1);
    assert_active_children(
        &pool,
        parent_id,
        0,
        "a waiting child is not running, so the active count leaves it out (ADR 0254)",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A running child that arms a wait is counted once, as active. The two
/// counts stay disjoint, so the waiting indicator can add them.
#[tokio::test]
async fn a_running_child_holding_a_wait_counts_as_active_only() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_wait_started(&bus, child_id).await;

    assert_eq!(waiting_children(&pool, parent_id).await, 0);
    assert_active_children(&pool, parent_id, 1, "still mid-turn").await;

    emit_cc_idle(&bus, child_id, false, None).await;
    assert_eq!(
        waiting_children(&pool, parent_id).await,
        1,
        "the idle moves the child from active to waiting in the same event"
    );
    assert_active_children(&pool, parent_id, 0, "the idle ended the turn").await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_wake_moves_the_child_back_to_active() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id, wait_id) = spawn_parent_with_waiting_child(&bus).await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::EventWaitExpired { wait_id },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(
        waiting_children(&pool, parent_id).await,
        0,
        "the wait is gone, so the child no longer waits"
    );

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::UserPromptInjected {
            text: "An event you subscribed to has arrived".into(),
            mode: ActorMode::Engine,
            origin: None,
            injected_message_id: None,
            delivered_event_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(waiting_children(&pool, parent_id).await, 0);
    assert_active_children(&pool, parent_id, 1, "the woken turn is running").await;

    emit_cc_idle(&bus, child_id, false, None).await;
    assert_eq!(waiting_children(&pool, parent_id).await, 0);
    assert_active_children(&pool, parent_id, 0, "the child finished for real").await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn stopping_the_last_wait_ends_the_waiting() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id, first) = spawn_parent_with_waiting_child(&bus).await;
    let second = emit_wait_started(&bus, child_id).await;

    emit_wait_canceled(&bus, child_id, first).await;
    assert_eq!(
        waiting_children(&pool, parent_id).await,
        1,
        "one wait is still live, so the child still waits"
    );

    emit_wait_canceled(&bus, child_id, second).await;
    assert_eq!(waiting_children(&pool, parent_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A follow-up revives the child while its wait is still live. It is running
/// again, so it moves from waiting to active.
#[tokio::test]
async fn a_follow_up_to_a_waiting_child_moves_it_to_active() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id, _wait_id) = spawn_parent_with_waiting_child(&bus).await;

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentUserMessageSent {
            text: "one more thing".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    assert_eq!(waiting_children(&pool, parent_id).await, 0);
    assert_active_children(&pool, parent_id, 1, "the follow-up revived it").await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn each_waiting_child_counts_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, first_child, _wait_id) = spawn_parent_with_waiting_child(&bus).await;
    emit_wait_started(&bus, first_child).await;

    let second_child = Uuid::new_v4();
    emit_cc_message_received(&bus, second_child, Some(parent_id), "second").await;
    emit_cc_session_started(&bus, second_child).await;
    emit_wait_started(&bus, second_child).await;
    emit_cc_idle(&bus, second_child, false, None).await;

    assert_eq!(
        waiting_children(&pool, parent_id).await,
        2,
        "a count of children, not of waits"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The frontend reads the count off the parent's aggregate. Without a
/// rebroadcast the dot would appear only after a reload.
#[tokio::test]
async fn arming_a_wait_rebroadcasts_the_parent_aggregate() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, false, None).await;

    let mut rx = bus.subscribe();
    emit_wait_started(&bus, child_id).await;

    let parent_aggregate = drain_aggregate_broadcasts(&mut rx)
        .into_iter()
        .filter(|(id, _, _)| *id == parent_id)
        .find_map(|(_, _, aggregate)| aggregate)
        .expect("the parent's aggregate is rebroadcast when its waiting count moves");
    assert_eq!(parent_aggregate.waiting_children_count, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_top_thread_holding_a_wait_touches_no_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    emit_cc_message_received(&bus, thread_id, None, "top").await;
    emit_wait_started(&bus, thread_id).await;

    assert_eq!(
        waiting_children(&pool, thread_id).await,
        0,
        "the thread's own wait is `live_event_wait_count`, not a waiting child"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_rebuild_repairs_a_drifted_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, _child_id, _wait_id) = spawn_parent_with_waiting_child(&bus).await;
    let (lone_parent, lone_child) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, lone_child).await;
    emit_cc_idle(&bus, lone_child, false, None).await;

    sqlx::query("UPDATE thread_summaries SET waiting_children_count = 0 WHERE thread_id = $1")
        .bind(parent_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE thread_summaries SET waiting_children_count = 3 WHERE thread_id = $1")
        .bind(lone_parent)
        .execute(&pool)
        .await
        .unwrap();

    EventBus::rebuild_children_counts(&pool).await.unwrap();

    assert_eq!(
        waiting_children(&pool, parent_id).await,
        1,
        "under-count repaired"
    );
    assert_eq!(
        waiting_children(&pool, lone_parent).await,
        0,
        "over-count repaired"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
