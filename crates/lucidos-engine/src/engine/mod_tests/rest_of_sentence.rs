//! Which live turn gets the rest of a spoken sentence (ADR 0198).
//!
//! The words already have their own row. So an offer is worth making only to
//! the turn that took the ask, and only once that turn has started reading its
//! history. Everywhere else the row reaches the right turn by itself.
//!
//! An orphaned ask joins the live turn as itself, so that turn takes its tail.

use super::common::*;
use super::*;
use uuid::Uuid;

fn the_rest(asked_turn: Uuid) -> InjectedPrompt {
    InjectedPrompt {
        text: "and the weather".into(),
        event_id: Some(asked_turn),
        mode: thread_events::ActorMode::Human,
        spawning_event_id: None,
        images: None,
        origin: None,
        kind: InjectedPromptKind::SpokenContinuation,
    }
}

fn offer(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    thread_id: Uuid,
    asked_turn: Uuid,
) -> RestOfSentence {
    inject_the_rest_of_a_sentence(threads, thread_id, asked_turn, the_rest(asked_turn))
}

/// Coalesce the ask `delegation` into the live turn, as the chat path does
/// when the ask finds the thread busy.
fn coalesce(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    thread_id: Uuid,
    delegation: Uuid,
) {
    let ask = InjectedPrompt {
        event_id: Some(delegation),
        ..test_prompt("what is on my calendar")
    };
    assert!(threads.lock().unwrap().get(&thread_id).unwrap().inject(ask));
}

/// The turn the ask started, reading its history already: it gets the words,
/// which its history may have missed.
#[test]
fn the_turn_the_ask_started_gets_the_rest() {
    let threads = make_threads();
    let thread_id = Uuid::new_v4();
    let delegation = Uuid::new_v4();
    let (_token, mut rx, guard) = register(&threads, thread_id);
    record_request_event_id(&threads, thread_id, guard.generation(), delegation);
    record_history_read_begun(&threads, thread_id, guard.generation());

    assert_eq!(
        offer(&threads, thread_id, delegation),
        RestOfSentence::Injected
    );
    assert_eq!(rx.try_recv().unwrap().text, "and the weather");
}

/// The asked turn queued behind another turn on the same thread, so the live
/// turn is somebody else's. Told "finish the request you
/// are working on", it would bolt the caller's words onto the wrong request.
#[test]
fn another_turn_on_the_thread_is_not_given_the_rest() {
    let threads = make_threads();
    let thread_id = Uuid::new_v4();
    let (_token, mut rx, guard) = register(&threads, thread_id);
    record_request_event_id(&threads, thread_id, guard.generation(), Uuid::new_v4());
    record_history_read_begun(&threads, thread_id, guard.generation());

    assert_eq!(
        offer(&threads, thread_id, Uuid::new_v4()),
        RestOfSentence::Held
    );
    assert!(rx.try_recv().is_err(), "nothing may reach the other turn");
    coalesce(&threads, thread_id, Uuid::new_v4());
    assert_eq!(rx.try_recv().unwrap().text, "what is on my calendar");
    assert!(
        rx.try_recv().is_err(),
        "another ask must not release the words"
    );
}

/// An ask on a busy thread coalesces into the live turn instead of queuing.
/// That turn took the ask, so it is owed the rest too.
#[test]
fn a_turn_the_ask_was_coalesced_into_gets_the_rest() {
    let threads = make_threads();
    let thread_id = Uuid::new_v4();
    let delegation = Uuid::new_v4();
    let (_token, mut rx, guard) = register(&threads, thread_id);
    record_request_event_id(&threads, thread_id, guard.generation(), Uuid::new_v4());
    record_history_read_begun(&threads, thread_id, guard.generation());
    coalesce(&threads, thread_id, delegation);

    assert_eq!(
        offer(&threads, thread_id, delegation),
        RestOfSentence::Injected
    );
    assert_eq!(rx.try_recv().unwrap().text, "what is on my calendar");
    assert_eq!(rx.try_recv().unwrap().text, "and the weather");
}

