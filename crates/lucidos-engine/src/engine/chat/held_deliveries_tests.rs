//! The hold predicate, the collector the answer's resume reads, and the note it
//! builds.

use super::*;
use crate::engine::agent_question::aq_test_helpers::{opt, seed_chat_thread};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, AnswerKind, ChildCompletionStatus, EventChannel, EventMeta,
};
use crate::test_support::{setup_test_db, teardown_test_db};

const QUESTION: &str = "toolu-open#q0";

fn chat_meta() -> EventMeta {
    EventMeta {
        channel: Some(EventChannel::Chat),
        ..EventMeta::NONE
    }
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) -> Uuid {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: chat_meta(),
    })
    .await
    .expect("emit must not fail")
    .expect("thread events persist")
    .event_id
}

async fn ask(bus: &EventBus, thread_id: Uuid, tool_use_id: &str) {
    emit(
        bus,
        thread_id,
        ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.into(),
            cc_session_id: String::new(),
            question: "Pick one".into(),
            options: vec![opt("opt-0", "A")],
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
        },
    )
    .await;
}

async fn child_report(bus: &EventBus, thread_id: Uuid) -> Uuid {
    emit(
        bus,
        thread_id,
        ThreadEvent::ChildThreadCompleted {
            child_thread_id: Uuid::new_v4(),
            child_thread_title: Some("Step 5".into()),
            status: ChildCompletionStatus::Success,
            summary: "done".into(),
            pending_change_ids: vec![],
            sub_thread_pending_changes: vec![],
        },
    )
    .await
}

/// A wait resolution and its re-entry anchor, the pair `emit_resolution`
/// writes. Returns the anchor's id.
async fn wait_delivery(bus: &EventBus, thread_id: Uuid, text: &str) -> Uuid {
    let delivered = emit(
        bus,
        thread_id,
        ThreadEvent::EventWaitDelivered {
            wait_id: Uuid::new_v4(),
            event_id: Uuid::new_v4(),
            event_type: "CodingAgentIdled".into(),
            payload: serde_json::json!({}),
            matched_index: 0,
        },
    )
    .await;
    emit(
        bus,
        thread_id,
        ThreadEvent::PromptInjected {
            text: text.into(),
            mode: ActorMode::Agent,
            origin: None,
            injected_message_id: None,
            delivered_event_id: Some(delivered),
        },
    )
    .await
}

async fn wait_expiry(bus: &EventBus, thread_id: Uuid, text: &str) -> Uuid {
    emit(
        bus,
        thread_id,
        ThreadEvent::EventWaitExpired {
            wait_id: Uuid::new_v4(),
        },
    )
    .await;
    emit(
        bus,
        thread_id,
        ThreadEvent::PromptInjected {
            text: text.into(),
            mode: ActorMode::Agent,
            origin: None,
            injected_message_id: None,
            delivered_event_id: None,
        },
    )
    .await
}

async fn turn_activity(bus: &EventBus, thread_id: Uuid) {
    emit(
        bus,
        thread_id,
        ThreadEvent::ThoughtStreamed {
            text: "Context: 9 messages".into(),
        },
    )
    .await;
}

