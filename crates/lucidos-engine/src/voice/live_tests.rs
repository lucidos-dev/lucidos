use base64::Engine as _;

use super::*;
use crate::voice::provider::AudioFormat;

fn opening() -> SessionOpening {
    SessionOpening {
        instructions: "You are Lucidos.".to_string(),
        resident_block: "[WHAT YOU ALREADY KNOW]\nWorkspace: dev".to_string(),
        voice: "marin".to_string(),
        transcriber: "gpt-transcribe".to_string(),
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

#[test]
fn the_opening_frame_names_the_model_and_starts_a_session() {
    let start = session_start("gpt-live-1", &opening());
    assert_eq!(start["type"], "session.start");
    assert_eq!(start["session"]["model"], "gpt-live-1");
}

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

#[test]
fn a_tool_less_talker_is_told_a_working_note_is_no_cue_to_speak() {
    let start = session_start("gpt-live-1", &opening());
    let instructions = start["session"]["instructions"]
        .as_str()
        .expect("instructions are a string");
    assert!(
        instructions.contains(&format!("A note that starts with {}", WORKING_NOTE_TAG)),
        "the note is never named: {}",
        instructions
    );
    assert!(
        instructions.contains("never a cue to speak"),
        "nothing keeps the talker quiet on a note: {}",
        instructions
    );
}

#[test]
fn a_tool_less_talker_is_told_the_call_opens_listening() {
    let start = session_start("gpt-live-1", &opening());
    let instructions = start["session"]["instructions"]
        .as_str()
        .expect("instructions are a string");
    let earlier_note = format!("A note that starts with {} is for you alone", EARLIER_TAG);
    for fact in [
        "the caller has not spoken yet",
        "Say nothing until they do",
        "recap of earlier talk only when they ask",
        earlier_note.as_str(),
    ] {
        assert!(
            instructions.contains(fact),
            "{} is missing: {}",
            fact,
            instructions
        );
    }
}

#[test]
fn a_tool_less_talker_leaves_an_unfinished_sentence_in_silence() {
    let start = session_start("gpt-live-1", &opening());
    let instructions = start["session"]["instructions"]
        .as_str()
        .expect("instructions are a string");
    assert!(
        instructions.contains("Backchannel policy: None."),
        "the provider's label is missing: {}",
        instructions
    );
    assert!(
        instructions.contains("An unfinished sentence gets silence, never a filler"),
        "nothing keeps the talker out of a pause: {}",
        instructions
    );
}

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

#[test]
fn the_opening_frame_configures_no_transcriber_and_no_language() {
    let mut opening = opening();
    opening.transcriber = "gpt-transcribe".to_string();
    opening.language = Some(crate::voice::language::SpokenLanguage {
        code: Some("nb".to_string()),
        name: "Norwegian".to_string(),
    });
    let start = session_start("gpt-live-1", &opening).to_string();
    for absent in ["transcription", "gpt-transcribe", "\"nb\""] {
        assert!(
            !start.contains(absent),
            "{} reached the opening frame",
            absent
        );
    }
}

#[test]
fn the_audio_format_and_voice_come_from_the_opening() {
    let start = session_start("gpt-live-1", &opening());
    let audio = &start["session"]["audio"];
    assert_eq!(audio["format"]["type"], "audio/pcm");
    assert_eq!(audio["format"]["rate"], 24_000);
    assert_eq!(audio["output"]["voice"], "marin");
}

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

#[test]
fn every_append_names_itself() {
    let frame = append_frame(COMMENTARY, "lucidos-7", None, "the order shipped");
    assert_eq!(frame["event_id"], "lucidos-7");
}

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

#[test]
fn a_long_block_opens_as_several_quiet_appends() {
    let frames = opening_appends(&"alpha bravo ".repeat(400));
    assert!(frames.len() > 1, "a 4800-char block stayed one append");
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(frame["type"], THINKING);
        assert_eq!(frame["event_id"], format!("resident-{}", index));
    }
}

#[test]
fn each_append_kind_is_the_one_its_seam_member_promises() {
    assert_eq!(append_frame(COMMENTARY, "e", None, "x")["type"], COMMENTARY);
    assert_eq!(append_frame(THINKING, "e", None, "x")["type"], THINKING);
}

#[test]
fn a_short_note_is_one_chunk_and_keeps_every_character() {
    assert_eq!(
        chunks("the order shipped today"),
        vec!["the order shipped today"]
    );
}

#[test]
fn an_empty_note_still_yields_one_chunk() {
    assert_eq!(chunks(""), vec![""]);
}

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

#[test]
fn a_long_answer_of_multibyte_text_never_splits_a_character() {
    let long = "é".repeat(APPEND_CHARS * 3);
    let pieces = chunks(&long);
    assert!(pieces.len() > 1);
    let rejoined: String = pieces.concat();
    assert_eq!(rejoined.chars().count(), long.chars().count());
    assert!(rejoined.chars().all(|c| c == 'é'));
}

#[test]
fn talker_audio_is_decoded_and_forwarded() {
    let mut turn = TurnState::default();
    let events = frame(talker_streamed(&[1, 2, 3]), &mut turn);
    assert_eq!(events, vec![VoiceEvent::Audio(vec![1, 2, 3])]);
}

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

#[test]
fn a_blank_talker_delta_arms_no_turn_end() {
    let mut turn = TurnState::default();
    for _ in 0..20 {
        frame(talker_said(""), &mut turn);
    }

    assert!(turn.last_words.is_none(), "a blank delta armed the bound");
    assert!(talker_went_quiet(&mut turn).is_empty());
}

#[test]
fn a_turn_ends_once_however_often_the_stream_is_quiet() {
    let mut turn = TurnState::default();
    frame(talker_said("hello"), &mut turn);
    assert_eq!(talker_went_quiet(&mut turn).len(), 1);
    assert!(talker_went_quiet(&mut turn).is_empty());
}

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
    assert_eq!(
        caller_finished(&mut turn, None),
        vec![VoiceEvent::UserTurnEnded {
            transcript: "what is on today".to_string()
        }]
    );
}

