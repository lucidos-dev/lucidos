//! The hold predicate, the release claim, and when an answer releases.

use super::*;
use crate::engine::agent_question::aq_test_helpers::{cc_meta, emit_user_question, seed_cc_thread};

use crate::test_support::{setup_test_db, teardown_test_db};

fn no_blobs() -> &'static Path {
    Path::new("/nonexistent-workspace")
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) -> Uuid {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: cc_meta(),
    })
    .await
    .expect("emit must not fail")
    .expect("thread events persist")
    .event_id
}

async fn hold(bus: &EventBus, thread_id: Uuid, text: &str) -> Uuid {
    emit(
        bus,
        thread_id,
        ThreadEvent::MessageHeld {
            text: text.into(),
            user_image_hashes: vec![],
            mode: ActorMode::Agent,
            origin: None,
        },
    )
    .await
}

async fn answer(bus: &EventBus, thread_id: Uuid, tool_use_id: &str) {
    emit(
        bus,
        thread_id,
        ThreadEvent::UserQuestionAnswered {
            tool_use_id: tool_use_id.into(),
            answer: AnswerKind::FreeText { text: "yes".into() },
        },
    )
    .await;
}

async fn count_releases(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'HeldMessageReleased'",
    )
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    .expect("count releases")
}

/// The reported shape. A parent follows up on a child parked on the user's
/// question. The follow-up must wait instead of superseding the question.
#[tokio::test]
async fn an_agent_message_is_held_behind_an_active_question() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_user_question(&bus, thread_id, "toolu-open#q0").await;

    assert!(message_is_held(&pool, thread_id, ActorMode::Agent, None).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A human message answers the question or supersedes it (ADR 0082). A message
/// already on the wire, or an engine re-entry (ADR 0255), is not held either.
#[tokio::test]
async fn only_a_fresh_agent_message_is_held() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_user_question(&bus, thread_id, "toolu-open#q0").await;

    assert!(!message_is_held(&pool, thread_id, ActorMode::Human, None).await);
    for pre in [
        PreEmittedOrigin::Message(Uuid::new_v4()),
        PreEmittedOrigin::EngineReentry(Uuid::new_v4()),
        PreEmittedOrigin::WaitReentry(Uuid::new_v4()),
    ] {
        assert!(
            !message_is_held(&pool, thread_id, ActorMode::Agent, Some(pre)).await,
            "{pre:?} must not be held"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An overtaken question has dead buttons, so an agent message still
/// supersedes it rather than wait on a question nobody can answer.
#[tokio::test]
async fn an_agent_message_is_not_held_behind_an_overtaken_question() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_user_question(&bus, thread_id, "toolu-open#q0").await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentToolCalled {
            name: "Bash".into(),
            args: serde_json::json!({"command": "ls"}),
            description: String::new(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            tool_use_id: "toolu-sibling".into(),
        },
    )
    .await;

    assert!(!message_is_held(&pool, thread_id, ActorMode::Agent, None).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// After a Cancel the question is gone but a message is still held. A newer
/// agent message must queue behind it, never jump ahead and restart the child.
/// Once the backlog is released, agent messages flow again.
#[tokio::test]
async fn an_agent_message_holds_behind_an_older_held_one() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_user_question(&bus, thread_id, "toolu-open#q0").await;
    let held = hold(&bus, thread_id, "first").await;
    answer(&bus, thread_id, "toolu-open#q0").await;

    assert!(message_is_held(&pool, thread_id, ActorMode::Agent, None).await);

    emit(
        &bus,
        thread_id,
        ThreadEvent::HeldMessageReleased {
            held_message_id: held,
        },
    )
    .await;
    assert!(!message_is_held(&pool, thread_id, ActorMode::Agent, None).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A release hands over the held messages one at a time, oldest first, and
/// marks each one. Once all are claimed nothing is left to deliver twice.
#[tokio::test]
async fn claims_release_held_messages_one_at_a_time_oldest_first() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_user_question(&bus, thread_id, "toolu-open#q0").await;
    let first = hold(&bus, thread_id, "first").await;
    let second = hold(&bus, thread_id, "second").await;

    let next = claim_next_held_message(&bus, &pool, no_blobs(), thread_id)
        .await
        .expect("the oldest is claimed first");
    assert_eq!((next.id, next.text.as_str()), (first, "first"));
    assert_eq!(
        count_releases(&pool, thread_id).await,
        1,
        "one at a time, so a crash strands at most one message"
    );
    let next = claim_next_held_message(&bus, &pool, no_blobs(), thread_id)
        .await
        .expect("then the next");
    assert_eq!(next.id, second);

    assert!(claim_next_held_message(&bus, &pool, no_blobs(), thread_id)
        .await
        .is_none());
    assert_eq!(count_releases(&pool, thread_id).await, 2);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Two releases can race. The index lets only one `HeldMessageReleased` per
/// held message land, so the loser cannot deliver it a second time.
#[tokio::test]
async fn a_held_message_can_be_released_only_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let held = hold(&bus, thread_id, "first").await;
    let release = || BusEvent::Thread {
        thread_id,
        event: ThreadEvent::HeldMessageReleased {
            held_message_id: held,
        },
        meta: EventMeta::NONE,
    };

    bus.emit(release()).await.expect("the first release lands");
    assert!(
        bus.emit(release()).await.is_err(),
        "a second release of the same message must be refused"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Cancel means stop, so it keeps the hold. Every other resolution releases.
#[test]
fn only_a_cancel_keeps_the_hold() {
    assert!(!answer_releases_held_messages(&AnswerKind::Canceled));
    for kind in [
        AnswerKind::FreeText { text: "yes".into() },
        AnswerKind::Selected {
            option_id: "opt-0".into(),
        },
        AnswerKind::Superseded,
    ] {
        assert!(
            answer_releases_held_messages(&kind),
            "{kind:?} must release"
        );
    }
}

/// A pending permission card waits on a human exactly as a question does. A
/// parent's follow-up used to deny it on the user's behalf. It must wait
/// instead, and once the card is resolved, agent messages flow again.
#[tokio::test]
async fn an_agent_message_is_held_behind_a_pending_permission_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-1".into(),
            tool_use_id: "toolu-edit".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({"file_path": "/tmp/x"}),
            summary: "Edit /tmp/x".into(),
        },
    )
    .await;

    assert!(message_is_held(&pool, thread_id, ActorMode::Agent, None).await);

    emit(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentPermissionResolved {
            request_id: "req-1".into(),
            allowed: true,
            reason: None,
            persist_scope: None,
        },
    )
    .await;
    assert!(!message_is_held(&pool, thread_id, ActorMode::Agent, None).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
