//! The approval gate: a proposal on this thread, then a reply from the user.

use super::*;
use crate::engine::thread_events::{ActorMode, AnswerKind, EventChannel, MessageOrigin};
use crate::test_support::{setup_test_db, teardown_test_db};

fn device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "test-device".into(),
    }
}

/// Persist a message, as the chat API does before any turn reads it.
async fn message(bus: &EventBus, thread_id: Uuid, origin: Option<MessageOrigin>) -> Uuid {
    let mode = if origin.is_some() {
        ActorMode::Human
    } else {
        ActorMode::Agent
    };
    bus.emit_for_id(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "reply".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode,
            model: None,
            reasoning_effort: None,
            origin,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap()
}

/// A turn takes `message` as its request: its first step carries the id.
async fn take_as_request(bus: &EventBus, thread_id: Uuid, message: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TextStreamed {
            text: "Reading".into(),
        },
        meta: EventMeta {
            request_event_id: Some(message),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

/// The user's message, read by the turn it started.
async fn reply(bus: &EventBus, thread_id: Uuid) {
    let id = message(bus, thread_id, Some(device())).await;
    take_as_request(bus, thread_id, id).await;
}

fn entry(action: &str) -> TriageProposalEntry {
    TriageProposalEntry {
        thread_id: Uuid::new_v4(),
        action: action.into(),
        reason: "idle for 3 days, nothing pending".into(),
    }
}

async fn gate(pool: &PgPool, caller: Uuid) -> Result<Vec<TriageProposalEntry>, ApprovalRefusal> {
    approved_proposal(pool, caller).await.unwrap()
}

#[tokio::test]
async fn apply_needs_a_proposal_and_a_user_reply_after_it() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let caller = Uuid::new_v4();
    reply(&bus, caller).await;
    assert_eq!(gate(&pool, caller).await, Err(ApprovalRefusal::NoProposal));

    let proposed = vec![entry("archive"), entry("pin")];
    record_proposal(&bus, caller, proposed.clone())
        .await
        .unwrap();
    assert_eq!(
        gate(&pool, caller).await,
        Err(ApprovalRefusal::NoReply),
        "the user's message before the proposal is not a reply to it"
    );

    let agent_message = message(&bus, caller, None).await;
    take_as_request(&bus, caller, agent_message).await;
    assert_eq!(
        gate(&pool, caller).await,
        Err(ApprovalRefusal::NoReply),
        "an agent's message is not the user's reply"
    );

    reply(&bus, caller).await;
    assert_eq!(gate(&pool, caller).await, Ok(proposed));

    // A newer proposal needs its own reply.
    record_proposal(&bus, caller, vec![entry("archive")])
        .await
        .unwrap();
    assert_eq!(gate(&pool, caller).await, Err(ApprovalRefusal::NoReply));

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn answering_a_question_card_counts_as_the_users_reply() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let caller = Uuid::new_v4();
    reply(&bus, caller).await;
    record_proposal(&bus, caller, vec![entry("archive")])
        .await
        .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: caller,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: "approve-triage".into(),
            answer: AnswerKind::Selected {
                option_id: "apply".into(),
            },
        },
        meta: EventMeta::with_actor(Some(device())),
    })
    .await
    .unwrap();
    assert!(gate(&pool, caller).await.is_ok());

    teardown_test_db(&db).await;
}

/// A proposal on another thread opens nothing here.
#[tokio::test]
async fn a_proposal_binds_only_its_own_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (proposer, other) = (Uuid::new_v4(), Uuid::new_v4());
    reply(&bus, proposer).await;
    record_proposal(&bus, proposer, vec![entry("archive")])
        .await
        .unwrap();
    reply(&bus, other).await;
    assert_eq!(gate(&pool, other).await, Err(ApprovalRefusal::NoProposal));

    teardown_test_db(&db).await;
}

/// A follow-up typed while the triage turn runs is persisted before the agent
/// reads it. Until a turn takes it or the loop injects it, the user has not
/// answered the proposal. A withdrawn follow-up never answers it.
#[tokio::test]
async fn a_queued_message_counts_only_once_the_agent_read_it() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let caller = Uuid::new_v4();
    reply(&bus, caller).await;
    record_proposal(&bus, caller, vec![entry("archive")])
        .await
        .unwrap();

    let withdrawn = message(&bus, caller, Some(device())).await;
    bus.emit(BusEvent::Thread {
        thread_id: caller,
        event: ThreadEvent::QueuedMessageRemoved {
            removed_message_id: withdrawn,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    let queued = message(&bus, caller, Some(device())).await;
    assert_eq!(
        gate(&pool, caller).await,
        Err(ApprovalRefusal::NoReply),
        "an unread or withdrawn message is not a reply"
    );

    bus.emit(BusEvent::Thread {
        thread_id: caller,
        event: ThreadEvent::PromptInjected {
            text: "reply".into(),
            mode: ActorMode::Human,
            origin: Some(device()),
            injected_message_id: Some(queued),
            delivered_event_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert!(gate(&pool, caller).await.is_ok());

    teardown_test_db(&db).await;
}
