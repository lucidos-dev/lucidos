use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};
use sqlx::PgPool;

async fn emit_message(bus: &EventBus, thread_id: Uuid, parent: Option<Uuid>, text: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: text.into(),
            user_image_hashes: vec![],
            device_id: None,
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
    crate::engine::LucidosEngine::authorize_child_follow_up(
        pool,
        caller,
        child,
        None,
        crate::engine::FollowUpUrgency::Normal,
    )
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
        crate::engine::FollowUpUrgency::Normal,
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

#[tokio::test]
async fn delivery_is_sampled_from_the_child_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;

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
        crate::engine::FollowUpUrgency::Normal,
    )
    .await
    .unwrap();
    assert!(!row.uses_coding_agent(), "a chat child routes to chat");

    let cc_child = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: cc_child,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "coding task".into(),
            user_image_hashes: vec![],
            device_id: None,
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
        crate::engine::FollowUpUrgency::Normal,
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

#[tokio::test]
async fn the_home_thread_follows_up_a_coding_agent_thread_it_never_spawned() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let home = crate::engine::home_thread::ensure_home_thread(&bus, &pool)
        .await
        .unwrap();
    let (parent, chat_child) = parent_and_child(&bus).await;
    let cc_thread = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: cc_thread,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "coding task".into(),
            user_image_hashes: vec![],
            device_id: None,
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

    let (row, ack) = crate::engine::LucidosEngine::authorize_child_follow_up(
        &pool,
        Some(home),
        cc_thread,
        None,
        crate::engine::FollowUpUrgency::Normal,
    )
    .await
    .unwrap();
    assert!(row.uses_coding_agent());
    assert_eq!(ack.reach, FollowUpReach::Home);
    assert_eq!(
        authorize(&pool, Some(home), parent).await.unwrap().reach,
        FollowUpReach::Home
    );
    assert_eq!(
        authorize(&pool, Some(parent), chat_child)
            .await
            .unwrap()
            .reach,
        FollowUpReach::OwnChild
    );
    assert!(matches!(
        authorize(&pool, Some(chat_child), cc_thread).await,
        Err(ChildFollowUpError::NotYourChild(_))
    ));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_home_thread_cannot_follow_up_an_unsent_draft() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let home = crate::engine::home_thread::ensure_home_thread(&bus, &pool)
        .await
        .unwrap();
    let draft = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: draft,
        event: ThreadEvent::ThreadStarted {
            recovered_compose: None,
            agent_draft: None,
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let refusal = authorize(&pool, Some(home), draft).await.unwrap_err();
    assert!(matches!(refusal, ChildFollowUpError::ChildIsDraft(id) if id == draft));
    assert_eq!(refusal.status_code(), 409);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

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

use crate::engine::chat::child_follow_up::{build_follow_up_message, parent_thread_link};
use crate::engine::thread_events::{MessageOrigin, ThreadDirection};

fn workspace() -> std::path::PathBuf {
    std::path::PathBuf::from("/tmp/child-follow-up-tests")
}

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

#[tokio::test]
async fn child_follow_up_does_not_change_total_children_count() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;

    let (_, total_before) = counters(&pool, parent).await;
    deliver_follow_up(&bus, parent, child, "while you work").await;
    assert_eq!(counters(&pool, parent).await.1, total_before);

    idle_the_child(&bus, child).await;
    deliver_follow_up(&bus, parent, child, "and again").await;
    assert_eq!(counters(&pool, parent).await.1, total_before);

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

#[tokio::test]
async fn follow_up_racing_the_child_terminal_leaves_counters_correct() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;

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
    assert!(
        !FollowUpDelivery::Interrupted.wants_pre_emit(),
        "the redirect lane must sequence the interrupted turn's Canceled terminal \
         BEFORE the follow-up's MessageReceived, so pre-emitting here would invert \
         the child's timeline"
    );
}

