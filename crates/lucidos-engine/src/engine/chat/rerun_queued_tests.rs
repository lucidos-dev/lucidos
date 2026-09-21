//! A follow-up queued behind an interrupted turn survives the restart.
//!
//! Reported case, from the dev workspace event store: a `MessageReceived` at
//! 08:57:40, an `engine_shutdown` abort five seconds later, then a resume that
//! carried only the engine note. The message was named by no
//! `UserPromptInjected`, no `QueuedMessageRemoved` and no response, so it sat
//! as a "Queued" bubble nobody could clear but the user.
//!
//! These drive the two functions the resume is made of, against a bare
//! `EventBus`. They pin the emitted-event surface and the prompt the loop is
//! handed. That is the whole fix, short of the LLM call.
//!
//! Plan: `docs/plans/2026-09-21-a-queued-follow-up-survives-the-restart.md`.

use super::*;
use crate::engine::chat::queued_recovery::window_end_sequence;
use crate::engine::thread_events::{AbortCause, TriggerInvocation};
use crate::test_support::{setup_test_db, teardown_test_db};

const NOTE: &str = "[Engine note] Your previous attempt was interrupted.";

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

async fn abort(bus: &EventBus, thread_id: Uuid, turn: Uuid) {
    emit_with_meta(
        bus,
        thread_id,
        ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: AbortCause::EngineShutdown,
        },
        EventMeta {
            request_event_id: Some(turn),
            ..EventMeta::NONE
        },
    )
    .await;
}

/// One resume, in the production order: sample the fence, emit the anchor,
/// ingest what is owed. The fence must be sampled first.
async fn resume(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    anchor: ChatResumeAnchor,
    channel: EventChannel,
) -> (Uuid, Option<RecoveredMessages>) {
    // Text-only fixtures reach no blob store.
    resume_in(
        bus,
        pool,
        std::path::Path::new("/nonexistent-workspace"),
        thread_id,
        anchor,
        channel,
    )
    .await
}

async fn resume_in(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    workspace: &std::path::Path,
    thread_id: Uuid,
    anchor: ChatResumeAnchor,
    channel: EventChannel,
) -> (Uuid, Option<RecoveredMessages>) {
    let window_end = window_end_sequence(pool, thread_id)
        .await
        .expect("window end");
    let anchor_event_id = emit_resume_anchor(bus, thread_id, anchor, NOTE, channel, None)
        .await
        .expect("anchor emit");
    let recovered = ingest_queued_messages_for_resume(
        ResumeStores {
            bus,
            pool,
            workspace,
        },
        thread_id,
        anchor,
        anchor_event_id,
        channel,
        window_end,
    )
    .await;
    (anchor_event_id, recovered)
}

/// Every `UserPromptInjected` on the thread, oldest first, as
/// `(injected_message_id, request_event_id, channel)`.
async fn injections(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Vec<(Option<Uuid>, Option<Uuid>, Option<String>)> {
    sqlx::query_as(
        "SELECT NULLIF(payload->>'injected_message_id','')::uuid, \
                NULLIF(payload->>'request_event_id','')::uuid, \
                payload->>'channel' \
           FROM events \
          WHERE aggregate_id = $1 AND event_type = 'UserPromptInjected' \
          ORDER BY sequence ASC",
    )
    .bind(thread_id.to_string())
    .fetch_all(pool)
    .await
    .expect("injection read")
}

async fn count_of(pool: &sqlx::PgPool, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = $2",
    )
    .bind(thread_id.to_string())
    .bind(event_type)
    .fetch_one(pool)
    .await
    .expect("count")
}

