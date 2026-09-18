use base64::Engine as _;

use super::*;
use crate::voice::provider::AudioFormat;

fn opening() -> SessionOpening {
    SessionOpening {
        instructions: "You are Lucidos.".to_string(),
        resident_block: "[WHAT YOU ALREADY KNOW]\nWorkspace: dev".to_string(),
        voice: "marin".to_string(),
        transcriber: "gpt-4o-mini-transcribe".to_string(),
        audio: AudioFormat::default(),
        language: None,
    }
}

fn frame(value: serde_json::Value, turn: &mut TurnState) -> Vec<VoiceEvent> {
    map_event(&value, turn, Instant::now())
}

fn caller_said(text: &str) -> serde_json::Value {
    serde_json::json!({ "type": "session.input_transcript.delta", "delta": text })
}

fn talker_said(text: &str) -> serde_json::Value {
    serde_json::json!({ "type": "session.output_transcript.delta", "delta": text })
}

fn talker_streamed(pcm: &[u8]) -> serde_json::Value {
    let encoded = base64::engine::general_purpose::STANDARD.encode(pcm);
    serde_json::json!({ "type": "session.output_audio.delta", "delta": encoded })
}

// The opening frame.

/// The model rides the payload rather than the URL, which is the first thing
/// this protocol does differently from the Realtime one.
#[test]
fn the_opening_frame_names_the_model_and_starts_a_session() {
    let start = session_start("gpt-live-1", &opening());
    assert_eq!(start["type"], "session.start");
    assert_eq!(start["session"]["model"], "gpt-live-1");
}

/// Client delegation, never the managed one. Responses mode would rent a second
/// brain that holds tools and acts, which ADR 0149 forbids.
#[test]
fn the_session_delegates_to_us_and_never_to_a_rented_backend() {
    let start = session_start("gpt-live-1", &opening());
    assert_eq!(start["session"]["delegation"]["type"], "client");
    assert!(
        start["session"]["delegation"]["responses"].is_null(),
        "a responses backend was configured: {}",
        start
    );
}

/// The persona opens the instructions. What this session KNOWS arrives
/// separately, as the resident block.
#[test]
fn the_opening_frame_carries_the_persona_and_not_the_resident_block() {
    let start = session_start("gpt-live-1", &opening());
    let instructions = start["session"]["instructions"]
        .as_str()
        .expect("instructions are a string");
    assert!(
        instructions.starts_with("You are Lucidos."),
        "the persona is not the prefix: {}",
        instructions
    );
    assert!(
        !start.to_string().contains("WHAT YOU ALREADY KNOW"),
        "the resident block was folded into the opening frame"
    );
}

/// The one thing a tool-less talker has nowhere else to read.
///
/// Realtime carries this in the `delegate` tool's description, which client
/// delegation never declares. Without it the talker promises to go and look and
/// then asks for nothing, which is the defect this closes.
#[test]
fn a_tool_less_talker_is_told_when_to_ask_for_help() {
    let start = session_start("gpt-live-1", &opening());
    let instructions = start["session"]["instructions"]
        .as_str()
        .expect("instructions are a string");
    for label in [
        "Backend tools:",
        "Delegate to the backend when:",
        "Do not delegate to the backend when:",
    ] {
        assert!(
            instructions.contains(label),
            "{} is missing from the policy: {}",
            label,
            instructions
        );
    }
}

/// The load-bearing half: a promise and the handover are one turn.
///
/// Its absence is what a caller actually meets. They hear "on it", nothing is
/// asked for, and they wait until they hang up.
#[test]
fn the_talker_may_not_promise_work_it_has_not_handed_over() {
    let start = session_start("gpt-live-1", &opening());
    let instructions = start["session"]["instructions"]
        .as_str()
        .expect("instructions are a string");
    assert!(
        instructions.contains("SAME turn"),
        "nothing ties the promise to the handover: {}",
        instructions
    );
}

/// Handing over is also how this talker settles what is waiting on the caller.
///
/// It holds no answering tool, so the ask is the whole of its reach. The engine
/// reads one against a parked question as the caller's answer. A talker that
/// never asks therefore leaves the card open for good, which is the reported
/// call.
///
/// Both halves, because one without the other is a different defect. Told only
/// to hand over, it settles the card with somebody still weighing it up.
#[test]
fn the_talker_is_told_that_handing_over_is_what_settles_a_card() {
    let start = session_start("gpt-live-1", &opening());
    let instructions = start["session"]["instructions"]
        .as_str()
        .expect("instructions are a string");
    for half in [
        "handing their words over IS the answer",
        "has not chosen yet",
    ] {
        assert!(
            instructions.contains(half),
            "{} is missing from the policy: {}",
            half,
            instructions
        );
    }
}