#[test]
fn urgency_upgrades_only_a_running_child() {
    use crate::engine::thread_lifecycle::ThreadStatus;

    assert_eq!(
        FollowUpDelivery::from_status(ThreadStatus::Running, FollowUpUrgency::Normal),
        FollowUpDelivery::Running
    );
    assert_eq!(
        FollowUpDelivery::from_status(ThreadStatus::Running, FollowUpUrgency::Urgent),
        FollowUpDelivery::Interrupted
    );
    for status in [
        ThreadStatus::WaitingForUserAnswer,
        ThreadStatus::Idle,
        ThreadStatus::Failed,
    ] {
        assert_eq!(
            FollowUpDelivery::from_status(status, FollowUpUrgency::Urgent),
            FollowUpDelivery::from_status(status, FollowUpUrgency::Normal),
            "urgency must not change the reading for {status:?}: there is no turn to stop"
        );
    }
}

#[test]
fn interrupted_and_running_read_differently() {
    let queued = FollowUpDelivery::Running.describe();
    let stopped = FollowUpDelivery::Interrupted.describe();

    assert_ne!(queued, stopped);
    assert!(
        queued.contains("queues"),
        "the default has to say the message waits:\n{queued}"
    );
    assert!(
        stopped.contains("stopped"),
        "an urgent follow-up has to say the child's current turn is ending:\n{stopped}"
    );
}

#[test]
fn only_an_interrupted_delivery_hands_the_turn_urgency() {
    assert_eq!(
        FollowUpDelivery::Interrupted.effective_urgency(),
        FollowUpUrgency::Urgent
    );
    for delivery in [
        FollowUpDelivery::Running,
        FollowUpDelivery::WaitingForUserAnswer,
        FollowUpDelivery::Revived,
    ] {
        assert_eq!(
            delivery.effective_urgency(),
            FollowUpUrgency::Normal,
            "{delivery:?} must not preempt: the ack does not promise it, and for \
             WaitingForUserAnswer it promises the opposite"
        );
    }
}

#[test]
fn the_ack_never_promises_a_wait_it_then_preempts() {
    for delivery in [
        FollowUpDelivery::Running,
        FollowUpDelivery::Interrupted,
        FollowUpDelivery::WaitingForUserAnswer,
        FollowUpDelivery::Revived,
    ] {
        let says_it_waits = delivery.describe().contains("will not read");
        assert!(
            !(says_it_waits && delivery.effective_urgency().is_urgent()),
            "{delivery:?} tells the caller the child waits, then preempts it anyway: {}",
            delivery.describe()
        );
    }
}

#[test]
fn missing_urgent_flag_is_normal() {
    assert_eq!(FollowUpUrgency::from_flag(None), FollowUpUrgency::Normal);
    assert_eq!(
        FollowUpUrgency::from_flag(Some(false)),
        FollowUpUrgency::Normal
    );
    assert_eq!(
        FollowUpUrgency::from_flag(Some(true)),
        FollowUpUrgency::Urgent
    );
    assert_eq!(FollowUpUrgency::default(), FollowUpUrgency::Normal);
}

#[test]
fn a_malformed_urgent_tool_arg_is_refused_not_defaulted() {
    use serde_json::json;

    assert_eq!(
        FollowUpUrgency::from_tool_arg(None),
        Ok(FollowUpUrgency::Normal)
    );
    assert_eq!(
        FollowUpUrgency::from_tool_arg(Some(&json!(null))),
        Ok(FollowUpUrgency::Normal)
    );
    assert_eq!(
        FollowUpUrgency::from_tool_arg(Some(&json!(false))),
        Ok(FollowUpUrgency::Normal)
    );
    assert_eq!(
        FollowUpUrgency::from_tool_arg(Some(&json!(true))),
        Ok(FollowUpUrgency::Urgent)
    );

    for bad in [json!("true"), json!("yes"), json!(1), json!(["true"])] {
        let err = FollowUpUrgency::from_tool_arg(Some(&bad))
            .expect_err(&format!("{bad} must be refused, not defaulted"));
        assert!(
            err.contains("must be a boolean"),
            "the refusal must say what shape is expected: {err}"
        );
    }
}

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