/// The reported case. The queued follow-up reaches the resumed turn as real
/// user text, and is announced so the pinned bubble stops reading as queued.
#[tokio::test]
async fn the_reported_case_answers_the_queued_message() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("summarize the tickets")).await;
    let queued = emit(&bus, thread_id, message("wtf r u up to?")).await;
    abort(&bus, thread_id, turn).await;

    let (anchor_event_id, recovered) = resume(
        &bus,
        &pool,
        thread_id,
        ChatResumeAnchor::NewBoundary {
            interrupted_turn: Some(turn),
        },
        EventChannel::Chat,
    )
    .await;

    let recovered = recovered.expect("the stranded follow-up is owed an answer");
    assert!(
        recovered.text.contains("wtf r u up to?"),
        "the resumed turn carries the user's words: {}",
        recovered.text
    );

    // The engine note is the boundary's own injection. The follow-up's names
    // the message, which is what clears the bubble.
    let announced = injections(&pool, thread_id).await;
    assert_eq!(announced.len(), 2, "the note, then the recovered message");
    assert_eq!(announced[1].0, Some(queued));
    assert_eq!(
        announced[1].1,
        Some(anchor_event_id),
        "anchored on the resume, so the turn stays one exchange"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The engine note is read first. It reports what the aborted run already did,
/// and the follow-ups were sent against that context.
#[tokio::test]
async fn two_queued_messages_are_ingested_in_order_after_the_note() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("summarize the tickets")).await;
    emit(&bus, thread_id, message("only the open ones")).await;
    emit(&bus, thread_id, message("and sort by age")).await;
    abort(&bus, thread_id, turn).await;

    let (_, recovered) = resume(
        &bus,
        &pool,
        thread_id,
        ChatResumeAnchor::NewBoundary {
            interrupted_turn: Some(turn),
        },
        EventChannel::Chat,
    )
    .await;

    let prompt =
        prompt_with_recovered_messages(NOTE.to_string(), recovered.as_ref().map(|r| &*r.text));
    let note_at = prompt.find(NOTE).expect("the note is in the prompt");
    let first_at = prompt.find("only the open ones").expect("first follow-up");
    let second_at = prompt.find("and sort by age").expect("second follow-up");
    assert!(note_at < first_at, "the note is read before the follow-ups");
    assert!(first_at < second_at, "arrival order is kept");

    // Coalesced into ONE turn, so the pair is answered together rather than
    // opening a turn each.
    assert_eq!(
        prompt.matches("[Engine note]").count(),
        1,
        "one turn, not one per message"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A retraction beats a recovery, including one the user made while the engine
/// was down.
#[tokio::test]
async fn a_retracted_queued_message_stays_retracted_across_a_restart() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("summarize the tickets")).await;
    let queued = emit(&bus, thread_id, message("never mind")).await;
    abort(&bus, thread_id, turn).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::QueuedMessageRemoved {
            removed_message_id: queued,
        },
    )
    .await;

    let (_, recovered) = resume(
        &bus,
        &pool,
        thread_id,
        ChatResumeAnchor::NewBoundary {
            interrupted_turn: Some(turn),
        },
        EventChannel::Chat,
    )
    .await;

    assert!(recovered.is_none(), "a retracted message stays gone");
    let announced = injections(&pool, thread_id).await;
    assert!(
        announced
            .iter()
            .all(|(injected, ..)| *injected != Some(queued)),
        "nothing announces a message the user took back"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A message the loop took before the abort is already in the thread's history,
/// so recovering it would say it twice.
#[tokio::test]
async fn a_message_ingested_before_the_abort_is_not_recovered() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("summarize the tickets")).await;
    let taken = emit(&bus, thread_id, message("only the open ones")).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::UserPromptInjected {
            text: "only the open ones".into(),
            mode: ActorMode::Human,
            origin: None,
            injected_message_id: Some(taken),
            delivered_event_id: None,
        },
    )
    .await;
    emit(&bus, thread_id, message("and sort by age")).await;
    abort(&bus, thread_id, turn).await;

    let (_, recovered) = resume(
        &bus,
        &pool,
        thread_id,
        ChatResumeAnchor::NewBoundary {
            interrupted_turn: Some(turn),
        },
        EventChannel::Chat,
    )
    .await;

    let recovered = recovered.expect("the undrained one is still owed");
    assert!(recovered.text.contains("and sort by age"));
    assert!(
        !recovered.text.contains("only the open ones"),
        "the loop already took that one: {}",
        recovered.text
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The first pass writes the marker the second pass reads, so a double Continue
/// ingests the message once. That write is the whole idempotency mechanism.
#[tokio::test]
async fn a_double_continue_ingests_the_queued_message_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("summarize the tickets")).await;
    let queued = emit(&bus, thread_id, message("wtf r u up to?")).await;
    abort(&bus, thread_id, turn).await;

    let anchor = ChatResumeAnchor::NewBoundary {
        interrupted_turn: Some(turn),
    };
    let (_, first) = resume(&bus, &pool, thread_id, anchor, EventChannel::Chat).await;
    let (_, second) = resume(&bus, &pool, thread_id, anchor, EventChannel::Chat).await;

    assert!(first.is_some(), "the first resume owes the message");
    assert!(second.is_none(), "the second finds it already announced");

    let naming_it = injections(&pool, thread_id)
        .await
        .into_iter()
        .filter(|(injected, ..)| *injected == Some(queued))
        .count();
    assert_eq!(naming_it, 1, "announced exactly once");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An answer-driven resume was never aborted, so it must stay invisible. The
/// recovered message's own announcement is correct and visible. A boundary
/// panel is not.
#[tokio::test]
async fn existing_turn_recovers_without_opening_a_boundary() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("ask me about colors")).await;
    let queued = emit(&bus, thread_id, message("actually, blue")).await;

    let (anchor_event_id, recovered) = resume(
        &bus,
        &pool,
        thread_id,
        ChatResumeAnchor::ExistingTurn(turn),
        EventChannel::Chat,
    )
    .await;

    assert_eq!(
        anchor_event_id, turn,
        "the interrupted turn stays the anchor"
    );
    assert!(recovered.is_some(), "the queued message is still owed");
    assert_eq!(
        count_of(&pool, thread_id, "ContinuationStarted").await,
        0,
        "nothing was interrupted, so nothing opens a boundary"
    );

    let announced = injections(&pool, thread_id).await;
    assert_eq!(
        announced.len(),
        1,
        "only the recovered message announces itself, never an engine note"
    );
    assert_eq!(announced[0].0, Some(queued));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A trigger thread's resume must not be rewritten to chat. The
