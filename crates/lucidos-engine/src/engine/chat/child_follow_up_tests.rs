//! Authorization-half tests for [`super`]. Delivery is exercised separately;
//! everything here reads one row and refuses (or does not).

use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};
use sqlx::PgPool;

async fn emit_message(bus: &EventBus, thread_id: Uuid, parent: Option<Uuid>, text: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
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

/// Parent with one child, both on the projection.
async fn parent_and_child(bus: &EventBus) -> (Uuid, Uuid) {
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();
    emit_message(bus, parent, None, "orchestrate").await;
    emit_message(bus, child, Some(parent), "child task").await;
    (parent, child)
}

async fn thread_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM thread_summaries")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn authorize(
    pool: &PgPool,
    caller: Option<Uuid>,
    child: Uuid,
) -> Result<FollowUpAck, ChildFollowUpError> {
    crate::engine::LucidosEngine::authorize_child_follow_up(pool, caller, child, None)
        .await
        .map(|(_, ack)| ack)
}

#[tokio::test]
async fn follow_up_to_own_child_is_authorized() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;

    let ack = authorize(&pool, Some(parent), child).await.unwrap();
    assert_eq!(ack.child_thread_id, child);
    assert_eq!(
        ack.child_title, "child task",
        "the ack names the child by its handle, not by uuid"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The follow-up path can never create a thread: a typo'd or raced uuid must
/// refuse rather than quietly spawn a top-level thread.
#[tokio::test]
async fn follow_up_to_unknown_thread_creates_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, _child) = parent_and_child(&bus).await;
    let before = thread_count(&pool).await;

    let missing = Uuid::new_v4();
    let err = authorize(&pool, Some(parent), missing).await.unwrap_err();
    assert_eq!(err, ChildFollowUpError::UnknownChild(missing));
    assert_eq!(err.status_code(), 404);
    assert_eq!(
        thread_count(&pool).await,
        before,
        "an unknown target must not create a thread"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn follow_up_to_a_non_child_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, _child) = parent_and_child(&bus).await;

    let stranger = Uuid::new_v4();
    emit_message(&bus, stranger, None, "unrelated top-level work").await;

    let err = authorize(&pool, Some(parent), stranger).await.unwrap_err();
    assert_eq!(err, ChildFollowUpError::NotYourChild(stranger));
    assert_eq!(err.status_code(), 403);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// No sibling edge, ever. The check is a single equality against the caller, so
/// a sibling would need a new predicate rather than a relaxed one.
#[tokio::test]
async fn follow_up_to_a_sibling_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child_a) = parent_and_child(&bus).await;
    let child_b = Uuid::new_v4();
    emit_message(&bus, child_b, Some(parent), "sibling task").await;

    let err = authorize(&pool, Some(child_a), child_b).await.unwrap_err();
    assert_eq!(err, ChildFollowUpError::NotYourChild(child_b));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Direct children only. A grandparent reaches a grandchild through the child,
/// which is what a star topology at each level means.
#[tokio::test]
async fn follow_up_to_a_grandchild_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;
    let grandchild = Uuid::new_v4();
    emit_message(&bus, grandchild, Some(child), "grandchild task").await;

    let err = authorize(&pool, Some(parent), grandchild)
        .await
        .unwrap_err();
    assert_eq!(err, ChildFollowUpError::NotYourChild(grandchild));

    // The child can reach it, which is the route the grandparent must take.
    assert!(authorize(&pool, Some(child), grandchild).await.is_ok());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn follow_up_to_self_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, _child) = parent_and_child(&bus).await;

    let err = authorize(&pool, Some(parent), parent).await.unwrap_err();
    assert_eq!(err, ChildFollowUpError::SelfTarget(parent));
    assert_eq!(err.status_code(), 400);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn follow_up_to_a_discarded_child_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;

    // Seeded directly: `ThreadDiscarded` only discards a thread still in
    // `composing`, and a spawned child is `active` from its first
    // `MessageReceived`. The ladder's branch is defense in depth against a
    // discarded row reached any other way, the same posture
    // `refire_unprocessed_child_completions` takes with its
    // `state IS DISTINCT FROM 'discarded'` filter.
    sqlx::query("UPDATE thread_summaries SET state = 'discarded' WHERE thread_id = $1")
        .bind(child)
        .execute(&pool)
        .await
        .unwrap();

    let err = authorize(&pool, Some(parent), child).await.unwrap_err();
    assert_eq!(err, ChildFollowUpError::ChildDiscarded(child));
    assert_eq!(err.status_code(), 409);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn follow_up_without_a_caller_thread_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_parent, child) = parent_and_child(&bus).await;

    let err = authorize(&pool, None, child).await.unwrap_err();
    assert_eq!(err, ChildFollowUpError::NoCaller);
    assert_eq!(err.status_code(), 403);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A cross-workspace caller has no children to follow up on, because
