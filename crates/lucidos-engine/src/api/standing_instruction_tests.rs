//! The two shapes, and everything that looks like one without being one.
//!
//! Every turn here is emitted through `EventBus`, not inserted. The predicate
//! reads a payload the engine wrote, so a hand-built row could agree with the
//! test and disagree with production.

use super::*;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::{
    ActorMode, EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadDirection, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};

/// A user turn, opened by whoever `origin` names.
async fn open_turn(bus: &EventBus, thread_id: Uuid, mode: ActorMode, origin: MessageOrigin) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "work".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode,
            model: None,
            reasoning_effort: None,
            origin: Some(origin),
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

fn device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "device-abc".into(),
        label: "My MacBook".into(),
    }
}

/// The owner speaking. Their words in the turn are the press.
#[tokio::test]
async fn a_turn_the_owner_opened_carries_the_standing_instruction() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    open_turn(&bus, thread, ActorMode::Human, device()).await;

    assert!(carries_standing_instruction(&pool, Some(thread), None).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The instruction is not inherited. A thread another thread spawned opens its
/// own turn with a `ThreadLink`, whoever opened the spawner's.
#[tokio::test]
async fn an_agent_spawned_thread_carries_none() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    open_turn(
        &bus,
        thread,
        ActorMode::Agent,
        MessageOrigin::ThreadLink {
            thread_id: Uuid::new_v4(),
            title: None,
            spawning_event_id: None,
            mode: ActorMode::Agent,
            direction: ThreadDirection::Parent,
        },
    )
    .await;

    assert!(!carries_standing_instruction(&pool, Some(thread), None).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Record who authored a trigger, which is how a fire of it is weighed.
async fn author_trigger(bus: &EventBus, trigger_id: &str, actor: Option<MessageOrigin>) {
    bus.emit(BusEvent::System(SystemEvent::TriggerCreated {
        trigger_id: trigger_id.to_string(),
        payload: serde_json::json!({ "trigger_id": trigger_id, "name": trigger_id }),
        actor,
    }))
    .await
    .unwrap();
}

/// Start a trigger thread's turn, the way a fire does.
async fn fire_trigger_on(bus: &EventBus, thread_id: Uuid, trigger_id: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            provider: None,
            trigger_id: trigger_id.to_string(),
            trigger_name: Some("Nightly release".into()),
            prompt: None,
            invocation: None,
            origin: Some(MessageOrigin::engine(EngineReason::Scheduler {
                trigger_id: trigger_id.to_string(),
                trigger_name: Some("Nightly release".into()),
            })),
            go_to_review: false,
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

/// An intent trigger's fire, which runs on a thread. The turn starts with
/// `TriggerStarted`, and the owner wrote the trigger that fired.
#[tokio::test]
async fn an_intent_triggers_fire_carries_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    author_trigger(&bus, "nightly", Some(device())).await;
    fire_trigger_on(&bus, thread, "nightly").await;

    assert!(carries_standing_instruction(&pool, Some(thread), None).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A script trigger's fire, which has no thread to read a turn from. The
/// second record of the same shape: the fire's id rides the caller's own token.
#[tokio::test]
async fn a_script_triggers_fire_carries_it_with_no_thread_at_all() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    author_trigger(&bus, "nightly", Some(device())).await;

    assert!(carries_standing_instruction(&pool, None, Some("nightly")).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The escalation this predicate must not allow. An agent creates triggers
/// freely. A fire that counted on its own would let any thread promote itself
/// to the owner's authority in two tool calls.
#[tokio::test]
async fn a_trigger_the_agent_wrote_carries_none() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    author_trigger(
        &bus,
        "self-made",
        Some(MessageOrigin::Api {
            user_agent: None,
            mode: ActorMode::Agent,
            source_thread_id: Some(Uuid::new_v4()),
        }),
    )
    .await;
    fire_trigger_on(&bus, thread, "self-made").await;

    assert!(!carries_standing_instruction(&pool, Some(thread), None).await);
    assert!(
        !carries_standing_instruction(&pool, None, Some("self-made")).await,
        "and the script-trigger record of the same fire answers the same"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The owner switching a trigger back on is authorizing it, and an agent
/// rewriting one takes that back. Newest wins, so the answer tracks the last
/// person to say what the trigger does.
#[tokio::test]
async fn the_newest_authoring_event_decides() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    author_trigger(&bus, "shared", None).await;
    fire_trigger_on(&bus, thread, "shared").await;
    assert!(
        !carries_standing_instruction(&pool, Some(thread), None).await,
        "a trigger nobody can show the owner authored is nobody's"
    );

    bus.emit(BusEvent::System(SystemEvent::TriggerEnabled {
        trigger_id: "shared".into(),
        payload: serde_json::json!({ "trigger_id": "shared" }),
        actor: Some(device()),
    }))
    .await
    .unwrap();
    assert!(
        carries_standing_instruction(&pool, Some(thread), None).await,
        "the owner switched it on, which ADR 0168 counts as authorizing it"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A fire naming no trigger has nothing to weigh, so it answers no rather than
/// falling through to "some trigger fired, good enough".
#[tokio::test]
async fn a_fire_naming_no_trigger_carries_none() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    author_trigger(&bus, "nightly", Some(device())).await;
    fire_trigger_on(&bus, thread, "").await;

    assert!(!carries_standing_instruction(&pool, Some(thread), None).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Resume the thread's turn, stamped with whatever origin the resume path uses.
/// Production writes both shapes: no origin at all (`auto_resume_after_switch`)
/// and an engine origin.
async fn resume_turn(bus: &EventBus, thread_id: Uuid, origin: Option<MessageOrigin>) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ContinuationStarted {
            branch: String::new(),
            origin,
            reason: Some("auto_resume_after_switch".into()),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

fn engine_resume() -> Option<MessageOrigin> {
    Some(MessageOrigin::engine(EngineReason::ContinuationStarted))
}

fn spawning_thread() -> MessageOrigin {
    MessageOrigin::ThreadLink {
        thread_id: Uuid::new_v4(),
        title: None,
        spawning_event_id: None,
        mode: ActorMode::Agent,
        direction: ThreadDirection::Parent,
    }
}

/// A resume is the owner's turn carrying on, not a new turn somebody else
/// opened (ADR 0168 clause 6). The user's own engine switch resumed this one,
/// and it then refused to create a top-thread.
#[tokio::test]
async fn an_engine_resume_keeps_the_owners_turn() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    open_turn(&bus, thread, ActorMode::Human, device()).await;
    resume_turn(&bus, thread, None).await;
    resume_turn(&bus, thread, engine_resume()).await;

    assert!(carries_standing_instruction(&pool, Some(thread), None).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The resume inherits, and never promotes: a turn another thread opened stays
/// that thread's after the engine resumes it. The CURRENT turn decides, so an
/// owner turn before it lends nothing.
#[tokio::test]
async fn an_engine_resume_of_an_agent_turn_carries_none() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    open_turn(&bus, thread, ActorMode::Human, device()).await;
    open_turn(&bus, thread, ActorMode::Agent, spawning_thread()).await;
    resume_turn(&bus, thread, engine_resume()).await;

    assert!(!carries_standing_instruction(&pool, Some(thread), None).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A resumed fire is still weighed by who authored its trigger.
#[tokio::test]
async fn an_engine_resume_of_a_fire_is_weighed_by_its_trigger() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let owners = Uuid::new_v4();
    let agents = Uuid::new_v4();
    author_trigger(&bus, "nightly", Some(device())).await;
    author_trigger(
        &bus,
        "self-made",
        Some(MessageOrigin::Api {
            user_agent: None,
            mode: ActorMode::Agent,
            source_thread_id: Some(Uuid::new_v4()),
        }),
    )
    .await;
    fire_trigger_on(&bus, owners, "nightly").await;
    fire_trigger_on(&bus, agents, "self-made").await;
    resume_turn(&bus, owners, None).await;
    resume_turn(&bus, agents, None).await;

    assert!(carries_standing_instruction(&pool, Some(owners), None).await);
    assert!(!carries_standing_instruction(&pool, Some(agents), None).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A resume with no turn behind it has nothing to inherit, so it is a no.
#[tokio::test]
async fn a_lone_engine_resume_carries_none() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    resume_turn(&bus, thread, engine_resume()).await;

    assert!(!carries_standing_instruction(&pool, Some(thread), None).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The owner clicking Continue is the owner speaking, whoever opened the turn
/// it resumes.
#[tokio::test]
async fn the_owner_clicking_continue_carries_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    open_turn(&bus, thread, ActorMode::Agent, spawning_thread()).await;
    resume_turn(&bus, thread, Some(device())).await;

    assert!(carries_standing_instruction(&pool, Some(thread), None).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Nothing to read is a no, and so is a caller presenting neither input. Both
/// are the fail-closed direction: an unknown never stands in for the owner.
#[tokio::test]
async fn nothing_to_read_carries_none() {
    let (pool, db_name) = setup_test_db().await;

    assert!(!carries_standing_instruction(&pool, Some(Uuid::new_v4()), None).await);
    assert!(!carries_standing_instruction(&pool, None, None).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