/// The protocol has no transcription settings, so the opening frame names
/// neither the workspace's transcriber nor its language.
///
/// Asserted rather than assumed. `SessionOpening` carries both, the Realtime
/// provider configures both, and the natural reading of that asymmetry is that
/// this one forgot. It did not: there is no key to write them to.
#[test]
fn the_opening_frame_configures_no_transcriber_and_no_language() {
    let mut opening = opening();
    opening.transcriber = "gpt-4o-mini-transcribe".to_string();
    opening.language = Some(crate::voice::language::SpokenLanguage {
        code: Some("nb".to_string()),
        name: "Norwegian".to_string(),
    });
    let start = session_start("gpt-live-1", &opening).to_string();
    for absent in ["transcription", "gpt-4o-mini-transcribe", "\"nb\""] {
        assert!(
            !start.contains(absent),
            "{} reached the opening frame",
            absent
        );
    }
}

/// One format both ways, at the rate the seam named. A client that cannot
/// negotiate must not be handed a rate it did not ask for.
#[test]
fn the_audio_format_and_voice_come_from_the_opening() {
    let start = session_start("gpt-live-1", &opening());
    let audio = &start["session"]["audio"];
    assert_eq!(audio["format"]["type"], "audio/pcm");
    assert_eq!(audio["format"]["rate"], 24_000);
    assert_eq!(audio["output"]["voice"], "marin");
}

/// The talker declares no tools, because client delegation has none to declare.
/// A payload that grew a tool list would be Responses mode by another name.
///
/// Read off the session OBJECT, never off the frame's text. The instructions
/// now discuss delegating in prose. A substring scan hits that policy and says
/// nothing about what was declared.
#[test]
fn the_opening_frame_declares_no_tools() {
    let start = session_start("gpt-live-1", &opening());
    let session = &start["session"];
    for absent in ["tools", "tool_choice"] {
        assert!(
            session[absent].is_null(),
            "{} reached the opening frame: {}",
            absent,
            start
        );
    }
}

// Appends.

/// The provider requires the key on all three appends. Left out, the append is
/// refused and the note reaches nobody.
#[test]
fn every_append_writes_its_delegation_key_even_when_there_is_none() {
    let session = append_frame(COMMENTARY, "lucidos-1", None, "the order shipped");
    assert!(
        session
            .as_object()
            .expect("object")
            .contains_key("delegation_id"),
        "{}",
        session
    );
    assert!(session["delegation_id"].is_null());

    let owned = append_frame(THINKING, "lucidos-2", Some("item_9tA"), "Taken.");
    assert_eq!(owned["delegation_id"], "item_9tA");
}

/// Every append names itself, which is what lets a refusal say what it
/// refused. With no name, `error_events` reads every refusal as the session
/// ending and drops a working call.
#[test]
fn every_append_names_itself() {
    let frame = append_frame(COMMENTARY, "lucidos-7", None, "the order shipped");
    assert_eq!(frame["event_id"], "lucidos-7");
}

/// The block is what the session KNOWS, so it rides the quiet channel.
///
/// Sent as steering it was new orders landing after the session started, and
/// the talker answered them aloud before the caller had spoken (ADR 0211).
#[test]
fn the_resident_block_opens_on_the_quiet_channel() {
    let frames = opening_appends(&opening().resident_block);
    assert_eq!(frames.len(), 1, "{:?}", frames);
    assert_eq!(frames[0]["type"], THINKING);
    assert!(frames[0]["delegation_id"].is_null(), "{}", frames[0]);
    assert_eq!(frames[0]["event_id"], "resident-0");
    assert!(
        frames[0]["content"]
            .as_str()
            .expect("content is a string")
            .contains("Workspace: dev"),
        "{}",
        frames[0]
    );
}

/// Every section switched off is a real state, and it opens no append at all.
#[test]
fn a_block_with_nothing_in_it_opens_no_append() {
    for blank in ["", "   ", "\n\n"] {
        assert!(
            opening_appends(blank).is_empty(),
            "{:?} sent a frame",
            blank
        );
    }
}

/// A block over the cap arrives whole, and every piece names itself so a
/// refusal can say which one it refused.
#[test]
fn a_long_block_opens_as_several_quiet_appends() {
    let frames = opening_appends(&"alpha bravo ".repeat(400));
    assert!(frames.len() > 1, "a 4800-char block stayed one append");
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(frame["type"], THINKING);
        assert_eq!(frame["event_id"], format!("resident-{}", index));
    }
}

/// Two kinds, and which one is used decides whether the caller hears it.
///
/// The provider has a third, `session.instructions.append`. This module sends
/// none, because steering is not what either of the two things it says is
/// (ADR 0211).
#[test]
fn each_append_kind_is_the_one_its_seam_member_promises() {
    assert_eq!(append_frame(COMMENTARY, "e", None, "x")["type"], COMMENTARY);
    assert_eq!(append_frame(THINKING, "e", None, "x")["type"], THINKING);
}

