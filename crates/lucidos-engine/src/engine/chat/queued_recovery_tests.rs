//! One test per arm of [`STRANDED_QUEUED_MESSAGES_SQL`].
//!
//! The predicate is the whole fix, so it is driven directly here rather than
//! only through its reader. Each arm is a separate reason a queued message is
//! NOT owed an answer. Where an arm could pass for the wrong reason, the test
//! asserts the decoy first: the same fixture without the excluding marker,
//! which must come back.

use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{AbortCause, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};

/// No blob store is needed for a message with no attachments, and every
/// fixture here is text.
fn no_blobs() -> &'static Path {
    Path::new("/nonexistent-workspace")
}

fn message(text: &str) -> ThreadEvent {
    ThreadEvent::MessageReceived {
        voice_session_id: None,
        text: text.to_string(),
        user_image_hashes: vec![],
        device_id: None,
        device: None,
        image_description: None,
        parent_thread_id: None,
        spawning_event_id: None,
        mode: ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    }
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) -> Uuid {
    emit_with_meta(bus, thread_id, event, EventMeta::NONE).await
}

async fn emit_with_meta(
    bus: &EventBus,
    thread_id: Uuid,
    event: ThreadEvent,
    meta: EventMeta,
) -> Uuid {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta,
    })
    .await
    .expect("emit must not fail")
    .expect("thread events persist")
    .event_id
}

/// Announce `message_id` as consumed, exactly as the loop does when it drains
/// an injection.
async fn announce_injected(bus: &EventBus, thread_id: Uuid, message_id: Uuid) {
    emit(
        bus,
        thread_id,
        ThreadEvent::UserPromptInjected {
            text: "whatever".into(),
            mode: ActorMode::Human,
            origin: None,
            injected_message_id: Some(message_id),
            delivered_event_id: None,
        },
    )
    .await;
}

/// The window every test reads with: from `turn` to whatever exists now.
async fn undrained(pool: &sqlx::PgPool, thread_id: Uuid, turn: Uuid) -> Vec<String> {
    let end = window_end_sequence(pool, thread_id)
        .await
        .expect("window end");
    undrained_user_messages(pool, no_blobs(), thread_id, turn, end)
        .await
        .expect("query must run")
        .into_iter()
        .map(|p| p.text)
        .collect()
}

/// The reported shape. A turn is running, the user types a follow-up, the
/// engine is torn down. The follow-up is owed an answer, and nothing in the
/// event store says otherwise.
#[tokio::test]
async fn a_queued_follow_up_behind_an_aborted_turn_is_owed_an_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("summarize the tickets")).await;
    emit(&bus, thread_id, message("wtf r u up to?")).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: AbortCause::EngineShutdown,
        },
    )
    .await;

    assert_eq!(undrained(&pool, thread_id, turn).await, ["wtf r u up to?"]);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The turn's OWN originating event is the lower bound, so a turn never