#[test]
fn an_empty_caller_delta_reaches_nobody() {
    let mut turn = TurnState::default();
    assert!(frame(caller_said(""), &mut turn).is_empty());
}

#[test]
fn a_hole_in_the_talkers_audio_never_cuts_the_callers_sentence() {
    let mut turn = TurnState::default();
    let audio = talker_streamed(&[]);

    frame(caller_said("Why "), &mut turn);
    for _ in 0..7 {
        frame(audio.clone(), &mut turn);
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
    let VoiceEvent::UserTurnEnded { transcript } = caller_finished(&mut turn, None).remove(0)
    else {
        panic!("the caller's words were not held");
    };
    assert!(transcript.starts_with("Why didn't you"));
    assert_eq!(transcript.matches("didn't you").count(), 7);
}

#[test]
fn a_nine_hundred_millisecond_hole_in_one_answer_never_cuts_the_caller() {
    let mut turn = TurnState::default();
    frame(
        caller_said("why didn't you tell me to restart the computer"),
        &mut turn,
    );

    let opened = frame(talker_said("Because "), &mut turn);
    assert_eq!(
        opened[0],
        VoiceEvent::UserTurnEnded {
            transcript: "why didn't you tell me to restart the computer".to_string()
        }
    );

    assert_eq!(talker_went_quiet(&mut turn).len(), 1);
    assert!(
        turn.talker_words.is_empty(),
        "the accumulator survived the hole"
    );

    let resumed = frame(talker_said("the update needed it"), &mut turn);
    assert_eq!(
        resumed,
        vec![VoiceEvent::TalkerTranscript {
            text: "the update needed it".to_string()
        }]
    );
}

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

#[test]
fn a_talker_that_only_streams_audio_has_no_turn_to_end() {
    let mut turn = TurnState::default();
    frame(talker_streamed(&[9]), &mut turn);

    assert!(talker_went_quiet(&mut turn).is_empty());
}

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

#[test]
fn a_delegation_with_no_id_is_dropped() {
    let mut turn = TurnState::default();
    let events = frame(
        serde_json::json!({ "type": "session.delegation.created", "delegation": {} }),
        &mut turn,
    );
    assert!(events.is_empty());
}

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

fn text_frame(value: serde_json::Value) -> Option<Result<Message, WsError>> {
    Some(Ok(Message::Text(value.to_string())))
}

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

#[test]
fn an_ordinary_frame_leaves_the_session_running() {
    let mut turn = TurnState::default();
    let (_, over) = read_frame(text_frame(talker_said("checking")), &mut turn);
    assert!(!over);

    let (_, gone) = read_frame(None, &mut turn);
    assert!(gone);
}

#[test]
fn an_untyped_frame_produces_nothing() {
    let mut turn = TurnState::default();
    assert!(frame(serde_json::json!({ "delta": "x" }), &mut turn).is_empty());
}

#[tokio::test]
#[ignore]
async fn a_real_session_accepts_the_opening_payload() {
    crate::net_config::install_crypto_provider();
    let Ok(api_key) = std::env::var("OPENAI_API_KEY") else {
        crate::log_legacy!("skipped: OPENAI_API_KEY is not set");
        return;
    };
    let model =
        std::env::var("LUCIDOS_VOICE_TALKER_MODEL").unwrap_or_else(|_| "gpt-live-1".to_string());
    crate::log_legacy!("opening a session on {}", model);

    let provider = LiveProvider::new(api_key, model);
    let mut session = provider.open(opening()).await.expect("connect");

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, session.next()).await {
            Ok(Some(VoiceEvent::Failed { message })) => {
                panic!("the opening payload was refused: {}", message)
            }
            Ok(Some(other)) => crate::log_legacy!("received {:?}", other),
            Ok(None) => panic!("the provider closed the socket"),
            Err(_) => break,
        }
    }
    session.close().await;
}