/// A note inside the cap is one append, whole and unchanged.
#[test]
fn a_short_note_is_one_chunk_and_keeps_every_character() {
    assert_eq!(
        chunks("the order shipped today"),
        vec!["the order shipped today"]
    );
}

/// Even nothing yields a piece, so a caller cannot lose a note to an empty
/// return.
#[test]
fn an_empty_note_still_yields_one_chunk() {
    assert_eq!(chunks(""), vec![""]);
}

/// A blank resident block is a real state: every section can be switched off.
/// Sending it would be a frame the provider has to refuse.
#[test]
fn a_blank_note_is_worth_no_append_at_all() {
    for blank in ["", "   ", "\n\n"] {
        assert!(
            appendable(blank).is_empty(),
            "{:?} produced an append",
            blank
        );
    }
    assert_eq!(appendable("Workspace: dev"), vec!["Workspace: dev"]);
}

/// The cap is the provider's, and going over it is refused. A refused answer is
/// one the caller never hears, so a long one is split rather than cut.
#[test]
fn a_long_answer_is_split_and_nothing_is_lost() {
    let long = "alpha bravo ".repeat(400);
    let pieces = chunks(&long);
    assert!(pieces.len() > 1, "a 4800-char answer stayed one append");
    for piece in &pieces {
        assert!(
            piece.chars().count() <= APPEND_CHARS,
            "a piece of {} chars is over the cap",
            piece.chars().count()
        );
    }
    let rejoined = pieces.join(" ");
    assert_eq!(
        rejoined.split_whitespace().count(),
        long.split_whitespace().count()
    );
}

/// A whitespace run longer than the cap yields a piece that trims to nothing.
/// An empty append is a frame the provider refuses, so it is dropped.
#[test]
fn a_long_run_of_whitespace_yields_no_empty_piece() {
    let padded = format!("{}the order shipped", " ".repeat(APPEND_CHARS * 2));
    let pieces = chunks(&padded);
    assert!(!pieces.is_empty());
    for piece in &pieces {
        assert!(!piece.trim().is_empty(), "an empty piece would be refused");
    }
    assert!(pieces.concat().contains("the order shipped"));
}

/// Multi-byte text is split on a char boundary. Slicing by byte index would
/// panic mid-character.
#[test]
fn a_long_answer_of_multibyte_text_never_splits_a_character() {
    let long = "é".repeat(APPEND_CHARS * 3);
    let pieces = chunks(&long);
    assert!(pieces.len() > 1);
    let rejoined: String = pieces.concat();
    assert_eq!(rejoined.chars().count(), long.chars().count());
    assert!(rejoined.chars().all(|c| c == 'é'));
}

// The talker's own stream.

/// Audio is forwarded as it arrives. Nothing here writes it down.
#[test]
fn talker_audio_is_decoded_and_forwarded() {
    let mut turn = TurnState::default();
    let events = frame(talker_streamed(&[1, 2, 3]), &mut turn);
    assert_eq!(events, vec![VoiceEvent::Audio(vec![1, 2, 3])]);
}

/// What the talker is saying reaches the caller's screen as it says it.
#[test]
fn talker_words_are_forwarded_as_they_arrive() {
    let mut turn = TurnState::default();
    let events = frame(talker_said("checking"), &mut turn);
    assert_eq!(
        events,
        vec![VoiceEvent::TalkerTranscript {
            text: "checking".to_string()
        }]
    );
}

/// The turn ends when the talker's own WORDS go quiet, and it carries
/// everything that turn said.
#[test]
fn a_talker_that_stops_saying_words_ends_its_turn() {
    let mut turn = TurnState::default();
    frame(talker_said("on it, "), &mut turn);
    frame(talker_said("one moment"), &mut turn);

    assert_eq!(
        talker_went_quiet(&mut turn),
        vec![VoiceEvent::TalkerTurnEnded {
            transcript: "on it, one moment".to_string(),
            usage: ApiUsage::default(),
        }]
    );
}

/// **The merged-reply regression.** This provider streams audio between turns,
/// so a clock reading the output STREAM never runs out. One call's two answers
/// then arrived as a single row at the hangup.
///
/// Audio after the words must leave the bound exactly where the words set it.
#[test]
fn the_talkers_audio_never_pushes_its_own_turn_end_back() {
    let mut turn = TurnState::default();
    frame(talker_said("on it"), &mut turn);
    let armed = turn.last_words.expect("the words armed the bound");

    for _ in 0..20 {
        frame(talker_streamed(&[0, 0, 0, 0]), &mut turn);
    }

    assert_eq!(turn.last_words, Some(armed), "audio moved the bound");
}

/// A blank delta says nothing, so it arms nothing. Both providers forward one
/// exactly as it arrives, and a stream of them would hold the bound off for a
/// whole call.
#[test]
fn a_blank_talker_delta_arms_no_turn_end() {
    let mut turn = TurnState::default();
    for _ in 0..20 {
        frame(talker_said(""), &mut turn);
    }

    assert!(turn.last_words.is_none(), "a blank delta armed the bound");
    assert!(talker_went_quiet(&mut turn).is_empty());
}

