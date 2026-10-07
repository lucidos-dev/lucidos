//! An agent archiving on its own authority (ADR 0310): the ladder, the waiting
//! refusal, and what the event records.

use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::EventChannel;
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

async fn authorize(
    pool: &PgPool,
    caller: Option<Uuid>,
    target: Uuid,
) -> Result<AgentArchiveTarget, AgentArchiveError> {
    LucidosEngine::authorize_agent_archive(pool, caller, target).await
}

/// A thread reaches itself and its own direct children, and nothing else: not
/// its parent, a sibling, a grandchild, or a thread that does not exist.
#[tokio::test]
async fn an_agent_reaches_only_itself_and_its_direct_children() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let root = spawn(&bus, None).await;
    let caller = spawn(&bus, Some(root)).await;
    let sibling = spawn(&bus, Some(root)).await;
    let child = spawn(&bus, Some(caller)).await;
    let grandchild = spawn(&bus, Some(child)).await;

    assert_eq!(
        authorize(&pool, Some(caller), caller).await.unwrap(),
        AgentArchiveTarget::Caller,
        "a running caller may ask; the request waits for its turn to end"
    );
    assert_eq!(
        authorize(&pool, Some(caller), child).await.unwrap(),
        AgentArchiveTarget::DirectChild
    );
    for other in [root, sibling, grandchild] {
        let refusal = authorize(&pool, Some(caller), other).await.unwrap_err();
        assert!(
            matches!(refusal, AgentArchiveError::NotYourThread(id) if id == other),
            "{refusal:?}"
        );
        assert_eq!(refusal.status_code(), 403);
        assert_eq!(refusal.reason(), "not_your_thread");
    }
    let unknown = Uuid::new_v4();
    assert!(matches!(
        authorize(&pool, Some(caller), unknown).await,
        Err(AgentArchiveError::UnknownThread(id)) if id == unknown
    ));
    assert!(matches!(
        authorize(&pool, None, child).await,
        Err(AgentArchiveError::NoCaller)
    ));

    teardown_test_db(&db).await;
}

/// A caller waiting on the user is refused with the Archive route's own
/// status and slug (ADR 0259), not a wording of its own.
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
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let refusal = authorize(&pool, Some(caller), caller).await.unwrap_err();
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

/// A thread the user threw away cannot be archived, as it cannot be moved.
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

    let refusal = authorize(&pool, Some(caller), child).await.unwrap_err();
    assert!(matches!(refusal, AgentArchiveError::Discarded(id) if id == child));
    assert_eq!(refusal.status_code(), 409);

    teardown_test_db(&db).await;
}

/// A pinned thread is the user's to archive (ADR 0312). An agent is refused
/// with the cascade's own slug, whether the target is its child or itself.
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
        let refusal = authorize(&pool, Some(caller), target).await.unwrap_err();
        let AgentArchiveError::Refused((status, body)) = &refusal else {
            panic!("expected the pinned refusal, got {refusal:?}");
        };
        assert_eq!(status.as_u16(), 409);
        assert_eq!(body["reason"], crate::api::threads::archive::THREAD_PINNED);
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

/// The event names an agent and the thread it ran in. Archiving a child moves
/// the child alone: its parent's row is untouched.
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

/// No agent archives the home thread (ADR 0362, invariant I10): not its own
/// agent, and not a thread whose child it could never be. The refusal is the
/// Archive route's own status and slug.
#[tokio::test]
async fn no_agent_archives_the_home_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let home = crate::engine::home_thread::ensure_home_thread(&bus, &pool)
        .await
        .unwrap();

    let refusal = authorize(&pool, Some(home), home).await.unwrap_err();
    let AgentArchiveError::Refused((status, body)) = &refusal else {
        panic!("expected the route's refusal, got {refusal:?}");
    };
    assert_eq!(status.as_u16(), 409);
    assert_eq!(body["reason"], "home_thread");
    assert_eq!(refusal.reason(), "home_thread");

    let other = spawn(&bus, None).await;
    assert!(matches!(
        authorize(&pool, Some(other), home).await,
        Err(AgentArchiveError::NotYourThread(id)) if id == home
    ));

    teardown_test_db(&db).await;
}