#[test]
fn words_the_engine_took_are_gone_from_the_reader() {
    let mut turn = TurnState::default();
    let shared = Arc::clone(&turn.caller_words);
    frame(caller_said("why didn't you tell me"), &mut turn);

    shared.lock().expect("caller words").clear();

    assert!(caller_finished(&mut turn, None).is_empty());
    frame(caller_said("to restart it"), &mut turn);
    assert_eq!(
        caller_finished(&mut turn, None),
        vec![VoiceEvent::UserTurnEnded {
            transcript: "to restart it".to_string()
        }]
    );
}

fn caller_said_at(text: &str, start_ms: i64) -> serde_json::Value {
    serde_json::json!({
        "type": "session.input_transcript.delta",
        "delta": text,
        "start_ms": start_ms,
        "end_ms": start_ms + 200,
    })
}

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

fn utterances(events: &[VoiceEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            VoiceEvent::UserTurnEnded { transcript } => Some(transcript.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn what_they_said_after_the_boundary_is_not_part_of_the_turn_before_it() {
    let mut turn = TurnState::default();
    frame(caller_said_at("What's going on", 500), &mut turn);
    frame(caller_said_at(" never mind", 2_900), &mut turn);

    let cut = frame(talker_said_at("On", 2_500), &mut turn);
    assert_eq!(utterances(&cut), vec!["What's going on".to_string()]);

    assert_eq!(
        utterances(&closing_events(&mut turn)),
        vec!["never mind".to_string()]
    );
}

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

fn continuations(events: &[VoiceEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            VoiceEvent::UserTurnContinued { transcript } => Some(transcript.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_rest_of_an_asked_sentence_continues_the_ask() {
    let mut turn = TurnState::default();
    frame(
        caller_said_at("when do you estimate that it's", 500),
        &mut turn,
    );

    let asked = frame(a_delegation_at(2_000), &mut turn);
    assert!(matches!(
        asked.last(),
        Some(VoiceEvent::DelegationRequested { .. })
    ));
    assert_eq!(
        utterances(&asked),
        vec!["when do you estimate that it's".to_string()]
    );

    frame(caller_said_at(" finished", 2_100), &mut turn);
    let reply = frame(talker_said_at("Hmm.", 2_300), &mut turn);
    assert_eq!(
        reply,
        vec![
            VoiceEvent::UserTurnContinued {
                transcript: "finished".to_string()
            },
            VoiceEvent::TalkerTranscript {
                text: "Hmm.".to_string()
            },
        ]
    );
}

#[test]
fn a_tail_transcribed_after_the_reply_began_still_continues_the_ask() {
    let mut turn = TurnState::default();
    frame(caller_said_at("is this in the latest", 500), &mut turn);
    frame(a_delegation_at(2_000), &mut turn);
    frame(talker_said_at("Good", 2_300), &mut turn);

    frame(caller_said_at(" release", 1_900), &mut turn);
    let next = frame(talker_said_at(" question.", 2_600), &mut turn);
    assert_eq!(continuations(&next), vec!["release".to_string()]);
    assert!(utterances(&next).is_empty(), "{:?}", next);
}

#[test]
fn words_said_over_the_reply_to_an_ask_do_not_continue_it() {
    let mut turn = TurnState::default();
    frame(caller_said_at("move my three o'clock", 500), &mut turn);
    frame(a_delegation_at(2_000), &mut turn);
    frame(talker_said_at("Sure", 2_300), &mut turn);

    frame(caller_said_at(" never mind", 2_500), &mut turn);
    let next = frame(talker_said_at(" thing.", 2_900), &mut turn);
    assert!(continuations(&next).is_empty(), "{:?}", next);
    assert_eq!(utterances(&next), vec!["never mind".to_string()]);
}

#[test]
fn a_late_piece_after_a_reply_with_no_ask_continues_nothing() {
    let mut turn = TurnState::default();
    frame(caller_said_at("is this in the latest", 500), &mut turn);
    frame(talker_said_at("Yes", 2_000), &mut turn);

    frame(caller_said_at(" release", 1_800), &mut turn);
    let next = frame(talker_said_at(" it is.", 2_400), &mut turn);
    assert!(continuations(&next).is_empty(), "{:?}", next);
    assert_eq!(utterances(&next), vec!["release".to_string()]);
}

#[test]
fn an_ask_made_mid_reply_ends_its_sentence_where_the_reply_began() {
    let mut turn = TurnState::default();
    frame(caller_said_at("when is it done", 500), &mut turn);
    frame(talker_said_at("Let", 2_000), &mut turn);
    frame(a_delegation_at(2_400), &mut turn);

    frame(caller_said_at(" today", 1_900), &mut turn);
    frame(caller_said_at(" thanks", 2_600), &mut turn);
    let next = frame(talker_said_at(" me check.", 2_800), &mut turn);
    assert_eq!(continuations(&next), vec!["today".to_string()]);
    assert_eq!(utterances(&next), vec!["thanks".to_string()]);
}

#[test]
fn the_closing_socket_still_hands_over_the_rest_of_an_asked_sentence() {
    let mut turn = TurnState::default();
    frame(caller_said_at("cancel the", 500), &mut turn);
    frame(a_delegation_at(1_500), &mut turn);
    frame(caller_said_at(" other one", 1_600), &mut turn);

    assert_eq!(
        continuations(&closing_events(&mut turn)),
        vec!["other one".to_string()]
    );
}

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

#[test]
fn a_silent_talker_does_not_keep_an_asked_sentence_open_forever() {
    for start in [3_999, 4_000, 20_000] {
        let mut turn = TurnState::default();
        frame(caller_said_at("check the build", 500), &mut turn);
        frame(a_delegation_at(2_000), &mut turn);
        frame(caller_said_at("and the tests", start), &mut turn);
        let events = frame(talker_said_at("Checking.", 21_000), &mut turn);
        if start < 4_000 {
            assert_eq!(continuations(&events), ["and the tests"]);
            assert!(utterances(&events).is_empty());
        } else {
            assert!(continuations(&events).is_empty());
            assert_eq!(utterances(&events), ["and the tests"]);
        }
    }
}

#[test]
fn a_late_transcript_cannot_extend_the_bound_after_the_talker_replies() {
    let mut turn = TurnState::default();
    frame(caller_said_at("check the build", 500), &mut turn);
    frame(a_delegation_at(2_000), &mut turn);
    frame(talker_said_at("Checking.", 20_000), &mut turn);
    frame(caller_said_at("another request", 10_000), &mut turn);
    let events = closing_events(&mut turn);
    assert!(continuations(&events).is_empty());
    assert_eq!(utterances(&events), ["another request"]);
}

#[test]
fn unplaced_asks_and_untimed_pieces_continue_nothing() {
    for timed_ask in [false, true] {
        let mut turn = TurnState::default();
        frame(caller_said_at("check the build", 500), &mut turn);
        let mut ask = a_delegation_at(2_000);
        if !timed_ask {
            ask.as_object_mut().unwrap().remove("offset_ms");
        }
        frame(ask, &mut turn);
        let piece = if timed_ask {
            caller_said("another request")
        } else {
            caller_said_at("another request", 2_100)
        };
        frame(piece, &mut turn);
        let events = closing_events(&mut turn);
        assert!(continuations(&events).is_empty());
        assert_eq!(utterances(&events), ["another request"]);
    }
}