/// One end per turn. A second idle with nothing said would write a reply the
/// caller never heard.
#[test]
fn a_turn_ends_once_however_often_the_stream_is_quiet() {
    let mut turn = TurnState::default();
    frame(talker_said("hello"), &mut turn);
    assert_eq!(talker_went_quiet(&mut turn).len(), 1);
    assert!(talker_went_quiet(&mut turn).is_empty());
}

/// Usage is zero because this provider reports no tokens. Its own duration
/// figures are cumulative snapshots, so summing them per turn would overstate
/// every call.
#[test]
fn a_talker_turn_reports_no_tokens_rather_than_invented_ones() {
    let mut turn = TurnState::default();
    frame(talker_said("done"), &mut turn);
    let VoiceEvent::TalkerTurnEnded { usage, .. } = talker_went_quiet(&mut turn).remove(0) else {
        panic!("the turn did not end");
    };
    assert_eq!(usage, ApiUsage::default());
    assert!(usage.modality.is_none());
}

// The caller's words.

/// Deltas are forwarded as PARTIALS and held as well. The partial draws the
/// caller's bubble as they speak; the held copy is what a delegation hands over.
#[test]
fn caller_deltas_are_forwarded_as_partials_and_still_held() {
    let mut turn = TurnState::default();
    assert_eq!(
        frame(caller_said("what is "), &mut turn),
        vec![VoiceEvent::UserTranscript {
            text: "what is ".to_string()
        }]
    );
    assert_eq!(
        frame(caller_said("on today"), &mut turn),
        vec![VoiceEvent::UserTranscript {
            text: "on today".to_string()
        }]
    );
    // Still whole when something finally asks for it, so the partial path
    // costs the delegation nothing.
    assert_eq!(
        caller_finished(&mut turn, None),
        vec![VoiceEvent::UserTurnEnded {
            transcript: "what is on today".to_string()
        }]
    );
}

/// A partial with nothing in it captions nothing, so it is not sent.
#[test]
fn an_empty_caller_delta_reaches_nobody() {
    let mut turn = TurnState::default();
    assert!(frame(caller_said(""), &mut turn).is_empty());
}

/// **The seven-bubble regression.** The talker's own output stream has holes in
/// it, and `TALKER_IDLE` closes its turn on every one. The frame that resumes
/// the stream must not be read as the talker answering: it said no words, so
/// the caller has not been judged finished by anybody.
#[test]
fn a_hole_in_the_talkers_audio_never_cuts_the_callers_sentence() {
    let mut turn = TurnState::default();
    let audio = talker_streamed(&[]);

    frame(caller_said("Why "), &mut turn);
    for _ in 0..7 {
        frame(audio.clone(), &mut turn);
        // The stream went quiet, exactly as it did on the reported call.
        talker_went_quiet(&mut turn);
        frame(caller_said("didn't you "), &mut turn);
        let resumed = frame(audio.clone(), &mut turn);
        assert!(
            !resumed
                .iter()
                .any(|e| matches!(e, VoiceEvent::UserTurnEnded { .. })),
            "an audio frame ended the caller's turn: {:?}",
            resumed
        );
    }
    // One sentence, still whole, still waiting for a real boundary.
    let VoiceEvent::UserTurnEnded { transcript } = caller_finished(&mut turn, None).remove(0)
    else {
        panic!("the caller's words were not held");
    };
    assert!(transcript.starts_with("Why didn't you"));
    assert_eq!(transcript.matches("didn't you").count(), 7);
}

/// **A hole in the MIDDLE of one answer is not a new answer.** `TALKER_IDLE` is
/// 700 ms, so a 900 ms gap ends the talker's turn and empties its accumulator.
/// The caller said nothing in that gap, so nothing of theirs may be handed over
/// a second time.
///
/// This is why the boundary reads the CALLER's accumulator and nothing else.
/// Gating on the talker's turn being fresh would cut here, which is the same
/// split at a higher threshold rather than the split removed.
#[test]
fn a_nine_hundred_millisecond_hole_in_one_answer_never_cuts_the_caller() {
    let mut turn = TurnState::default();
    frame(
        caller_said("why didn't you tell me to restart the computer"),
        &mut turn,
    );

    // The answer opens, and THAT is the caller's boundary.
    let opened = frame(talker_said("Because "), &mut turn);
    assert_eq!(
        opened[0],
        VoiceEvent::UserTurnEnded {
            transcript: "why didn't you tell me to restart the computer".to_string()
        }
    );

    // 900 ms with no output of any kind, so the reader ends the turn.
    assert_eq!(talker_went_quiet(&mut turn).len(), 1);
    assert!(
        turn.talker_words.is_empty(),
        "the accumulator survived the hole"
    );

    // The rest of the same answer, and not one word of the caller's with it.
    let resumed = frame(talker_said("the update needed it"), &mut turn);
    assert_eq!(
        resumed,
        vec![VoiceEvent::TalkerTranscript {
            text: "the update needed it".to_string()
        }]
    );
}

