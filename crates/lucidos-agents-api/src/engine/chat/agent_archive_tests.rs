use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::{ActorMode, EventChannel};
use crate::engine::LucidosEngine;
use crate::test_support::{setup_test_db, teardown_test_db};
use sqlx::PgPool;

async fn spawn(bus: &EventBus, parent: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "work".into(),
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
    id
}

async fn check_eligibility(
    pool: &PgPool,
    caller: Uuid,
    target: Uuid,
) -> Result<(), AgentArchiveError> {
    LucidosEngine::check_agent_archive_eligibility(pool, target, caller == target).await
}

#[tokio::test]
async fn a_running_caller_may_request_its_own_archive() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let caller = spawn(&bus, None).await;

    check_eligibility(&pool, caller, caller).await.unwrap();
    let unknown = Uuid::new_v4();
    assert!(matches!(
        check_eligibility(&pool, caller, unknown).await,
        Err(AgentArchiveError::UnknownThread(id)) if id == unknown
    ));

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_waiting_caller_gets_the_archive_routes_refusal() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let caller = spawn(&bus, None).await;
    bus.emit(BusEvent::Thread {
        thread_id: caller,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: "tu-1".into(),
            cc_session_id: String::new(),
            question: "Which one?".into(),
            options: vec![],
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            permission_request: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let refusal = check_eligibility(&pool, caller, caller).await.unwrap_err();
    let AgentArchiveError::Refused((status, body)) = &refusal else {
        panic!("expected the route's refusal, got {refusal:?}");
    };
    assert_eq!(status.as_u16(), 409);
    assert_eq!(body["reason"], "parent_not_archivable");
    assert_eq!(body["parent_status"], "waiting_for_user_answer");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("waiting on the user")),
        "{:?}",
        body.0
    );
    assert_eq!(refusal.reason(), "parent_not_archivable");

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_discarded_thread_is_refused() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let caller = spawn(&bus, None).await;
    let child = spawn(&bus, Some(caller)).await;
    sqlx::query("UPDATE thread_summaries SET state = 'discarded' WHERE thread_id = $1")
        .bind(child)
        .execute(&pool)
        .await
        .unwrap();

    let refusal = check_eligibility(&pool, caller, child).await.unwrap_err();
    assert!(matches!(refusal, AgentArchiveError::Discarded(id) if id == child));
    assert_eq!(refusal.status_code(), 409);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_pinned_thread_is_refused_to_an_agent() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let caller = spawn(&bus, None).await;
    let child = spawn(&bus, Some(caller)).await;
    for pinned in [child, caller] {
        bus.emit(BusEvent::Thread {
            thread_id: pinned,
            event: ThreadEvent::ThreadSaved,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }

    for target in [child, caller] {
        let refusal = check_eligibility(&pool, caller, target).await.unwrap_err();
        let AgentArchiveError::Refused((status, body)) = &refusal else {
            panic!("expected the pinned refusal, got {refusal:?}");
        };
        assert_eq!(status.as_u16(), 409);
        assert_eq!(body["reason"], crate::engine::archive_family::THREAD_PINNED);
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|m| m.contains("pinned")),
            "{:?}",
            body.0
        );
    }

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_agent_archive_records_the_agent_thread_and_leaves_the_parent() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = spawn(&bus, None).await;
    let child = spawn(&bus, Some(parent)).await;

    bus.emit(BusEvent::Thread {
        thread_id: child,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::with_actor(Some(agent_thread_actor(parent))),
    })
    .await
    .unwrap();

    let (kind, mode, source): (String, String, String) = sqlx::query_as(
        "SELECT payload->'actor'->>'kind', payload->'actor'->>'mode', \
                payload->'actor'->>'source_thread_id' \
           FROM events WHERE thread_id = $1 AND event_type = 'ThreadArchived'",
    )
    .bind(child)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (kind.as_str(), mode.as_str()),
        ("api", "agent"),
        "an agent archived it"
    );
    assert_eq!(source, parent.to_string(), "and the parent thread did");

    let states: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT thread_id, archive_state FROM thread_summaries WHERE thread_id = ANY($1)",
    )
    .bind(vec![parent, child])
    .fetch_all(&pool)
    .await
    .unwrap();
    for (id, state) in states {
        let expected = if id == child { "archived" } else { "inbox" };
        assert_eq!(state, expected, "thread {id}");
    }

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn no_agent_archives_the_home_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let home = crate::engine::home_thread::ensure_home_thread(&bus, &pool)
        .await
        .unwrap();

    let refusal = check_eligibility(&pool, home, home).await.unwrap_err();
    let AgentArchiveError::Refused((status, body)) = &refusal else {
        panic!("expected the route's refusal, got {refusal:?}");
    };
    assert_eq!(status.as_u16(), 409);
    assert_eq!(body["reason"], "home_thread");
    assert_eq!(refusal.reason(), "home_thread");

    let other = spawn(&bus, None).await;
    assert!(matches!(
        check_eligibility(&pool, other, home).await,
        Err(AgentArchiveError::Refused((_, body))) if body["reason"] == "home_thread"
    ));

    teardown_test_db(&db).await;
}