/// The reported shape. A restart dropped the question's turn, then the child
/// reported and a wait delivered. Neither may start a turn over the card.
#[tokio::test]
async fn a_delivery_is_held_behind_a_question_with_no_live_turn() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_chat_thread(&bus, thread_id, "run the nightly").await;
    ask(&bus, thread_id, QUESTION).await;
    let card = child_report(&bus, thread_id).await;
    let anchor = wait_delivery(&bus, thread_id, "Step 5 reported").await;

    for origin in [
        PreEmittedOrigin::EngineReentry(card),
        PreEmittedOrigin::WaitReentry(anchor),
    ] {
        assert!(
            delivery_is_held(&pool, thread_id, Some(origin), false).await,
            "{origin:?} must wait for the answer"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A live question turn reads its injections after the answer (ADR 0255), so
/// the gate stands aside and the delivery injects as before.
#[tokio::test]
async fn a_delivery_is_not_held_when_a_turn_is_live() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_chat_thread(&bus, thread_id, "run the nightly").await;
    ask(&bus, thread_id, QUESTION).await;
    let anchor = wait_delivery(&bus, thread_id, "Step 5 reported").await;

    assert!(
        !delivery_is_held(
            &pool,
            thread_id,
            Some(PreEmittedOrigin::WaitReentry(anchor)),
            true
        )
        .await
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Only a delivery nobody asked for is held. A Continue click and an answer's
/// resume are engine re-entries on their own notes, and a message is the
/// person's own words.
#[tokio::test]
async fn only_a_delivery_is_held() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let first = seed_chat_thread(&bus, thread_id, "run the nightly").await;
    ask(&bus, thread_id, QUESTION).await;
    let resume_note = emit(
        &bus,
        thread_id,
        ThreadEvent::PromptInjected {
            text: "resumed".into(),
            mode: ActorMode::Engine,
            origin: None,
            injected_message_id: None,
            delivered_event_id: None,
        },
    )
    .await;

    for origin in [
        None,
        Some(PreEmittedOrigin::Message(first)),
        Some(PreEmittedOrigin::EngineReentry(resume_note)),
        Some(PreEmittedOrigin::EngineReentry(Uuid::new_v4())),
    ] {
        assert!(
            !delivery_is_held(&pool, thread_id, origin, false).await,
            "{origin:?} must never be held"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// No open question, or one already answered or overtaken, holds nothing.
#[tokio::test]
async fn a_delivery_is_not_held_without_an_active_question() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_chat_thread(&bus, thread_id, "run the nightly").await;
    let before = wait_delivery(&bus, thread_id, "early").await;
    assert!(
        !delivery_is_held(
            &pool,
            thread_id,
            Some(PreEmittedOrigin::WaitReentry(before)),
            false
        )
        .await,
        "no question"
    );

    ask(&bus, thread_id, QUESTION).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::UserQuestionAnswered {
            tool_use_id: QUESTION.into(),
            answer: AnswerKind::FreeText {
                text: "yes".into(),
                image_hashes: vec![],
            },
        },
    )
    .await;
    let answered = wait_delivery(&bus, thread_id, "after the answer").await;
    assert!(
        !delivery_is_held(
            &pool,
            thread_id,
            Some(PreEmittedOrigin::WaitReentry(answered)),
            false
        )
        .await,
        "answered question"
    );

    ask(&bus, thread_id, "toolu-second#q0").await;
    turn_activity(&bus, thread_id).await;
    let overtaken = wait_delivery(&bus, thread_id, "after the overtake").await;
    assert!(
        !delivery_is_held(
            &pool,
            thread_id,
            Some(PreEmittedOrigin::WaitReentry(overtaken)),
            false
        )
        .await,
        "overtaken question"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The resume carries every wait delivery held since the question, oldest
/// first, and counts the child reports. A delivery from before the question
/// was read by an earlier turn, so it stays out.
#[tokio::test]
async fn the_resume_collects_every_delivery_held_since_the_question() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_chat_thread(&bus, thread_id, "run the nightly").await;
    wait_delivery(&bus, thread_id, "before the question").await;
    turn_activity(&bus, thread_id).await;
    ask(&bus, thread_id, QUESTION).await;
    child_report(&bus, thread_id).await;
    wait_delivery(&bus, thread_id, "Step 5 reported").await;
    wait_expiry(&bus, thread_id, "Step 6 timed out").await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::UserQuestionAnswered {
            tool_use_id: QUESTION.into(),
            answer: AnswerKind::FreeText {
                text: "yes".into(),
                image_hashes: vec![],
            },
        },
    )
    .await;

    assert_eq!(
        held_deliveries(&pool, thread_id, QUESTION).await,
        HeldDeliveries {
            wait_texts: vec!["Step 5 reported".into(), "Step 6 timed out".into()],
            child_reports: 1,
        }
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Two waits resolving together write both resolutions before either anchor,
/// and a child report can land in between. Every anchor is still carried, and
/// a live turn's acknowledgement of an injected message is not.
#[tokio::test]
async fn the_resume_finds_an_anchor_its_resolution_does_not_precede() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let first = seed_chat_thread(&bus, thread_id, "run the nightly").await;
    ask(&bus, thread_id, QUESTION).await;
    for _ in 0..2 {
        emit(
            &bus,
            thread_id,
            ThreadEvent::EventWaitDelivered {
                wait_id: Uuid::new_v4(),
                event_id: Uuid::new_v4(),
                event_type: "ChangeProposed".into(),
                payload: serde_json::json!({}),
                matched_index: 0,
            },
        )
        .await;
    }
    child_report(&bus, thread_id).await;
    for text in ["first wait", "second wait"] {
        emit(
            &bus,
            thread_id,
            ThreadEvent::PromptInjected {
                text: text.into(),
                mode: ActorMode::Agent,
                origin: None,
                injected_message_id: None,
                delivered_event_id: None,
            },
        )
        .await;
    }
    emit(
        &bus,
        thread_id,
        ThreadEvent::PromptInjected {
            text: "an acknowledged message".into(),
            mode: ActorMode::Agent,
            origin: None,
            injected_message_id: Some(first),
            delivered_event_id: None,
        },
    )
    .await;

    assert_eq!(
        held_deliveries(&pool, thread_id, QUESTION).await,
        HeldDeliveries {
            wait_texts: vec!["first wait".into(), "second wait".into()],
            child_reports: 1,
        }
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A delivery a turn already read, before the restart dropped it, is not
/// carried twice.
#[tokio::test]
async fn the_resume_skips_a_delivery_a_turn_already_read() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_chat_thread(&bus, thread_id, "run the nightly").await;
    ask(&bus, thread_id, QUESTION).await;
    child_report(&bus, thread_id).await;
    wait_delivery(&bus, thread_id, "read already").await;
    turn_activity(&bus, thread_id).await;
    wait_delivery(&bus, thread_id, "still held").await;

    assert_eq!(
        held_deliveries(&pool, thread_id, QUESTION).await,
        HeldDeliveries {
            wait_texts: vec!["still held".into()],
            child_reports: 0,
        }
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn the_note_is_unchanged_when_nothing_was_held() {
    assert_eq!(
        resume_note_with_held_deliveries("resume", &HeldDeliveries::default()),
        "resume"
    );
}

#[test]
fn the_note_carries_each_wait_text_and_points_at_child_reports() {
    let note = resume_note_with_held_deliveries(
        "resume",
        &HeldDeliveries {
            wait_texts: vec!["first".into(), "second".into()],
            child_reports: 2,
        },
    );
    let parts: Vec<&str> = note.split("\n\n---\n\n").collect();
    assert_eq!(parts[0], "resume");
    assert!(parts[1].contains("waited for the answer"), "{note}");
    assert!(
        parts[2].contains("Child thread reports waiting: 2"),
        "{note}"
    );
    assert_eq!(&parts[3..], ["first", "second"]);
}