/// The caller speaking INTO that hole is a real boundary, and exactly one.
///
/// What they said is handed over whole, and nothing of the sentence before it
/// comes back: that one was already spent when the answer opened.
#[test]
fn a_caller_who_speaks_into_the_hole_gets_one_boundary_for_it() {
    let mut turn = TurnState::default();
    frame(caller_said("what is on today"), &mut turn);
    frame(talker_said("Two "), &mut turn);
    talker_went_quiet(&mut turn);

    frame(caller_said("and tomorrow"), &mut turn);
    let resumed = frame(talker_said("meetings"), &mut turn);

    assert_eq!(
        resumed[0],
        VoiceEvent::UserTurnEnded {
            transcript: "and tomorrow".to_string()
        }
    );
    assert_eq!(resumed.len(), 2, "{:?}", resumed);
}

/// A talker that streams audio and never a word ends no caller turn, and has
/// no turn of its own to end.
///
/// The honest reading: with no words there is no reply to write down, and
/// `call.rs` took no floor for one either.
#[test]
fn a_talker_that_only_streams_audio_has_no_turn_to_end() {
    let mut turn = TurnState::default();
    frame(talker_streamed(&[9]), &mut turn);

    assert!(talker_went_quiet(&mut turn).is_empty());
}

/// The talker answering IS its judgment that the caller stopped. No timer
/// decides it, which is the parent plan's decision 11.
#[test]
fn the_talker_taking_the_floor_finishes_the_callers_turn() {
    let mut turn = TurnState::default();
    frame(caller_said("what is on today"), &mut turn);

    let events = frame(talker_said("checking"), &mut turn);
    assert_eq!(
        events[0],
        VoiceEvent::UserTurnEnded {
            transcript: "what is on today".to_string()
        }
    );
}

/// Only the FIRST delta of a turn finishes the caller's. Every later one would
/// otherwise hand over an empty utterance.
#[test]
fn only_the_start_of_a_talker_turn_finishes_the_callers() {
    let mut turn = TurnState::default();
    frame(caller_said("what is on today"), &mut turn);
    frame(talker_said("check"), &mut turn);

    let later = frame(talker_said("ing"), &mut turn);
    assert_eq!(
        later,
        vec![VoiceEvent::TalkerTranscript {
            text: "ing".to_string()
        }]
    );
}

/// A talker turn with nothing said before it hands over no utterance. A blank
/// row would claim the caller spoke when they did not.
#[test]
fn a_talker_turn_with_no_caller_words_hands_over_nothing() {
    let mut turn = TurnState::default();
    let events = frame(talker_said("hello"), &mut turn);
    assert_eq!(
        events,
        vec![VoiceEvent::TalkerTranscript {
            text: "hello".to_string()
        }]
    );
}

/// Whatever ended the call, the caller said what they said. Dropping it would
/// lose a sentence from the thread for good.
#[test]
fn the_socket_closing_still_hands_over_what_the_caller_said() {
    let mut turn = TurnState::default();
    frame(caller_said("book it for tuesday"), &mut turn);

    assert_eq!(
        closing_events(&mut turn),
        vec![VoiceEvent::UserTurnEnded {
            transcript: "book it for tuesday".to_string()
        }]
    );
}

/// A turn the talker was mid-way through is owed too, or its words reach no
/// transcript.
#[test]
fn the_socket_closing_ends_a_turn_the_talker_was_still_speaking() {
    let mut turn = TurnState::default();
    frame(caller_said("what is on today"), &mut turn);
    frame(talker_said("you have two"), &mut turn);

    assert_eq!(
        closing_events(&mut turn),
        vec![VoiceEvent::TalkerTurnEnded {
            transcript: "you have two".to_string(),
            usage: ApiUsage::default(),
        }]
    );
}

// Delegation, which is the whole of this protocol's tool surface.

/// The ask carries an id and no words. It hands the caller's words over as the
/// utterance, and asks for nothing in its own name.
///
/// **The reason is empty, and that is the fix.** It used to be the caller's
/// transcript, which the transcript then drew back at them as something the
/// talker said.
#[test]
fn a_delegation_hands_over_the_callers_words_and_then_asks() {
    let mut turn = TurnState::default();
    frame(caller_said("move my three o'clock"), &mut turn);

    let events = frame(
        serde_json::json!({
            "type": "session.delegation.created",
            "delegation": { "id": "item_9tA", "type": "delegation", "target": "client" }
        }),
        &mut turn,
    );
    assert_eq!(
        events,
        vec![
            VoiceEvent::UserTurnEnded {
                transcript: "move my three o'clock".to_string()
            },
            VoiceEvent::DelegationRequested {
                tool_call_id: "item_9tA".to_string(),
                reason: String::new(),
            },
        ]
    );
}