/// cross-workspace spawns require `relation = "top"` and therefore land with
/// `parent_thread_id = NULL` in the receiving workspace. Refused with a reason,
/// never silently reinterpreted as a same-workspace call.
#[tokio::test]
async fn follow_up_with_caller_workspace_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;

    let err = crate::engine::LucidosEngine::authorize_child_follow_up(
        &pool,
        Some(parent),
        child,
        Some("other-workspace"),
    )
    .await
    .unwrap_err();
    assert_eq!(err, ChildFollowUpError::CrossWorkspaceUnsupported);
    assert_eq!(err.status_code(), 400);
    assert!(
        err.to_string().contains("Cross-workspace"),
        "the refusal must name cross-workspace as the reason: {err}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// `delivered_to` is sampled from the child's own status BEFORE anything is
/// delivered, which is what makes it structurally impossible to derive from an
/// await later.
#[tokio::test]
async fn delivery_is_sampled_from_the_child_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;

    // Fresh child: MessageReceived leaves it running.
    let ack = authorize(&pool, Some(parent), child).await.unwrap();
    assert_eq!(ack.delivered_to, FollowUpDelivery::Running);

    for (status, expected) in [
        ("idle", FollowUpDelivery::Revived),
        ("waiting", FollowUpDelivery::Revived),
        ("failed", FollowUpDelivery::Revived),
        (
            "waiting_for_user_answer",
            FollowUpDelivery::WaitingForUserAnswer,
        ),
    ] {
        sqlx::query("UPDATE thread_summaries SET status = $2 WHERE thread_id = $1")
            .bind(child)
            .bind(status)
            .execute(&pool)
            .await
            .unwrap();
        let ack = authorize(&pool, Some(parent), child).await.unwrap();
        assert_eq!(
            ack.delivered_to, expected,
            "status {status} must sample as {expected:?}"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Coding-agent-ness is derived from the child's own row and can never be
/// stated by the caller. A mis-derived flag routes a coding-agent child down
/// the Lucidos Agent's loop, where its terminal matches neither
/// `should_callback` nor `should_decrement`, so the parent is never woken and
/// its counter never comes down.
#[tokio::test]
async fn coding_agent_routing_is_derived_from_the_child_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, chat_child) = parent_and_child(&bus).await;

    let (row, _) = crate::engine::LucidosEngine::authorize_child_follow_up(
        &pool,
        Some(parent),
        chat_child,
        None,
    )
    .await
    .unwrap();
    assert!(!row.uses_coding_agent(), "a chat child routes to chat");

    let cc_child = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: cc_child,
        event: ThreadEvent::MessageReceived {
            text: "coding task".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent),
            spawning_event_id: None,
            mode: ActorMode::Agent,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let (row, _) = crate::engine::LucidosEngine::authorize_child_follow_up(
        &pool,
        Some(parent),
        cc_child,
        None,
    )
    .await
    .unwrap();
    assert!(
        row.uses_coding_agent(),
        "a coding-agent child routes to the coding-agent branch"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The label falls back to the spawn prompt when the child has no title yet,
/// so the tool's success text can always name the child by something a human
/// recognises rather than by a uuid.
#[tokio::test]
async fn the_ack_falls_back_to_the_spawn_prompt_for_a_title() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;

    let ack = authorize(&pool, Some(parent), child).await.unwrap();
    assert_eq!(ack.child_title, "child task", "falls back to first_message");

    bus.emit(BusEvent::Thread {
        thread_id: child,
        event: ThreadEvent::ThreadTitleGenerated {
            title: "Audit the auth module".into(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    let ack = authorize(&pool, Some(parent), child).await.unwrap();
    assert_eq!(ack.child_title, "Audit the auth module");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// --- Delivery half ---
//
// The delivery half's own contribution is the pre-emit: which event goes on the
// child's timeline, when it is written inline, and what its projection does to
// the parent's counters. Everything after the pre-emit is
// `process_message_with_steps`, the same entry point `chat_submit` uses, so the
// fast paths, the permission supersede (`chat/process/run.rs`, none of whose
// three supersede calls is gated on mode) and the orphan chain are exercised by
// that machinery's own suites rather than duplicated here.

use crate::engine::chat::child_follow_up::{build_follow_up_message, parent_thread_link};
use crate::engine::thread_events::{MessageOrigin, ThreadDirection};

fn workspace() -> std::path::PathBuf {
    std::path::PathBuf::from("/tmp/child-follow-up-tests")
}

/// Emit the exact event a follow-up puts on the child's timeline.
async fn deliver_follow_up(bus: &EventBus, parent: Uuid, child: Uuid, text: &str) -> Uuid {
    let origin = parent_thread_link(parent, Some("the orchestrator".into()), None);
    let event = build_follow_up_message(&workspace(), text, None, &origin);
    bus.emit(BusEvent::Thread {
        thread_id: child,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap()
    .unwrap()
    .event_id
}

async fn counters(pool: &PgPool, parent: Uuid) -> (i32, i32) {
    sqlx::query_as(
        "SELECT active_children_count, total_children_count \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(parent)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn cards_on(pool: &PgPool, parent: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ChildThreadCompleted'",
    )
    .bind(parent.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn idle_the_child(bus: &EventBus, child: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id: child,
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

/// The two load-bearing arguments. A payload carrying `parent_thread_id` routes
/// the projection down the SPAWN branch, which adds +1 to the parent's
/// `total_children_count` for a child that already exists.
#[test]
fn the_follow_up_message_carries_no_spawn_linkage() {
    let origin = parent_thread_link(Uuid::new_v4(), Some("parent".into()), None);
    let event = build_follow_up_message(&workspace(), "go the other way", None, &origin);
    match event {
        ThreadEvent::MessageReceived {
            parent_thread_id,
            spawning_event_id,
            mode,
            ..
        } => {
            assert_eq!(
                parent_thread_id, None,
                "a follow-up is not a spawn: a payload parent_thread_id would \
                 grow the parent's total_children_count"
            );
            assert_eq!(spawning_event_id, None);
            assert_eq!(mode, ActorMode::Agent);
        }
        other => panic!("expected a MessageReceived, got {other:?}"),
    }
}

/// The child's timeline attributes the follow-up to the parent thread, by
/// title, with `direction: Parent`. Without this it renders as "You".
#[tokio::test]
async fn child_follow_up_stamps_a_parent_thread_link() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;
    idle_the_child(&bus, child).await;

    let event_id = deliver_follow_up(&bus, parent, child, "go the other way").await;

    let payload: serde_json::Value = sqlx::query_scalar("SELECT payload FROM events WHERE id = $1")
        .bind(event_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let origin: MessageOrigin =
        serde_json::from_value(payload.get("origin").expect("origin present").clone()).unwrap();
    match origin {
        MessageOrigin::ThreadLink {
            thread_id,
            title,
            mode,
            direction,
            ..
        } => {
            assert_eq!(thread_id, parent, "the link points at the parent");
            assert_eq!(title.as_deref(), Some("the orchestrator"));
            assert_eq!(mode, ActorMode::Agent);
            assert_eq!(
                direction,
                ThreadDirection::Parent,
                "Parent means the linked thread spawned this one"
            );
        }
        other => panic!("expected a ThreadLink origin, got {other:?}"),
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Ordering. The pre-emit is awaited, so by the time the caller could observe
/// the ack the child's `MessageReceived` is persisted AND its projection has
/// committed the parent's re-increment. Otherwise the parent's own
/// `ResponseGenerated` wins the race and the parent flips to review with a
/// revived child still working.
#[tokio::test]
async fn follow_up_persists_the_child_message_before_returning() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;
    idle_the_child(&bus, child).await;
    assert_eq!(counters(&pool, parent).await, (0, 1), "child is done");

    let event_id = deliver_follow_up(&bus, parent, child, "one more thing").await;

    let persisted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events WHERE id = $1)")
        .bind(event_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(persisted, "the child's message is on disk before the ack");
    assert_eq!(
        counters(&pool, parent).await,
        (1, 1),
        "and the parent already counts the revived child"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A child follow-up never changes `total_children_count`, for any prior state
/// of the child. The failure this guards is a drawer badge promising
/// sub-threads that do not exist.
#[tokio::test]
async fn child_follow_up_does_not_change_total_children_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;

    // Running.
    let (_, total_before) = counters(&pool, parent).await;
    deliver_follow_up(&bus, parent, child, "while you work").await;
    assert_eq!(counters(&pool, parent).await.1, total_before);

    // Terminated.
    idle_the_child(&bus, child).await;
    deliver_follow_up(&bus, parent, child, "and again").await;
    assert_eq!(counters(&pool, parent).await.1, total_before);

    // Question-parked.
    sqlx::query(
        "UPDATE thread_summaries SET status = 'waiting_for_user_answer' WHERE thread_id = $1",
    )
    .bind(child)
    .execute(&pool)
    .await
    .unwrap();
    deliver_follow_up(&bus, parent, child, "and once more").await;
    assert_eq!(
        counters(&pool, parent).await.1,
        total_before,
        "total_children_count never moves for a follow-up"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// `active_children_count` equals the true count of the parent's children in
/// `{running, waiting_for_user_answer}` at every step of the loop.
#[tokio::test]
async fn child_follow_up_active_count_matches_ground_truth() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;

    deliver_follow_up(&bus, parent, child, "while you work").await;
    assert_eq!(
        counters(&pool, parent).await.0,
        1,
        "follow-up to a running child: unchanged"
    );

    idle_the_child(&bus, child).await;
    assert_eq!(counters(&pool, parent).await.0, 0, "the child finished");

    deliver_follow_up(&bus, parent, child, "one more thing").await;
    assert_eq!(
        counters(&pool, parent).await.0,
        1,
        "follow-up to a terminated child: +1"
    );

    sqlx::query(
        "UPDATE thread_summaries SET status = 'waiting_for_user_answer' WHERE thread_id = $1",
    )
    .bind(child)
    .execute(&pool)
    .await
    .unwrap();
    deliver_follow_up(&bus, parent, child, "while you wait").await;
    assert_eq!(
        counters(&pool, parent).await.0,
        1,
        "follow-up to a question-parked child: unchanged, it never left the count"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A followed-up child that terminates again reports a second completion, so
/// the parent is woken for the work it actually asked for.
#[tokio::test]
async fn followed_up_child_reports_a_second_completion() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;

    idle_the_child(&bus, child).await;
    assert_eq!(cards_on(&pool, parent).await, 1, "first completion");

    deliver_follow_up(&bus, parent, child, "go the other way").await;
    idle_the_child(&bus, child).await;
    assert_eq!(
        cards_on(&pool, parent).await,
        2,
        "the redirected turn reports too"
    );

    let mut wakes = 0;
    while rx.try_recv().is_ok() {
        wakes += 1;
    }
    assert_eq!(wakes, 2, "and the parent is woken once per real completion");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Reentrancy: two follow-ups in one parent turn both land, in order. The two
/// take different code paths in production (the first starts a turn, the second
/// queues into the turn the first started), and neither is allowed to lose a
/// message or double-count.
#[tokio::test]
async fn two_follow_ups_to_the_same_child_in_one_turn_both_arrive_in_order() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;
    idle_the_child(&bus, child).await;

    deliver_follow_up(&bus, parent, child, "first redirect").await;
    deliver_follow_up(&bus, parent, child, "second redirect").await;

    let texts: Vec<String> = sqlx::query_scalar(
        "SELECT payload->>'text' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'MessageReceived' \
         ORDER BY sequence",
    )
    .bind(child.to_string())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        texts,
        vec!["child task", "first redirect", "second redirect"],
        "both follow-ups land, in the order they were issued"
    );
    assert_eq!(
        counters(&pool, parent).await,
        (1, 1),
        "the second follow-up finds the child already revived and adds nothing"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A follow-up racing the child's own terminal leaves the counters correct. The
/// parent gets a confusing pair of events (a completion for the pre-redirect
/// turn, then a second one later), which is why the tool description warns
/// about it, but nothing corrupts.
#[tokio::test]
async fn follow_up_racing_the_child_terminal_leaves_counters_correct() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;

    // The terminal lands first, then the follow-up that was already in flight.
    idle_the_child(&bus, child).await;
    deliver_follow_up(&bus, parent, child, "go the other way").await;
    assert_eq!(counters(&pool, parent).await, (1, 1));

    idle_the_child(&bus, child).await;
    assert_eq!(
        counters(&pool, parent).await,
        (0, 1),
        "both turns reconcile: the counter lands where ground truth says"
    );
    assert_eq!(cards_on(&pool, parent).await, 2, "one card per real turn");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The pre-emit rule, stated once and asserted here: inline exactly when the
/// child is outside the in-flight set. In flight means a live lane already owns
/// the emit, and pre-empting it would reorder the child's timeline.
#[test]
fn the_pre_emit_rule_is_the_in_flight_set() {
    assert!(
        FollowUpDelivery::Revived.wants_pre_emit(),
        "a revive is owed a re-increment, so its emit must be awaited"
    );
    assert!(
        !FollowUpDelivery::Running.wants_pre_emit(),
        "a live turn's lane owns the emit"
    );
    assert!(
        !FollowUpDelivery::WaitingForUserAnswer.wants_pre_emit(),
        "a parked child never left the parent's count, so nothing is owed"
    );
}

/// A follow-up to a question-parked child is reported as waiting, not as
/// delivered-and-working: the message does not answer the child's open question
/// (that route requires `mode == Human`), so it sits until a human answers.
#[tokio::test]
async fn follow_up_to_a_question_parked_child_reports_waiting() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;
    sqlx::query(
        "UPDATE thread_summaries SET status = 'waiting_for_user_answer' WHERE thread_id = $1",
    )
    .bind(child)
    .execute(&pool)
    .await
    .unwrap();

    let ack = authorize(&pool, Some(parent), child).await.unwrap();
    assert_eq!(ack.delivered_to, FollowUpDelivery::WaitingForUserAnswer);
    assert!(
        ack.delivered_to.describe().contains("human"),
        "the model must be told a human has to act first: {}",
        ack.delivered_to.describe()
    );

    deliver_follow_up(&bus, parent, child, "while you wait").await;
    let answered: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'UserQuestionAnswered'",
    )
    .bind(child.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        answered, 0,
        "an agent follow-up must never consume the child's open question"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// ADR 0011's idempotency argument survives the new edge: the boot sweep
/// selects parents whose LATEST event is a `ChildThreadCompleted`, and a parent
/// that reacted by issuing a follow-up has its own later events, so it is
/// skipped.
#[tokio::test]
async fn boot_sweep_skips_a_parent_that_already_followed_up() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;

    idle_the_child(&bus, child).await;
    assert_eq!(cards_on(&pool, parent).await, 1);

    // The parent reacts: it issues a follow-up and then ends its own turn.
    deliver_follow_up(&bus, parent, child, "go the other way").await;
    bus.emit(BusEvent::Thread {
        thread_id: parent,
        event: ThreadEvent::ResponseGenerated {
            text: "redirected child B".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    while rx.try_recv().is_ok() {}

    let refired = bus.refire_unprocessed_child_completions().await;
    assert_eq!(
        refired, 0,
        "the card is no longer the parent's latest event, so the sweep skips it"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The full loop, end to end at the projection layer:
/// `total_children_count` never moves off 2 across the whole sequence. That
/// single assertion is the counter-integrity regression the naive design (a
/// follow-up carrying `parent_thread_id`) fails.
#[tokio::test]
async fn parent_redirects_one_of_two_children_and_both_counters_land_at_zero() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent = Uuid::new_v4();
    let child_a = Uuid::new_v4();
    let child_b = Uuid::new_v4();
    emit_message(&bus, parent, None, "do two things").await;
    emit_message(&bus, child_a, Some(parent), "task A").await;
    emit_message(&bus, child_b, Some(parent), "task B").await;
    assert_eq!(counters(&pool, parent).await, (2, 2), "1. both spawned");

    idle_the_child(&bus, child_a).await;
    assert_eq!(counters(&pool, parent).await, (1, 2), "2. A finished");
    assert_eq!(cards_on(&pool, parent).await, 1);

    deliver_follow_up(&bus, parent, child_b, "go the other way").await;
    assert_eq!(
        counters(&pool, parent).await,
        (1, 2),
        "3. redirecting a RUNNING child changes no counter"
    );

    idle_the_child(&bus, child_b).await;
    assert_eq!(counters(&pool, parent).await, (0, 2), "4. B finished");
    assert_eq!(cards_on(&pool, parent).await, 2);

    deliver_follow_up(&bus, parent, child_b, "one more pass").await;
    assert_eq!(
        counters(&pool, parent).await,
        (1, 2),
        "5. redirecting a TERMINATED child revives it"
    );
    let pending: bool = sqlx::query_scalar(
        "SELECT parent_callback_pending FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(child_b)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        pending,
        "5. and the marker is armed at the moment the follow-up returns, not merely eventually"
    );

    idle_the_child(&bus, child_b).await;
    assert_eq!(counters(&pool, parent).await, (0, 2), "6. B finished again");
    assert_eq!(cards_on(&pool, parent).await, 3, "6. a third card");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The coding-agent twin of the full loop: hazard 12's case, where the card
/// count differed before Phase 1c.
#[tokio::test]
async fn parent_redirects_a_coding_agent_child_and_the_card_count_matches() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();
    emit_message(&bus, parent, None, "do a coding task").await;
    bus.emit(BusEvent::Thread {
        thread_id: child,
        event: ThreadEvent::MessageReceived {
            text: "task B".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent),
            spawning_event_id: None,
            mode: ActorMode::Agent,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: child,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "test-session".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    async fn idle(bus: &EventBus, child: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id: child,
            event: ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: None,
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
                bg_bash_pending: false,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }

    idle(&bus, child).await;
    assert_eq!(cards_on(&pool, parent).await, 1, "first turn reports");
    assert_eq!(counters(&pool, parent).await, (0, 1));

    deliver_follow_up(&bus, parent, child, "go the other way").await;
    assert_eq!(counters(&pool, parent).await, (1, 1), "revived");

    idle(&bus, child).await;
    assert_eq!(
        cards_on(&pool, parent).await,
        2,
        "the redirected coding-agent turn reports its own completion"
    );
    assert_eq!(counters(&pool, parent).await, (0, 1));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The `parent_callback_pending` invariant as the plan's 2x4 matrix:
/// {chat, coding-agent} x {running, idle, waiting, waiting_for_user_answer}.
/// After any delivered follow-up the marker must be TRUE, because that is what
/// carries the child's next terminal back to the parent. The failure it guards
/// is total: the parent redirects a child and is never woken again.
#[tokio::test]
async fn follow_up_marks_the_callback_pending() {
    for coding_agent in [false, true] {
        for status in ["running", "idle", "waiting", "waiting_for_user_answer"] {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());

            let parent = Uuid::new_v4();
            let child = Uuid::new_v4();
            emit_message(&bus, parent, None, "orchestrate").await;
            let channel = if coding_agent {
                EventChannel::ClaudeCode
            } else {
                EventChannel::Chat
            };
            bus.emit(BusEvent::Thread {
                thread_id: child,
                event: ThreadEvent::MessageReceived {
                    text: "child task".into(),
                    user_image_hashes: vec![],
                    device_id: None,
                    device: None,
                    image_description: None,
                    parent_thread_id: Some(parent),
                    spawning_event_id: None,
                    mode: ActorMode::Agent,
                    model: None,
                    reasoning_effort: None,
                    origin: None,
                },
                meta: EventMeta {
                    channel: Some(channel),
                    ..EventMeta::NONE
                },
            })
            .await
            .unwrap();

            // Put the child in the state under test, and clear the marker so
            // the assertion cannot pass on the spawn write alone.
            sqlx::query(
                "UPDATE thread_summaries \
                 SET status = $2, parent_callback_pending = FALSE WHERE thread_id = $1",
            )
            .bind(child)
            .bind(status)
            .execute(&pool)
            .await
            .unwrap();

            deliver_follow_up(&bus, parent, child, "go the other way").await;

            assert!(
                read_pending(&pool, child).await,
                "a delivered follow-up must leave the callback pending \
                 (coding_agent={coding_agent}, prior status={status}); without it \
                 the child's next terminal never reaches the parent"
            );

            pool.close().await;
            teardown_test_db(&db_name).await;
        }
    }
}

async fn read_pending(pool: &PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT parent_callback_pending FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Hazard 11, which is WHY coding-agent-ness is derived from the child's row
/// rather than stated by the caller. A coding-agent child routed down the
/// non-coding-agent branch ends its turn with `ResponseGenerated`, and for
/// `is_coding_agent = true` that matches neither `should_callback` nor
/// `should_decrement`. The parent is never told and its counter never comes
/// back down: silent in both dimensions.
///
/// This asserts the failure mode itself, so the derivation stays load-bearing
/// rather than looking like a tidiness choice.
#[tokio::test]
async fn a_coding_agent_child_ending_with_response_generated_reports_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut rx) = EventBus::new(pool.clone());

    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();
    emit_message(&bus, parent, None, "orchestrate").await;
    bus.emit(BusEvent::Thread {
        thread_id: child,
        event: ThreadEvent::MessageReceived {
            text: "coding task".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent),
            spawning_event_id: None,
            mode: ActorMode::Agent,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: child,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "test-session".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(
        counters(&pool, parent).await,
        (1, 1),
        "the child is in flight"
    );
    while rx.try_recv().is_ok() {}

    idle_the_child(&bus, child).await;

    assert_eq!(
        cards_on(&pool, parent).await,
        0,
        "no card: (true, ResponseGenerated) matches no should_callback arm"
    );
    assert!(rx.try_recv().is_err(), "and no wake");
    // The in-tx reconcile still runs, so the counter is not what leaks here;
    // the missing CARD is the whole failure, and it is silent.
    assert_eq!(
        cards_on(&pool, parent).await,
        0,
        "the parent is simply never told the child finished"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