/// `ContinuationStarted` projection arm writes `source = <channel>`, so a
/// mis-stamped recovery would change what the thread IS.
#[tokio::test]
async fn a_trigger_thread_keeps_its_channel_through_the_recovery() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit_with_meta(
        &bus,
        thread_id,
        ThreadEvent::TriggerStarted {
            trigger_id: "morning-digest".into(),
            trigger_name: Some("Morning digest".into()),
            prompt: Some("summarize overnight".into()),
            invocation: Some(TriggerInvocation::Schedule),
            origin: None,
            go_to_review: false,
            model: None,
            reasoning_effort: None,
        },
        EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    )
    .await;
    emit(&bus, thread_id, message("skip the closed tickets")).await;
    abort(&bus, thread_id, turn).await;

    resume(
        &bus,
        &pool,
        thread_id,
        ChatResumeAnchor::NewBoundary {
            interrupted_turn: Some(turn),
        },
        EventChannel::Trigger,
    )
    .await;

    for (_, _, channel) in injections(&pool, thread_id).await {
        assert_eq!(
            channel.as_deref(),
            Some("trigger"),
            "every injection the recovery emits stays on the trigger channel"
        );
    }

    let source: String =
        sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("thread summary");
    assert_eq!(source, "trigger", "the thread is still a trigger thread");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The legacy fallback carries no interrupted turn, so it recovers nothing. A
/// recovery with no lower bound would sweep the whole thread.
#[tokio::test]
async fn an_anchor_without_an_interrupted_turn_recovers_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    emit(&bus, thread_id, message("summarize the tickets")).await;
    emit(&bus, thread_id, message("and the closed ones")).await;

    let (_, recovered) = resume(
        &bus,
        &pool,
        thread_id,
        ChatResumeAnchor::NewBoundary {
            interrupted_turn: None,
        },
        EventChannel::Chat,
    )
    .await;

    assert!(recovered.is_none());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The bytes reach the resumed turn, not just the blob store.
///
/// Rebuilding the attachment and then handing the loop `None` was the shape
/// this pins against: an image-only follow-up would reach the model with no
/// user content at all.
#[tokio::test]
async fn a_recovered_attachment_reaches_the_resumed_turn() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let workspace = tempfile::tempdir().expect("temp workspace");

    let png: Vec<u8> = vec![
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00,
    ];
    let blob = crate::core::blobs::write_blob(workspace.path(), &png).expect("blob write");

    let turn = emit(&bus, thread_id, message("summarize the tickets")).await;
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
    abort(&bus, thread_id, turn).await;

    let (_, recovered) = resume_in(
        &bus,
        &pool,
        workspace.path(),
        thread_id,
        ChatResumeAnchor::NewBoundary {
            interrupted_turn: Some(turn),
        },
        EventChannel::Chat,
    )
    .await;

    let recovered = recovered.expect("the follow-up is owed an answer");
    let images = recovered.images.expect("its attachment travels with it");
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].mime_type, "image/png");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A retraction landing between two announcements still wins for the message
/// it names. The check runs per message, immediately before its own emit, so
/// the batch ahead of it does not carry the retracted one through.
#[tokio::test]
async fn a_retraction_mid_batch_still_beats_the_recovery() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let turn = emit(&bus, thread_id, message("summarize the tickets")).await;
    emit(&bus, thread_id, message("only the open ones")).await;
    let second = emit(&bus, thread_id, message("never mind that")).await;
    abort(&bus, thread_id, turn).await;

    // Stands in for the trash icon landing while the first message is being
    // announced: the removal is already persisted when the loop reaches it.
    emit(
        &bus,
        thread_id,
        ThreadEvent::QueuedMessageRemoved {
            removed_message_id: second,
        },
    )
    .await;

    let (_, recovered) = resume(
        &bus,
        &pool,
        thread_id,
        ChatResumeAnchor::NewBoundary {
            interrupted_turn: Some(turn),
        },
        EventChannel::Chat,
    )
    .await;

    let recovered = recovered.expect("the first message is still owed");
    assert!(recovered.text.contains("only the open ones"));
    assert!(
        !recovered.text.contains("never mind that"),
        "a retracted message never reaches the prompt: {}",
        recovered.text
    );
    let announced = injections(&pool, thread_id).await;
    assert!(
        announced
            .iter()
            .all(|(injected, ..)| *injected != Some(second)),
        "and nothing announces it either"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