/// **The caller never hears their own sentence read back.** A Live delegation
/// composes no words, so it carries none, however long the question was.
///
/// `WorkDelegated` renders a reason under the talker's speaker label, and
/// `build.rs` writes no row for a blank one. That is what keeps the caller's
/// question out of the talker's mouth, and out of the doer's history twice.
#[test]
fn a_delegation_carries_no_reason_of_its_own() {
    for question in ["move my three o'clock", "", &"a".repeat(1_000)] {
        let mut turn = TurnState::default();
        if !question.is_empty() {
            frame(caller_said(question), &mut turn);
        }
        let events = frame(
            serde_json::json!({
                "type": "session.delegation.created",
                "delegation": { "id": "item_9tA" }
            }),
            &mut turn,
        );
        let asked = events
            .iter()
            .find_map(|event| match event {
                VoiceEvent::DelegationRequested { reason, .. } => Some(reason.clone()),
                _ => None,
            })
            .expect("nothing was delegated");
        assert_eq!(asked, "", "the ask spoke for the caller: {:?}", asked);
    }
}

/// An ask with nothing held still reaches the doer, and says so rather than
/// inventing a reason. Dropping it would lose the work the caller asked for.
#[test]
fn a_delegation_with_no_held_words_still_asks() {
    let mut turn = TurnState::default();
    let events = frame(
        serde_json::json!({
            "type": "session.delegation.created",
            "delegation": { "id": "item_9tA" }
        }),
        &mut turn,
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        VoiceEvent::DelegationRequested { tool_call_id, .. } if tool_call_id == "item_9tA"
    ));
}

/// An ask with no id can never be answered, so it is dropped with a line
/// saying so rather than acknowledged into nothing.
#[test]
fn a_delegation_with_no_id_is_dropped() {
    let mut turn = TurnState::default();
    let events = frame(
        serde_json::json!({ "type": "session.delegation.created", "delegation": {} }),
        &mut turn,
    );
    assert!(events.is_empty());
}

/// A long question reaches the doer whole. Only the LOG line is clipped, and a
/// turn started on half a question is a turn answering something else.
#[test]
fn a_long_question_is_handed_over_whole() {
    let mut turn = TurnState::default();
    let long = "a".repeat(super::super::READ_ALOUD_CHARS * 2);
    frame(caller_said(&long), &mut turn);

    let events = frame(
        serde_json::json!({
            "type": "session.delegation.created",
            "delegation": { "id": "item_9tA" }
        }),
        &mut turn,
    );
    let VoiceEvent::UserTurnEnded { transcript } = &events[0] else {
        panic!("the caller's words were not handed over");
    };
    assert_eq!(transcript.chars().count(), long.chars().count());
}

// Errors and the frames the seam has no word for.

#[test]
fn an_error_frame_ends_the_call_with_what_it_said() {
    let mut turn = TurnState::default();
    let events = frame(
        serde_json::json!({ "type": "error", "error": { "message": "no such model" } }),
        &mut turn,
    );
    assert_eq!(
        events,
        vec![VoiceEvent::Failed {
            message: "no such model".to_string()
        }]
    );
}

/// One frame carries two things, and only one of them is the session dying.
/// A refused append costs one note, and dropping a working call over it is the
/// worse failure.
#[test]
fn a_refused_command_is_logged_and_never_ends_the_call() {
    let mut turn = TurnState::default();
    let events = frame(
        serde_json::json!({
            "type": "error",
            "error": {
                "message": "content is too long",
                "client_event_id": "lucidos-4",
            }
        }),
        &mut turn,
    );
    assert!(events.is_empty(), "a refused append ended the call");
}

/// An error with nothing to say still ends the call, rather than leaving the
/// caller listening to silence.
#[test]
fn an_error_frame_with_no_message_still_says_something() {
    let mut turn = TurnState::default();
    let events = frame(serde_json::json!({ "type": "error" }), &mut turn);
    assert_eq!(
        events,
        vec![VoiceEvent::Failed {
            message: "the talker reported an error with no message".to_string()
        }]
    );
}

/// Most of what this protocol sends narrates its own state machine, and the
/// seam has no word for any of it.
///
/// `session.closed` is deliberately absent. It is the terminal frame, and
/// `read_frame` acts on it before this ever sees it.
#[test]
fn the_frames_the_seam_has_no_word_for_produce_nothing() {
    let mut turn = TurnState::default();
    for kind in [
        "session.started",
        "session.updated",
        "session.thinking.appended",
        "session.commentary.appended",
        "session.instructions.appended",
        "response.event",
    ] {
        let events = frame(serde_json::json!({ "type": kind }), &mut turn);
        assert!(events.is_empty(), "{} produced an event", kind);
    }
}

// The three ways a session ends.

