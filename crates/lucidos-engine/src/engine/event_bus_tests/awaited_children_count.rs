//! `awaited_by_parent` and `awaited_children_count`: the children a parent
//! waits on (ADR 0672).
//!
//! A child is awaited when its finish wakes the parent and it is unfinished:
//! active, holding a live event wait, or awaiting a child of its own. The last
//! clause climbs any depth. A `no-wake` child is never awaited.
//! See `docs/plans/2026-11-13-a-parent-waits-on-what-will-wake-it.md`.

use super::super::*;
use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::engine::thread_events::EventWaitCancelCause;

async fn awaited_children(pool: &PgPool, thread_id: Uuid) -> i32 {
    sqlx::query_scalar("SELECT awaited_children_count FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn status(pool: &PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn awaited_by_parent(pool: &PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT awaited_by_parent FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
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

async fn emit_follow_up(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
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
}

async fn detach(bus: &EventBus, parent_id: Uuid, child_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::ChildThreadDetached {
            child_thread_id: child_id,
            child_thread_title: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// Start a running coding-agent child of `parent_id` under `wake_mode`.
async fn spawn_running_child(bus: &EventBus, parent_id: Uuid, wake_mode: Option<WakeMode>) -> Uuid {
    let child_id = Uuid::new_v4();
    spawn_child_of(
        bus,
        parent_id,
        child_id,
        EventChannel::ClaudeCode,
        wake_mode,
    )
    .await;
    emit_cc_session_started(bus, child_id).await;
    child_id
}

/// The reported shape: an idle top thread, an idle `wake` child that already
/// reported, and a running `wake` grandchild.
async fn spawn_reported_family(bus: &EventBus) -> (Uuid, Uuid, Uuid) {
    let (top, child) =
        spawn_parent_child_with_mode(bus, EventChannel::ClaudeCode, Some(WakeMode::Wake)).await;
    emit_cc_session_started(bus, child).await;
    let grandchild = spawn_running_child(bus, child, Some(WakeMode::Wake)).await;
    emit_cc_idle(bus, child, false, None).await;
    (top, child, grandchild)
}

#[tokio::test]
async fn a_running_wake_child_is_awaited() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) =
        spawn_parent_child_with_mode(&bus, EventChannel::ClaudeCode, Some(WakeMode::Wake)).await;

    assert!(awaited_by_parent(&pool, child_id).await);
    assert_eq!(awaited_children(&pool, parent_id).await, 1);

    emit_cc_session_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, false, None).await;
    assert!(
        !awaited_by_parent(&pool, child_id).await,
        "the child finished"
    );
    assert_eq!(awaited_children(&pool, parent_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A child from before wake modes decides per turn, so it may wake.
#[tokio::test]
async fn a_child_with_no_stored_wake_mode_is_awaited() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, _child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;

    assert_eq!(awaited_children(&pool, parent_id).await, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Nothing wakes the parent of a `no-wake` child, so it does not wait on it.
/// The child still holds a child slot (ADR 0380).
#[tokio::test]
async fn a_running_no_wake_child_is_not_awaited() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) =
        spawn_parent_child_with_mode(&bus, EventChannel::ClaudeCode, Some(WakeMode::NoWake)).await;
    emit_cc_session_started(&bus, child_id).await;

    assert!(!awaited_by_parent(&pool, child_id).await);
    assert_eq!(awaited_children(&pool, parent_id).await, 0);
    assert_active_children(&pool, parent_id, 1, "it is still running").await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The user's report. The grandchild's finish starts a new turn on the child,
/// which reports to the top thread again. So the top thread waits.
#[tokio::test]
async fn a_top_thread_waits_on_a_running_grandchild_behind_an_idle_child() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (top, child, grandchild) = spawn_reported_family(&bus).await;

    assert!(awaited_by_parent(&pool, grandchild).await);
    assert_eq!(awaited_children(&pool, child).await, 1);
    assert!(
        awaited_by_parent(&pool, child).await,
        "the child awaits a child of its own"
    );
    assert_eq!(awaited_children(&pool, top).await, 1);
    assert_active_children(&pool, top, 0, "the active count stays direct").await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_grandchild_finishing_wakes_the_child_and_its_turn_end_clears_the_top() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (top, child, grandchild) = spawn_reported_family(&bus).await;

    emit_cc_idle(&bus, grandchild, false, None).await;
    assert_eq!(awaited_children(&pool, child).await, 0);
    assert_eq!(
        status(&pool, child).await,
        "running",
        "the report woke the child"
    );
    assert_eq!(
        awaited_children(&pool, top).await,
        1,
        "so the top thread still waits"
    );

    emit_cc_idle(&bus, child, false, None).await;
    assert!(!awaited_by_parent(&pool, child).await);
    assert_eq!(awaited_children(&pool, top).await, 0);

    emit_follow_up(&bus, grandchild).await;
    assert_eq!(awaited_children(&pool, child).await, 1);
    assert_eq!(
        awaited_children(&pool, top).await,
        1,
        "the revive climbs as well"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_roll_up_reaches_the_root_of_a_four_level_chain() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (root, first) =
        spawn_parent_child_with_mode(&bus, EventChannel::ClaudeCode, Some(WakeMode::Wake)).await;
    emit_cc_session_started(&bus, first).await;
    let second = spawn_running_child(&bus, first, Some(WakeMode::Wake)).await;
    let third = spawn_running_child(&bus, second, Some(WakeMode::Wake)).await;
    emit_cc_idle(&bus, second, false, None).await;
    emit_cc_idle(&bus, first, false, None).await;

    assert_eq!(awaited_children(&pool, root).await, 1);

    // Each finish wakes the level above, until the chain's last turn ends.
    // The test bus starts no woken turn, so its start event is emitted here.
    emit_cc_idle(&bus, third, false, None).await;
    for id in [second, first] {
        assert_eq!(awaited_children(&pool, root).await, 1);
        emit_follow_up(&bus, id).await;
        emit_cc_idle(&bus, id, false, None).await;
    }
    for id in [root, first, second] {
        assert_eq!(
            awaited_children(&pool, id).await,
            0,
            "nothing below is unfinished"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A `no-wake` grandchild wakes nobody, so it stops the climb above it.
#[tokio::test]
async fn a_no_wake_grandchild_leaves_both_ancestors_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (top, child) =
        spawn_parent_child_with_mode(&bus, EventChannel::ClaudeCode, Some(WakeMode::Wake)).await;
    emit_cc_session_started(&bus, child).await;
    spawn_running_child(&bus, child, Some(WakeMode::NoWake)).await;
    emit_cc_idle(&bus, child, false, None).await;

    assert_eq!(awaited_children(&pool, child).await, 0);
    assert_eq!(awaited_children(&pool, top).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A child idling on a wait has not finished (ADR 0254), at any depth.
#[tokio::test]
async fn a_wait_on_the_grandchild_keeps_both_ancestors_waiting_until_it_ends() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (top, child, grandchild) = spawn_reported_family(&bus).await;
    let first = emit_wait_started(&bus, grandchild).await;
    let second = emit_wait_started(&bus, grandchild).await;
    emit_cc_idle(&bus, grandchild, false, None).await;
    assert_eq!(
        awaited_children(&pool, top).await,
        1,
        "the idle grandchild still waits"
    );

    emit_wait_canceled(&bus, grandchild, first).await;
    assert_eq!(
        awaited_children(&pool, top).await,
        1,
        "one wait is still live"
    );

    emit_wait_canceled(&bus, grandchild, second).await;
    assert_eq!(awaited_children(&pool, child).await, 0);
    emit_cc_idle(&bus, child, false, None).await;
    assert_eq!(awaited_children(&pool, top).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn detaching_the_grandchild_clears_both_ancestors() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (top, child, grandchild) = spawn_reported_family(&bus).await;
    detach(&bus, child, grandchild).await;

    assert!(
        !awaited_by_parent(&pool, grandchild).await,
        "a top thread is nobody's child"
    );
    assert_eq!(awaited_children(&pool, child).await, 0);
    assert_eq!(awaited_children(&pool, top).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn detaching_the_child_clears_the_top_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (top, child, _grandchild) = spawn_reported_family(&bus).await;
    detach(&bus, top, child).await;

    assert!(!awaited_by_parent(&pool, child).await);
    assert_eq!(
        awaited_children(&pool, child).await,
        1,
        "the child still waits itself"
    );
    assert_eq!(awaited_children(&pool, top).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn each_awaited_child_counts_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, first) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, first).await;
    emit_wait_started(&bus, first).await;
    emit_wait_started(&bus, first).await;
    spawn_running_child(&bus, parent_id, None).await;

    assert_eq!(
        awaited_children(&pool, parent_id).await,
        2,
        "children, not waits"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The frontend reads the count off each ancestor's aggregate. Without a
/// rebroadcast the top thread's dot would move only on reload.
#[tokio::test]
async fn the_grandchild_finishing_rebroadcasts_both_ancestors() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (top, child, grandchild) = spawn_reported_family(&bus).await;

    let mut rx = bus.subscribe();
    emit_cc_idle(&bus, grandchild, false, None).await;

    let broadcasts = drain_aggregate_broadcasts(&mut rx);
    for ancestor in [top, child] {
        let aggregate = broadcasts
            .iter()
            .filter(|(id, _, _)| *id == ancestor)
            .find_map(|(_, _, aggregate)| aggregate.clone())
            .expect("every ancestor whose count moved is rebroadcast");
        assert_eq!(aggregate.awaited_children_count, 0);
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_top_thread_holding_a_wait_is_nobodys_awaited_child() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    emit_cc_message_received(&bus, thread_id, None, "top").await;
    emit_wait_started(&bus, thread_id).await;

    assert!(!awaited_by_parent(&pool, thread_id).await);
    assert_eq!(awaited_children(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The boot rebuild and the live climb share one predicate, so a rebuild from
/// zeroed or drifted columns lands where the live projection stood.
#[tokio::test]
async fn the_rebuild_reaches_the_live_values_from_drifted_columns() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (top, child, grandchild) = spawn_reported_family(&bus).await;
    let (lone_parent, lone_child) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, lone_child).await;
    emit_cc_idle(&bus, lone_child, false, None).await;

    let snapshot = "SELECT thread_id, awaited_by_parent, awaited_children_count \
                    FROM thread_summaries ORDER BY thread_id";
    let live: Vec<(Uuid, bool, i32)> = sqlx::query_as(snapshot).fetch_all(&pool).await.unwrap();

    sqlx::query(
        "UPDATE thread_summaries SET awaited_by_parent = FALSE, awaited_children_count = 0",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE thread_summaries SET awaited_by_parent = TRUE, awaited_children_count = 3 \
         WHERE thread_id = ANY($1)",
    )
    .bind([lone_parent, lone_child])
    .execute(&pool)
    .await
    .unwrap();

    EventBus::rebuild_children_counts(&pool).await.unwrap();

    let rebuilt: Vec<(Uuid, bool, i32)> = sqlx::query_as(snapshot).fetch_all(&pool).await.unwrap();
    assert_eq!(rebuilt, live);
    assert_eq!(awaited_children(&pool, top).await, 1);
    assert!(awaited_by_parent(&pool, child).await);
    assert!(awaited_by_parent(&pool, grandchild).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A legacy family stores a wrong `depth`: its backfill gave every child
/// depth 1, grandchildren included. The rebuild must still reach the live
/// values, so it cannot order its levels by `depth`.
#[tokio::test]
async fn the_rebuild_ignores_a_legacy_depth() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (top, child, grandchild) = spawn_reported_family(&bus).await;
    sqlx::query("UPDATE thread_summaries SET depth = 1 WHERE thread_id = ANY($1)")
        .bind([child, grandchild])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE thread_summaries SET awaited_by_parent = FALSE, awaited_children_count = 0",
    )
    .execute(&pool)
    .await
    .unwrap();

    EventBus::rebuild_children_counts(&pool).await.unwrap();

    assert!(awaited_by_parent(&pool, grandchild).await);
    assert!(awaited_by_parent(&pool, child).await);
    assert_eq!(awaited_children(&pool, top).await, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