#[tokio::test]
async fn snapshot_settlement_does_not_create_a_parent_completion() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;
    sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) SELECT $1, 'snapshot', now(), MAX(sequence) FROM events")
        .bind(Uuid::new_v4()).execute(&pool).await.unwrap();
    while rx.try_recv().is_ok() {}
    crate::engine::thread_events::emit_response_aborted(
        &bus,
        child,
        crate::engine::thread_events::AbortCause::SnapshotRestored,
        "Stopped by restore".into(),
        vec![],
        None,
        None,
        EventMeta::NONE,
        "[test] restore",
    )
    .await;
    assert_eq!(cards_on(&pool, parent).await, 0);
    assert_eq!(bus.refire_unprocessed_child_completions().await, 0);
    idle_the_child(&bus, child).await;
    assert_eq!(
        cards_on(&pool, parent).await,
        0,
        "recovery idle must not report archived work"
    );
    deliver_follow_up(&bus, parent, child, "new work").await;
    sqlx::query("ALTER TABLE snapshot_restore_boundary RENAME TO hidden_restore_boundary")
        .execute(&pool)
        .await
        .unwrap();
    idle_the_child(&bus, child).await;
    assert_eq!(
        cards_on(&pool, parent).await,
        0,
        "unknown generation refuses callback"
    );
    sqlx::query("ALTER TABLE hidden_restore_boundary RENAME TO snapshot_restore_boundary")
        .execute(&pool)
        .await
        .unwrap();
    idle_the_child(&bus, child).await;
    assert_eq!(cards_on(&pool, parent).await, 1);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn restored_child_completion_never_refires_but_fresh_completion_does() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut rx) = EventBus::new(pool.clone());
    let (_, archived_child) = parent_and_child(&bus).await;
    idle_the_child(&bus, archived_child).await;
    while rx.try_recv().is_ok() {}
    sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) SELECT $1, 'snapshot', now(), MAX(sequence) FROM events")
        .bind(Uuid::new_v4()).execute(&pool).await.unwrap();
    for _ in 0..2 {
        assert_eq!(bus.refire_unprocessed_child_completions().await, 0);
        assert!(rx.try_recv().is_err());
    }
    let (_, fresh_child) = parent_and_child(&bus).await;
    idle_the_child(&bus, fresh_child).await;
    while rx.try_recv().is_ok() {}
    assert_eq!(bus.refire_unprocessed_child_completions().await, 1);
    assert!(rx.try_recv().is_ok());
    sqlx::query("ALTER TABLE snapshot_restore_boundary RENAME TO hidden_restore_boundary")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(bus.refire_unprocessed_child_completions().await, 0);
    sqlx::query("ALTER TABLE hidden_restore_boundary RENAME TO snapshot_restore_boundary")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn boot_sweep_skips_a_parent_that_already_followed_up() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut rx) = EventBus::new(pool.clone());
    let (parent, child) = parent_and_child(&bus).await;

    idle_the_child(&bus, child).await;
    assert_eq!(cards_on(&pool, parent).await, 1);

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
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "task B".into(),
            user_image_hashes: vec![],
            device_id: None,
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
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "test-session".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
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
                turn_end: None,
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
                    provider: None,
                    voice_session_id: None,
                    wake_mode: None,
                    text: "child task".into(),
                    user_image_hashes: vec![],
                    device_id: None,
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
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "coding task".into(),
            user_image_hashes: vec![],
            device_id: None,
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
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "test-session".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
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
    assert_eq!(
        cards_on(&pool, parent).await,
        0,
        "the parent is simply never told the child finished"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_question_parked_coding_agent_child_acks_held() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, _) = parent_and_child(&bus).await;
    let cc_child = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: cc_child,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "coding task".into(),
            user_image_hashes: vec![],
            device_id: None,
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
    crate::engine::agent_question::aq_test_helpers::emit_user_question(
        &bus,
        cc_child,
        "toolu-open#q0",
    )
    .await;

    let ack = authorize(&pool, Some(parent), cc_child).await.unwrap();
    assert_eq!(ack.delivered_to, FollowUpDelivery::Held);
    assert!(ack.delivered_to.describe().contains("held"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}
