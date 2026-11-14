//! A child's *wake decision* on its row: I5 of
//! `docs/plans/2026-10-11-sub-threads-wake-parents-only-when-needed.md`.

use super::*;

async fn parent_wake_requested(pool: &PgPool, thread_id: Uuid) -> Option<bool> {
    sqlx::query_scalar("SELECT parent_wake_requested FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn each_decision_projects_onto_the_child_and_a_new_turn_starts_undecided() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    assert_eq!(parent_wake_requested(&pool, child_id).await, None);

    emit(&bus, child_id, ThreadEvent::ParentWakeNotRequested).await;
    assert_eq!(parent_wake_requested(&pool, child_id).await, Some(false));
    emit(&bus, child_id, ThreadEvent::ParentWakeRequested).await;
    assert_eq!(parent_wake_requested(&pool, child_id).await, Some(true));

    emit(&bus, child_id, ThreadEvent::ParentWakeNotRequested).await;
    emit_thread_message(&bus, child_id, None, "one more thing").await;
    assert_eq!(
        parent_wake_requested(&pool, child_id).await,
        None,
        "a follow-up turn must not inherit the previous turn's no"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn wake_mode(pool: &PgPool, thread_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT wake_mode FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// I8 of `docs/plans/2026-10-12-the-spawn-declares-the-wake-mode.md`: the
/// spawning message writes the mode with the parent link, and a follow-up,
/// which carries none, leaves it alone.
#[tokio::test]
async fn the_spawning_message_stores_the_wake_mode_for_the_childs_life() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) =
        spawn_parent_child_with_mode(&bus, EventChannel::Chat, Some(WakeMode::NoWake)).await;
    assert_eq!(wake_mode(&pool, child_id).await.as_deref(), Some("no-wake"));
    assert_eq!(
        wake_mode(&pool, parent_id).await,
        None,
        "a top thread has none"
    );

    emit_thread_message(&bus, child_id, None, "one more thing").await;
    assert_eq!(wake_mode(&pool, child_id).await.as_deref(), Some("no-wake"));

    let (_, undeclared) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    assert_eq!(wake_mode(&pool, undeclared).await, None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