fn text_frame(value: serde_json::Value) -> Option<Result<Message, WsError>> {
    Some(Ok(Message::Text(value.to_string())))
}

/// The terminal frame carries the final usage, so it arrives while the socket
/// is still open. Read as ordinary narration, the reader would wait on a socket
/// nothing more is coming down and the caller would hear silence.
#[test]
fn the_terminal_frame_ends_the_session_and_hands_over_what_is_held() {
    let mut turn = TurnState::default();
    read_frame(text_frame(caller_said("book it for tuesday")), &mut turn);

    let (events, over) = read_frame(
        text_frame(serde_json::json!({ "type": "session.closed" })),
        &mut turn,
    );
    assert!(over, "the terminal frame did not end the session");
    assert_eq!(
        events,
        vec![VoiceEvent::UserTurnEnded {
            transcript: "book it for tuesday".to_string()
        }]
    );
}

/// A socket that fails owes the same. It used to break without flushing, which
/// lost the caller's last sentence from the thread.
#[test]
fn a_failed_socket_ends_the_session_and_hands_over_what_is_held() {
    let mut turn = TurnState::default();
    read_frame(text_frame(caller_said("cancel that")), &mut turn);

    let (events, over) = read_frame(Some(Err(WsError::ConnectionClosed)), &mut turn);
    assert!(over);
    assert_eq!(
        events,
        vec![VoiceEvent::UserTurnEnded {
            transcript: "cancel that".to_string()
        }]
    );
}

/// A socket that simply goes is the third way, and the reader keeps reading
/// until one of the three lands.
#[test]
fn an_ordinary_frame_leaves_the_session_running() {
    let mut turn = TurnState::default();
    let (_, over) = read_frame(text_frame(talker_said("checking")), &mut turn);
    assert!(!over);

    let (_, gone) = read_frame(None, &mut turn);
    assert!(gone);
}

/// A frame with no type is not a frame this maps. It must not panic either.
#[test]
fn an_untyped_frame_produces_nothing() {
    let mut turn = TurnState::default();
    assert!(frame(serde_json::json!({ "delta": "x" }), &mut turn).is_empty());
}

/// The one test that talks to the real provider. Only it can tell us the
/// opening payload above is still a shape the API accepts.
///
/// Ignored, because it needs a credential and a network. Run it deliberately:
///
/// ```text
/// cargo test -p lucidos-engine --lib voice::live -- --ignored --nocapture
/// ```
///
/// It opens a session and listens. A pass means the provider accepted the
/// opening frame and held the socket open. No audio, so the call costs a
/// handshake.
///
/// Skips itself with a printed line when no key is configured, rather than
/// failing: a machine with no OpenAI key is not a broken one.
#[tokio::test]
#[ignore]
async fn a_real_session_accepts_the_opening_payload() {
    crate::net_config::install_crypto_provider();
    let Ok(api_key) = std::env::var("OPENAI_API_KEY") else {
        println!("skipped: OPENAI_API_KEY is not set");
        return;
    };
    let model =
        std::env::var("LUCIDOS_VOICE_TALKER_MODEL").unwrap_or_else(|_| "gpt-live-1".to_string());
    println!("opening a session on {}", model);

    let provider = LiveProvider::new(api_key, model);
    let mut session = provider.open(opening()).await.expect("connect");

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, session.next()).await {
            Ok(Some(VoiceEvent::Failed { message })) => {
                panic!("the opening payload was refused: {}", message)
            }
            Ok(Some(other)) => println!("received {:?}", other),
            Ok(None) => panic!("the provider closed the socket"),
            Err(_) => break,
        }
    }
    session.close().await;
}

/// The engine taking the caller's words empties the reader's own copy.
///
/// Without it the next boundary hands the same sentence over again, and one
/// breath draws two rows. The session reaches the accumulator through the
/// handle it shares with the reader.
#[test]
fn words_the_engine_took_are_gone_from_the_reader() {
    let mut turn = TurnState::default();
    let shared = Arc::clone(&turn.caller_words);
    frame(caller_said("why didn't you tell me"), &mut turn);

    // What `LiveSession::caller_words_were_taken` does.
    shared.lock().expect("caller words").clear();

    assert!(caller_finished(&mut turn, None).is_empty());
    // And the next thing they say is still theirs.
    frame(caller_said("to restart it"), &mut turn);
    assert_eq!(
        caller_finished(&mut turn, None),
        vec![VoiceEvent::UserTurnEnded {
            transcript: "to restart it".to_string()
        }]
    );
}

// The session clock.

/// A caller fragment that says where it sits on the session timeline.
fn caller_said_at(text: &str, start_ms: i64) -> serde_json::Value {
    serde_json::json!({
        "type": "session.input_transcript.delta",
        "delta": text,
        "start_ms": start_ms,
        "end_ms": start_ms + 200,
    })
}