/// The ask's turn task is still on its way to the busy thread when the rest
/// of the sentence lands. The live turn holds the words,
/// and they follow the ask in once it is coalesced, in speaking order.
#[test]
fn the_rest_that_beats_its_ask_follows_the_ask_in() {
    let threads = make_threads();
    let thread_id = Uuid::new_v4();
    let delegation = Uuid::new_v4();
    let (_token, mut rx, guard) = register(&threads, thread_id);
    record_request_event_id(&threads, thread_id, guard.generation(), Uuid::new_v4());
    record_history_read_begun(&threads, thread_id, guard.generation());

    assert_eq!(offer(&threads, thread_id, delegation), RestOfSentence::Held);
    assert!(rx.try_recv().is_err());
    coalesce(&threads, thread_id, delegation);
    assert_eq!(rx.try_recv().unwrap().text, "what is on my calendar");
    assert_eq!(rx.try_recv().unwrap().text, "and the weather");
    assert!(rx.try_recv().is_err(), "the words arrive once");
}

/// A turn that has not read its history yet reads the row there. Given the
/// words as well, it would read them twice.
#[test]
fn a_turn_still_to_read_its_history_is_not_given_the_rest() {
    let threads = make_threads();
    let thread_id = Uuid::new_v4();
    let delegation = Uuid::new_v4();
    let (_token, mut rx, guard) = register(&threads, thread_id);
    record_request_event_id(&threads, thread_id, guard.generation(), delegation);

    assert_eq!(
        offer(&threads, thread_id, delegation),
        RestOfSentence::LeftToHistory
    );
    assert!(rx.try_recv().is_err());
}

/// An evicted turn keeps unwinding under its old generation. Its history read
/// must not mark the replacement, which may not have read its own yet.
#[test]
fn a_stale_generation_cannot_mark_the_history_read() {
    let threads = make_threads();
    let thread_id = Uuid::new_v4();
    let delegation = Uuid::new_v4();
    let (_token, _rx, guard) = register(&threads, thread_id);
    record_request_event_id(&threads, thread_id, guard.generation(), delegation);
    record_history_read_begun(&threads, thread_id, guard.generation() + 1);

    assert_eq!(
        offer(&threads, thread_id, delegation),
        RestOfSentence::LeftToHistory
    );
}

/// No live turn at all: the ordinary miss, and never an error.
#[test]
fn an_idle_thread_is_given_nothing() {
    let threads = make_threads();
    assert_eq!(
        offer(&threads, Uuid::new_v4(), Uuid::new_v4()),
        RestOfSentence::LeftToHistory
    );
}

// ── hand_over_or_admit: the orphan hand-over ──

fn orphan(text: &str) -> InjectedPrompt {
    InjectedPrompt {
        event_id: Some(Uuid::new_v4()),
        ..test_prompt(text)
    }
}

fn texts(prompts: impl IntoIterator<Item = InjectedPrompt>) -> Vec<String> {
    prompts.into_iter().map(|prompt| prompt.text).collect()
}

/// An ask orphaned behind an earlier message goes into the live turn as
/// itself. So that turn took it, and the rest of the sentence it held for the
/// ask follows it in.
#[test]
fn an_orphaned_ask_handed_to_the_live_turn_takes_its_held_rest() {
    let threads = make_threads();
    let thread_id = Uuid::new_v4();
    let (_token, mut rx, guard) = register(&threads, thread_id);
    record_request_event_id(&threads, thread_id, guard.generation(), Uuid::new_v4());
    record_history_read_begun(&threads, thread_id, guard.generation());
    let mut ask = orphan("what is on my calendar");
    ask.images = Some(vec![crate::api::ChatImage {
        base64: "image-bytes".into(),
        mime_type: "image/png".into(),
    }]);
    let ask_id = ask.event_id.unwrap();
    assert_eq!(offer(&threads, thread_id, ask_id), RestOfSentence::Held);

    let rest = hand_over_or_admit(
        &threads,
        &Default::default(),
        thread_id,
        vec![orphan("check the build"), ask],
    );

    assert!(matches!(rest, OrphanAdmission::HandedOver));
    assert!(threads.lock().unwrap()[&thread_id].took(ask_id));
    let received: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert_eq!(received[1].event_id, Some(ask_id));
    assert!(matches!(received[1].kind, InjectedPromptKind::UserText));
    assert_eq!(
        received[1].images.as_ref().unwrap()[0].base64,
        "image-bytes"
    );
    assert_eq!(
        texts(received),
        [
            "check the build",
            "what is on my calendar",
            "and the weather"
        ]
    );
}