/// recovers itself. Recovering it would re-ask what the interrupted run was
/// already working on.
#[tokio::test]
async fn the_interrupted_turn_does_not_recover_its_own_message() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("summarize the tickets")).await;

    assert!(undrained(&pool, thread_id, turn).await.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A message the loop already took carries a `UserPromptInjected`. That marker
/// is what makes a second pass a no-op, so this is the idempotency test at the
/// predicate level.
#[tokio::test]
async fn an_announced_message_is_no_longer_owed() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("start")).await;
    let queued = emit(&bus, thread_id, message("also check the closed ones")).await;

    assert_eq!(
        undrained(&pool, thread_id, turn).await,
        ["also check the closed ones"],
        "decoy: with no marker it IS owed"
    );

    announce_injected(&bus, thread_id, queued).await;

    assert!(
        undrained(&pool, thread_id, turn).await.is_empty(),
        "the announcement is what a second pass reads"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A retraction beats a recovery, including one made while the engine was down.
/// The trash icon is the only thing that should decide this.
#[tokio::test]
async fn a_retracted_message_is_not_owed() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("start")).await;
    let queued = emit(&bus, thread_id, message("never mind")).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::QueuedMessageRemoved {
            removed_message_id: queued,
        },
    )
    .await;

    assert!(undrained(&pool, thread_id, turn).await.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A turn that is STILL RUNNING owns its message. The predicate asks whether
/// anything picked the message up, not whether a turn finished.
///
/// Asking for a terminator alone was the earlier form, and this is the case it
/// got wrong: Continue clicked over a live turn recovered the very message that
/// turn was answering, so the user got two answers to one message.
#[tokio::test]
async fn a_message_a_running_turn_owns_is_not_owed() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("start")).await;
    let running = emit(&bus, thread_id, message("and the closed ones")).await;

    assert_eq!(
        undrained(&pool, thread_id, turn).await,
        ["and the closed ones"],
        "decoy: before its turn does anything it IS owed"
    );

    // One streamed token is enough: a turn has picked the message up.
    emit_with_meta(
        &bus,
        thread_id,
        ThreadEvent::TextStreamed {
            text: "Reading".into(),
        },
        EventMeta {
            request_event_id: Some(running),
            ..EventMeta::NONE
        },
    )
    .await;

    assert!(
        undrained(&pool, thread_id, turn).await.is_empty(),
        "a turn is working on it, so nobody else may take it"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The same arm from the settled end: a message its turn's terminator names.
/// Every terminal type counts. A canceled turn settled the message as much as a
/// generated one did, because the user saw it picked up.
#[tokio::test]
async fn a_message_with_a_terminator_is_not_owed() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    for terminal in [
        ThreadEvent::ResponseGenerated {
            text: "done".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: AbortCause::EngineShutdown,
        },
    ] {
        let thread_id = Uuid::new_v4();
        let turn = emit(&bus, thread_id, message("start")).await;
        let own_turn = emit(&bus, thread_id, message("a turn of its own")).await;

        assert_eq!(
            undrained(&pool, thread_id, turn).await,
            ["a turn of its own"],
            "decoy: with no terminator it IS owed"
        );

        emit_with_meta(
            &bus,
            thread_id,
            terminal,
            EventMeta {
                request_event_id: Some(own_turn),
                ..EventMeta::NONE
            },
        )
        .await;

        assert!(
            undrained(&pool, thread_id, turn).await.is_empty(),
            "a message its own terminator names is already settled"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A message sent during a call is owed an answer like any other. Branching on
/// that would let the doer answer the same question two ways by input channel,
/// which is the failure ADR 0149 names. The channel drain does not branch on it
/// either, so the two definitions of undrained agree.
#[tokio::test]
async fn a_message_sent_during_a_call_is_owed_like_any_other() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("start")).await;
    let during_a_call = match message("and the closed ones") {
        ThreadEvent::MessageReceived { text, mode, .. } => ThreadEvent::MessageReceived {
            voice_session_id: Some(Uuid::new_v4()),
            text,
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        other => other,
    };
    emit(&bus, thread_id, during_a_call).await;

    assert_eq!(
        undrained(&pool, thread_id, turn).await,
        ["and the closed ones"]
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The fence. A message landing after the reader sampled its window belongs to
/// whoever handles it next, so recovering it here would answer it twice.
#[tokio::test]
async fn a_message_past_the_window_end_is_left_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("start")).await;
    emit(&bus, thread_id, message("inside the window")).await;
    let fence = window_end_sequence(&pool, thread_id)
        .await
        .expect("window end");
    emit(&bus, thread_id, message("after the fence")).await;

    let texts: Vec<String> = undrained_user_messages(&pool, no_blobs(), thread_id, turn, fence)
        .await
        .expect("query must run")
        .into_iter()
        .map(|p| p.text)
        .collect();
    assert_eq!(texts, ["inside the window"]);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Oldest first, so the resumed turn reads what the user typed in the order
/// they typed it.
#[tokio::test]
async fn owed_messages_come_back_oldest_first() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("start")).await;
    emit(&bus, thread_id, message("first")).await;
    emit(&bus, thread_id, message("second")).await;
    emit(&bus, thread_id, message("third")).await;

    assert_eq!(
        undrained(&pool, thread_id, turn).await,
        ["first", "second", "third"]
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A recovered message carries the bytes it was sent with, not just its text.
///
/// An image-only follow-up is the case that decides it. The live injection path
/// hands the model the bytes, so a recovery that dropped them would answer a
/// different question than the user asked.
#[tokio::test]
async fn a_recovered_message_carries_its_attachment() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let workspace = tempfile::tempdir().expect("temp workspace");

    // Smallest thing `sniff_image_mime` accepts: a PNG signature plus IHDR.
    let png: Vec<u8> = vec![
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00,
    ];
    let blob = crate::core::blobs::write_blob(workspace.path(), &png).expect("blob write");

    let turn = emit(&bus, thread_id, message("start")).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "what is wrong with this?".into(),
            user_image_hashes: vec![blob.hash.clone()],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
    )
    .await;

    let end = window_end_sequence(&pool, thread_id)
        .await
        .expect("window end");
    let prompts = undrained_user_messages(&pool, workspace.path(), thread_id, turn, end)
        .await
        .expect("query must run");

    let images = prompts[0]
        .images
        .as_ref()
        .expect("the attachment comes back");
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].mime_type, "image/png");
    assert!(!images[0].base64.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A hash whose blob is gone costs the attachment, never the message. The user
/// still asked something, and dropping the whole follow-up would be worse.
#[tokio::test]
async fn a_missing_blob_does_not_cost_the_message() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("start")).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "and this one?".into(),
            user_image_hashes: vec!["f".repeat(64)],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
    )
    .await;

    let end = window_end_sequence(&pool, thread_id)
        .await
        .expect("window end");
    let prompts = undrained_user_messages(&pool, no_blobs(), thread_id, turn, end)
        .await
        .expect("query must run");

    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].text, "and this one?");
    assert!(prompts[0].images.is_none());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The rebuilt prompt carries the fields the live path would have carried. The
/// announcement then names the right message, and the model is told who sent
/// it.
#[tokio::test]
async fn the_rebuilt_prompt_keeps_the_message_identity() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("start")).await;
    let queued = emit(
        &bus,
        thread_id,
        ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "and the closed ones".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: Some(MessageOrigin::system()),
        },
    )
    .await;

    let end = window_end_sequence(&pool, thread_id)
        .await
        .expect("window end");
    let prompts = undrained_user_messages(&pool, no_blobs(), thread_id, turn, end)
        .await
        .expect("query must run");

    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].event_id, Some(queued));
    assert_eq!(prompts[0].mode, ActorMode::Human);
    assert!(
        prompts[0].origin.is_some(),
        "origin survives the round trip"
    );
    assert!(
        matches!(prompts[0].kind, InjectedPromptKind::UserText),
        "a recovered message is user text, so it announces itself"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