/// A talker fragment on the same clock. Its `start_ms` is the boundary.
fn talker_said_at(text: &str, start_ms: i64) -> serde_json::Value {
    serde_json::json!({
        "type": "session.output_transcript.delta",
        "delta": text,
        "start_ms": start_ms,
        "end_ms": start_ms + 200,
    })
}

fn a_delegation_at(offset_ms: i64) -> serde_json::Value {
    serde_json::json!({
        "type": "session.delegation.created",
        "offset_ms": offset_ms,
        "delegation": { "id": "item_9tA", "type": "delegation", "target": "client" }
    })
}

/// Pull the finished utterances out of what a frame produced.
fn utterances(events: &[VoiceEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            VoiceEvent::UserTurnEnded { transcript } => Some(transcript.clone()),
            _ => None,
        })
        .collect()
}

/// **The boundary is a point on the clock, not a moment of arrival.** Words the
/// caller said after the talker took the floor are them talking OVER the reply.
///
/// Swept into the turn the talker just answered, they read as part of a
/// question that was already put.
#[test]
fn what_they_said_after_the_boundary_is_not_part_of_the_turn_before_it() {
    let mut turn = TurnState::default();
    frame(caller_said_at("What's going on", 500), &mut turn);
    frame(caller_said_at(" never mind", 2_900), &mut turn);

    let cut = frame(talker_said_at("On", 2_500), &mut turn);
    assert_eq!(utterances(&cut), vec!["What's going on".to_string()]);

    // Still theirs, and still owed a turn of its own.
    assert_eq!(
        utterances(&closing_events(&mut turn)),
        vec!["never mind".to_string()]
    );
}

/// Arrival order is not timeline order, so the row is assembled by the clock.
#[test]
fn the_pieces_are_joined_in_the_order_they_were_spoken() {
    let mut turn = TurnState::default();
    frame(caller_said_at("the workspace ", 900), &mut turn);
    frame(caller_said_at("What is in ", 400), &mut turn);
    frame(caller_said_at("doing", 1_400), &mut turn);

    assert_eq!(
        caller_finished(&mut turn, None),
        vec![VoiceEvent::UserTurnEnded {
            transcript: "What is in the workspace doing".to_string()
        }]
    );
}

/// A stream that times some pieces and not others is joined as it arrived.
///
/// An untimed piece sorts ahead of every timed one. Sorting a mixed set would
/// move a word from the middle of a sentence to its front.
#[test]
fn a_piece_with_no_timing_does_not_jump_to_the_front() {
    let mut turn = TurnState::default();
    frame(caller_said_at("What is ", 400), &mut turn);
    frame(caller_said("the workspace "), &mut turn);
    frame(caller_said_at("doing", 1_400), &mut turn);

    assert_eq!(
        caller_finished(&mut turn, None),
        vec![VoiceEvent::UserTurnEnded {
            transcript: "What is the workspace doing".to_string()
        }]
    );
}

/// An ask cuts on its own place in the session, for the same reason a spoken
/// word does.
#[test]
fn an_ask_cuts_the_caller_off_where_the_frame_says_it_landed() {
    let mut turn = TurnState::default();
    frame(caller_said_at("move my three o'clock", 500), &mut turn);
    frame(caller_said_at(" and book a car", 4_500), &mut turn);

    assert_eq!(
        frame(a_delegation_at(4_000), &mut turn),
        vec![
            VoiceEvent::UserTurnEnded {
                transcript: "move my three o'clock".to_string()
            },
            VoiceEvent::DelegationRequested {
                tool_call_id: "item_9tA".to_string(),
                reason: String::new(),
            },
        ]
    );
}

/// The socket going takes everything, boundary or not. A piece held back for a
/// turn that will never come is a piece lost for good.
#[test]
fn the_closing_socket_takes_every_piece_that_is_left() {
    let mut turn = TurnState::default();
    frame(caller_said_at("What's going on", 500), &mut turn);
    frame(talker_said_at("On", 2_500), &mut turn);
    frame(caller_said_at("and what is next", 3_200), &mut turn);

    assert_eq!(
        utterances(&closing_events(&mut turn)),
        vec!["and what is next".to_string()]
    );
}

/// **The caller's words lead the reply they triggered, in one batch.**
///
/// `call.rs` reads a finished utterance as a MOVE of the conversation: it
/// closes the talker's row and writes the caller's. Released any later, it cuts
/// the reply it is the question for in half (ADR 0188, ADR 0198).
#[test]
fn the_callers_words_reach_the_seam_ahead_of_the_reply_they_triggered() {
    let mut turn = TurnState::default();
    frame(caller_said_at("What's going on", 500), &mut turn);

    let batch = frame(talker_said_at("On", 2_500), &mut turn);

    assert_eq!(
        batch,
        vec![
            VoiceEvent::UserTurnEnded {
                transcript: "What's going on".to_string()
            },
            VoiceEvent::TalkerTranscript {
                text: "On".to_string()
            },
        ]
    );
}