/// A thread with no live turn keeps the whole batch, in order, for its re-run.
#[test]
fn an_idle_thread_keeps_the_whole_batch_in_order() {
    let threads = make_threads();
    let batch = vec![orphan("m0"), orphan("m1"), orphan("m2")];

    let thread_id = Uuid::new_v4();
    let OrphanAdmission::Admitted(registration, rest) =
        hand_over_or_admit(&threads, &Default::default(), thread_id, batch)
    else {
        panic!("idle thread must admit")
    };
    assert!(threads.lock().unwrap().contains_key(&thread_id));
    assert!(matches!(
        try_register_thread(&threads, &Default::default(), thread_id),
        Admission::Busy { .. }
    ));
    drop(registration);

    assert_eq!(texts(rest), ["m0", "m1", "m2"]);
}

/// A turn whose channel is closed takes none of the batch, so no orphan is
/// split from the ones behind it.
#[test]
fn a_turn_that_stopped_reading_takes_none_of_the_batch() {
    let threads = make_threads();
    let thread_id = Uuid::new_v4();
    let (_token, rx, _guard) = register(&threads, thread_id);
    drop(rx);

    let OrphanAdmission::Closing(rest) = hand_over_or_admit(
        &threads,
        &Default::default(),
        thread_id,
        vec![orphan("m0"), orphan("m1")],
    ) else {
        panic!("closing turn must refuse")
    };

    assert_eq!(texts(rest), ["m0", "m1"]);
}

#[test]
fn dropping_pre_admission_releases_and_recovers_injections() {
    let threads = make_threads();
    let thread_id = Uuid::new_v4();
    let registration = register(&threads, thread_id);
    let recovered = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = recovered.clone();
    let pending = PreAdmission::new(registration, move |prompts| {
        *sink.lock().unwrap() = prompts;
    });
    assert!(threads.lock().unwrap()[&thread_id].inject(orphan("follow-up")));
    drop(pending);
    assert!(!threads.lock().unwrap().contains_key(&thread_id));
    assert_eq!(texts(recovered.lock().unwrap().drain(..)), ["follow-up"]);
}

#[test]
fn taking_pre_admission_keeps_the_receiver_and_does_not_recover() {
    let threads = make_threads();
    let thread_id = Uuid::new_v4();
    let pending = PreAdmission::new(register(&threads, thread_id), |_| {
        panic!("an admitted turn owns its injections");
    });
    assert!(threads.lock().unwrap()[&thread_id].inject(orphan("follow-up")));
    let (_, mut rx, guard) = pending.take();
    assert!(threads.lock().unwrap().contains_key(&thread_id));
    assert_eq!(rx.try_recv().unwrap().text, "follow-up");
    drop(guard);
}

#[test]
fn setup_distinguishes_its_reservation_from_another_live_turn() {
    let threads = make_threads();
    let thread_id = Uuid::new_v4();
    assert!(!has_other_live_turn(&threads, thread_id, None));
    let pending = PreAdmission::new(register(&threads, thread_id), |_| {});
    assert!(has_other_live_turn(&threads, thread_id, None));
    assert!(!has_other_live_turn(&threads, thread_id, Some(&pending)));

    // An evicted reservation must still recognize its replacement as live.
    threads.lock().unwrap().remove(&thread_id);
    let replacement = register(&threads, thread_id);
    assert!(has_other_live_turn(&threads, thread_id, Some(&pending)));
    drop(pending);
    assert!(has_other_live_turn(&threads, thread_id, None));
    drop(replacement);
}
