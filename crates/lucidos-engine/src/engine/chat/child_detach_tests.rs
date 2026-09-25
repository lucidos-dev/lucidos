//! Moving a child thread to top level (ADR 0278): the ladder, the projection,
//! and what the former parent no longer gets.

use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::{ActorMode, EventChannel};
use crate::engine::LucidosEngine;
use crate::test_support::{setup_test_db, teardown_test_db};
use sqlx::PgPool;

async fn emit_message(bus: &EventBus, thread_id: Uuid, parent: Option<Uuid>, text: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: text.into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: parent,
            spawning_event_id: None,
            mode: ActorMode::Human,
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
}

async fn spawn(bus: &EventBus, parent: Option<Uuid>, text: &str) -> Uuid {
    let id = Uuid::new_v4();
    emit_message(bus, id, parent, text).await;
    id
}

/// Emit the move exactly as `detach_child_thread` does. `None` means the bus
/// dropped it.
async fn detach(bus: &EventBus, parent: Uuid, child: Uuid) -> Option<Uuid> {
    bus.emit(BusEvent::Thread {
        thread_id: parent,
        event: ThreadEvent::ChildThreadDetached {
            child_thread_id: child,
            child_thread_title: Some("child".into()),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap()
    .map(|r| r.event_id)
}

async fn finish(bus: &EventBus, thread: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::ResponseGenerated {
            text: "done".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct Row {
    parent_thread_id: Option<Uuid>,
    depth: i32,
    status: String,
    archive_state: String,
    parent_callback_pending: bool,
    is_stopped_child: bool,
    active_children_count: i32,
    total_children_count: i32,
    blocking_descendant_count: i32,
    attention_descendant_count: i32,
}

async fn row(pool: &PgPool, thread: Uuid) -> Row {
    sqlx::query_as(
        "SELECT parent_thread_id, depth, status, archive_state, parent_callback_pending, \
                is_stopped_child, active_children_count, total_children_count, \
                blocking_descendant_count, attention_descendant_count \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn count_events(pool: &PgPool, thread: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate = 'thread' AND aggregate_id = $1 AND event_type = $2",
    )
    .bind(thread.to_string())
    .bind(event_type)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn authorize(
    pool: &PgPool,
    caller: DetachCaller,
    target: Uuid,
) -> Result<(Uuid, String), ChildDetachError> {
    LucidosEngine::authorize_child_detach(pool, caller, target).await
}

// --- the ladder ------------------------------------------------------------

/// An agent moves only its own direct children. The user moves any thread that
/// has a parent, however deep.
#[tokio::test]
async fn the_ladder_lets_an_agent_move_only_its_direct_children() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let root = spawn(&bus, None, "root").await;
    let parent = spawn(&bus, Some(root), "parent").await;
    let child = spawn(&bus, Some(parent), "child task").await;
    let sibling = spawn(&bus, Some(parent), "sibling").await;
    let agent = |id| DetachCaller::Agent(Some(id));

    assert_eq!(
        authorize(&pool, agent(parent), child).await,
        Ok((parent, "child task".into()))
    );
    for (caller, target, why) in [
        (sibling, child, "a sibling"),
        (root, child, "a grandparent"),
        (child, parent, "a child aiming up"),
        (parent, root, "a thread aiming at a top-level thread"),
    ] {
        assert_eq!(
            authorize(&pool, agent(caller), target).await,
            Err(ChildDetachError::NotYourChild(target)),
            "{why} is refused"
        );
    }
    assert_eq!(
        authorize(&pool, agent(child), child).await,
        Err(ChildDetachError::SelfTarget(child))
    );
    assert_eq!(
        authorize(&pool, DetachCaller::Agent(None), child).await,
        Err(ChildDetachError::NoCaller)
    );

    assert_eq!(
        authorize(&pool, DetachCaller::User, child).await,
        Ok((parent, "child task".into())),
        "the user reaches a grandchild of a top-level thread"
    );
    assert_eq!(
        authorize(&pool, DetachCaller::User, root).await,
        Err(ChildDetachError::NotAChild(root))
    );
    let missing = Uuid::new_v4();
    assert_eq!(
        authorize(&pool, DetachCaller::User, missing).await,
        Err(ChildDetachError::UnknownThread(missing))
    );

    sqlx::query("UPDATE thread_summaries SET state = 'discarded' WHERE thread_id = $1")
        .bind(sibling)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        authorize(&pool, DetachCaller::User, sibling).await,
        Err(ChildDetachError::Discarded(sibling))
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn every_refusal_maps_to_its_status() {
    let id = Uuid::new_v4();
    for (err, status) in [
        (ChildDetachError::UnknownThread(id), 404),
        (ChildDetachError::NotAChild(id), 409),
        (ChildDetachError::Discarded(id), 409),
        (ChildDetachError::NotYourChild(id), 403),
        (ChildDetachError::NoCaller, 403),
        (ChildDetachError::SelfTarget(id), 400),
        (ChildDetachError::Internal("x".into()), 500),
    ] {
        assert_eq!(err.status_code(), status, "{err}");
    }
}

// --- the projection ----------------------------------------------------------

/// The move on a RUNNING child, so the counters are non-zero and the order of
/// the reconcile is observable. The subtree moves as a unit and the former
/// parent and its ancestor stop counting it.
#[tokio::test]
async fn moving_a_running_child_cuts_the_edge_and_reconciles_every_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let root = spawn(&bus, None, "root").await;
    let parent = spawn(&bus, Some(root), "parent").await;
    let child = spawn(&bus, Some(parent), "child").await;
    let grandchild = spawn(&bus, Some(child), "grandchild").await;

    let before = row(&pool, parent).await;
    assert_eq!(before.active_children_count, 1);
    assert_eq!(before.total_children_count, 1);
    assert_eq!(before.blocking_descendant_count, 2);
    assert_eq!(row(&pool, root).await.blocking_descendant_count, 3);
    assert_eq!(row(&pool, grandchild).await.depth, 3);

    assert!(detach(&bus, parent, child).await.is_some());

    let moved = row(&pool, child).await;
    assert_eq!(moved.parent_thread_id, None);
    assert_eq!(moved.depth, 0);
    assert!(!moved.parent_callback_pending);
    assert_eq!(moved.status, "running", "the move never stops the child");
    assert_eq!(
        moved.blocking_descendant_count, 1,
        "the child keeps its own subtree"
    );
    let grand = row(&pool, grandchild).await;
    assert_eq!(grand.parent_thread_id, Some(child));
    assert_eq!(grand.depth, 1, "every descendant shifts by the same delta");

    let parent_after = row(&pool, parent).await;
    assert_eq!(parent_after.active_children_count, 0);
    assert_eq!(parent_after.total_children_count, 0);
    assert_eq!(parent_after.blocking_descendant_count, 0);
    assert_eq!(row(&pool, root).await.blocking_descendant_count, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Applying the event a second time changes nothing, and the boot-time
/// reconciles agree with what the projection left.
#[tokio::test]
async fn the_move_is_replay_safe_and_survives_the_boot_reconciles() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = spawn(&bus, None, "parent").await;
    let child = spawn(&bus, Some(parent), "child").await;
    let sibling = spawn(&bus, Some(parent), "sibling").await;

    assert!(detach(&bus, parent, child).await.is_some());
    let parent_once = row(&pool, parent).await;
    let child_once = row(&pool, child).await;
    assert_eq!(parent_once.active_children_count, 1);
    assert_eq!(parent_once.total_children_count, 1);

    // Replay the event through the projection: it finds no edge to cut.
    let mut tx = pool.begin().await.unwrap();
    let (_, touched) = bus
        .update_thread_projection(
            &mut tx,
            parent,
            &ThreadEvent::ChildThreadDetached {
                child_thread_id: child,
                child_thread_title: None,
            },
            &EventMeta::NONE,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(touched.is_empty(), "a replay rebroadcasts nothing");
    assert_eq!(row(&pool, parent).await, parent_once);
    assert_eq!(row(&pool, child).await, child_once);
    // And the bus refuses to record a second move of the same child.
    assert_eq!(detach(&bus, parent, child).await, None);
    assert_eq!(count_events(&pool, parent, "ChildThreadDetached").await, 1);

    EventBus::rebuild_children_counts(&pool).await.unwrap();
    EventBus::rebuild_blocking_descendant_count(&pool)
        .await
        .unwrap();
    assert_eq!(row(&pool, parent).await, parent_once);
    assert_eq!(row(&pool, child).await, child_once);
    assert_eq!(row(&pool, sibling).await.parent_thread_id, Some(parent));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Two moves of one child at once, as from two devices. One lands, the other
/// is dropped, and neither deadlocks on the child's row.
#[tokio::test]
async fn two_moves_at_once_record_one() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = spawn(&bus, None, "parent").await;
    let child = spawn(&bus, Some(parent), "child").await;
    let _grandchild = spawn(&bus, Some(child), "grandchild").await;

    let (a, b) = tokio::join!(detach(&bus, parent, child), detach(&bus, parent, child));
    assert_eq!(
        [a, b].iter().filter(|r| r.is_some()).count(),
        1,
        "exactly one move is recorded"
    );
    assert_eq!(count_events(&pool, parent, "ChildThreadDetached").await, 1);
    assert_eq!(row(&pool, child).await.parent_thread_id, None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A former parent left with no children is reset at boot, so drift on it can
/// never outlive a restart.
#[tokio::test]
async fn the_boot_reconcile_resets_a_childless_former_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = spawn(&bus, None, "parent").await;
    let child = spawn(&bus, Some(parent), "child").await;
    assert!(detach(&bus, parent, child).await.is_some());

    sqlx::query(
        "UPDATE thread_summaries SET active_children_count = 3, total_children_count = 3, \
                blocking_descendant_count = 3, attention_descendant_count = 3 \
         WHERE thread_id = $1",
    )
    .bind(parent)
    .execute(&pool)
    .await
    .unwrap();
    EventBus::rebuild_children_counts(&pool).await.unwrap();
    EventBus::rebuild_blocking_descendant_count(&pool)
        .await
        .unwrap();

    let reset = row(&pool, parent).await;
    assert_eq!(
        (
            reset.active_children_count,
            reset.total_children_count,
            reset.blocking_descendant_count,
            reset.attention_descendant_count
        ),
        (0, 0, 0, 0)
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A *stopped child* owes its parent a card, and lights the parent's attention
/// count. Once moved it owes nothing, and the parent's count comes down.
#[tokio::test]
async fn moving_a_stopped_child_settles_what_it_owed() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = spawn(&bus, None, "parent").await;
    let child = spawn(&bus, Some(parent), "child").await;
    sqlx::query(
        "UPDATE thread_summaries SET status = 'idle', is_stopped_child = TRUE, \
                parent_callback_pending = TRUE WHERE thread_id = $1",
    )
    .bind(child)
    .execute(&pool)
    .await
    .unwrap();
    EventBus::rebuild_blocking_descendant_count(&pool)
        .await
        .unwrap();
    assert_eq!(row(&pool, parent).await.attention_descendant_count, 1);

    assert!(detach(&bus, parent, child).await.is_some());

    let moved = row(&pool, child).await;
    assert!(!moved.is_stopped_child);
    assert!(!moved.parent_callback_pending);
    assert_eq!(row(&pool, parent).await.attention_descendant_count, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// --- what the former parent no longer gets -----------------------------------

/// Move a child mid-turn, then let the turn finish. No card reaches the
/// former parent, while a sibling still nested reports as before.
#[tokio::test]
async fn a_moved_child_finishing_its_turn_sends_no_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = spawn(&bus, None, "parent").await;
    let child = spawn(&bus, Some(parent), "child").await;
    let control = spawn(&bus, Some(parent), "control").await;

    assert!(detach(&bus, parent, child).await.is_some());
    finish(&bus, child).await;
    finish(&bus, control).await;

    assert_eq!(
        count_events(&pool, parent, "ChildThreadCompleted").await,
        1,
        "only the child still nested reports to the parent"
    );
    let moved = row(&pool, child).await;
    assert_eq!(moved.status, "idle", "the turn finished normally");
    assert_ne!(moved.archive_state, "archived");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The fan-in read the edge before the move committed, then emits its card
/// afterwards. The bus drops that card.
#[tokio::test]
async fn a_card_arriving_after_the_move_is_dropped() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = spawn(&bus, None, "parent").await;
    let child = spawn(&bus, Some(parent), "child").await;
    assert!(detach(&bus, parent, child).await.is_some());

    for event in [
        ThreadEvent::ChildThreadCompleted {
            child_thread_id: child,
            child_thread_title: None,
            status: crate::engine::thread_events::ChildCompletionStatus::Success,
            summary: "late".into(),
            pending_change_ids: vec![],
            sub_thread_pending_changes: vec![],
        },
        ThreadEvent::ChildThreadStopped {
            child_thread_id: child,
            child_thread_title: None,
        },
    ] {
        let emitted = bus
            .emit(BusEvent::Thread {
                thread_id: parent,
                event,
                meta: EventMeta::NONE,
            })
            .await
            .unwrap();
        assert!(
            emitted.is_none(),
            "the late event is dropped, not persisted"
        );
    }
    assert_eq!(count_events(&pool, parent, "ChildThreadCompleted").await, 0);
    assert_eq!(count_events(&pool, parent, "ChildThreadStopped").await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Moving child B out must not hide child A's unprocessed card from the boot
/// sweep, or A's result is lost to a restart (ADR 0011).
#[tokio::test]
async fn moving_one_child_does_not_strand_a_siblings_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut rx) = EventBus::new(pool.clone());
    let parent = spawn(&bus, None, "parent").await;
    let child_a = spawn(&bus, Some(parent), "a").await;
    let child_b = spawn(&bus, Some(parent), "b").await;

    finish(&bus, child_a).await;
    assert_eq!(count_events(&pool, parent, "ChildThreadCompleted").await, 1);
    assert!(detach(&bus, parent, child_b).await.is_some());
    while rx.try_recv().is_ok() {}

    assert_eq!(bus.refire_unprocessed_child_completions().await, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The former parent can no longer follow up on the moved child.
#[tokio::test]
async fn the_former_parent_loses_its_follow_up_authority() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = spawn(&bus, None, "parent").await;
    let child = spawn(&bus, Some(parent), "child").await;
    assert!(detach(&bus, parent, child).await.is_some());

    let err = LucidosEngine::authorize_child_follow_up(
        &pool,
        Some(parent),
        child,
        None,
        crate::engine::FollowUpUrgency::Normal,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err,
        crate::engine::chat::child_follow_up::ChildFollowUpError::NotYourChild(child)
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A move never buys back a child slot, or it becomes a way round the cap.
#[tokio::test]
async fn a_moved_child_still_counts_against_the_fan_out_cap() {
    use super::super::recursion_guard::MAX_CHILDREN_PER_THREAD;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = spawn(&bus, None, "parent").await;
    let mut children = Vec::new();
    for i in 0..MAX_CHILDREN_PER_THREAD {
        children.push(spawn(&bus, Some(parent), &format!("child {i}")).await);
    }
    assert!(LucidosEngine::check_thread_recursion_guard(&pool, parent)
        .await
        .is_err());

    assert!(detach(&bus, parent, children[0]).await.is_some());
    assert!(
        LucidosEngine::check_thread_recursion_guard(&pool, parent)
            .await
            .is_err(),
        "the slot the moved child used stays used"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
