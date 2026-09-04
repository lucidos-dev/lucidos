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

fn speaking(preference: &str) -> SessionOpening {
    SessionOpening {
        language: crate::voice::SpokenLanguage::resolve(preference),
        ..opening()
    }
}

/// One named transcriber, told one language. The pair the language key branches
/// on, so both halves have to vary together.
fn transcribing(model: &str, preference: &str) -> SessionOpening {
    SessionOpening {
        transcriber: model.to_string(),
        language: crate::voice::SpokenLanguage::resolve(preference),
        ..opening()
    }
}

fn transcription_of(opening: &SessionOpening) -> serde_json::Value {
    session_update(opening)["session"]["audio"]["input"]["transcription"].clone()
}

/// The defect this fixed. Left to guess, the transcriber picks a language per
/// utterance, and a Bokmål phrase came back as Nynorsk.
#[test]
fn a_resolved_language_pins_the_transcriber() {
    assert_eq!(transcription_of(&speaking("Norwegian"))["language"], "no");
    assert_eq!(transcription_of(&speaking("Nynorsk"))["language"], "nn");
}

/// The second model in the loop reaches the wire. Before this it was a const,
/// so a workspace could name a transcriber and still be transcribed by the old
/// one.
#[test]
fn the_configured_transcriber_is_the_one_asked_for() {
    let picked = SessionOpening {
        transcriber: "whisper-1".to_string(),
        ..opening()
    };
    assert_eq!(transcription_of(&picked)["model"], "whisper-1");
}

/// An opening that names none still transcribes. A mute call is the worse
/// outcome, and a blank row is a cleared field rather than a choice.
#[test]
fn an_unnamed_transcriber_falls_back_rather_than_going_mute() {
    for blank in ["", "   "] {
        let unnamed = SessionOpening {
            transcriber: blank.to_string(),
            ..opening()
        };
        assert_eq!(transcription_of(&unnamed)["model"], TRANSCRIBE_MODEL);
    }
}

/// A name nobody can map, and an unset preference, both leave the payload as
/// it was. A typo in Settings must not break a call.
#[test]
fn an_unresolved_language_leaves_the_transcriber_alone() {
    let untouched = transcription_of(&opening());
    assert_eq!(untouched.get("language"), None);
    assert_eq!(transcription_of(&speaking("Klingon")), untouched);
}

/// The live model spells the same pin as an array. A singular `language` here
/// is a field it does not read, so the call would go back to guessing.
#[test]
fn the_live_transcriber_takes_its_language_as_an_array() {
    let config = transcription_of(&transcribing(LIVE_TRANSCRIBE_MODEL, "Norwegian"));
    assert_eq!(config["languages"], serde_json::json!(["no"]));
    assert_eq!(config.get("language"), None, "{}", config);
}

/// Every other model keeps the singular key, unchanged. The provider refuses a
/// payload holding both, so neither branch may write the other one's key.
#[test]
fn every_other_transcriber_keeps_the_singular_language() {
    for model in [
        "gpt-transcribe",
        "gpt-4o-mini-transcribe",
        "gpt-4o-transcribe",
        "whisper-1",
    ] {
        let config = transcription_of(&transcribing(model, "Norwegian"));
        assert_eq!(config["language"], "no", "{}", model);
        assert_eq!(config.get("languages"), None, "{}", model);
    }
}

/// An unset Locale and a name nobody can map both pin nothing, whichever model
/// transcribes. An empty array would be a language set the provider cannot
/// satisfy, which is worse than letting it guess.
#[test]
fn an_unresolved_language_writes_neither_key_whichever_model_transcribes() {
    for model in [LIVE_TRANSCRIBE_MODEL, "gpt-4o-mini-transcribe"] {
        for preference in ["", "Klingon"] {
            let config = transcription_of(&transcribing(model, preference));
            assert_eq!(config.get("language"), None, "{} {}", model, preference);
            assert_eq!(config.get("languages"), None, "{} {}", model, preference);
        }
    }
}

/// The structural guarantee, checked on the wire. Three tools: one asks for
/// work, one answers what is waiting, one hangs up. A fourth entry here is a
/// talker that can act, which is the thing ADR 0149 exists to prevent.
#[test]
fn the_talker_is_opened_with_its_three_tools_and_no_more() {
    let payload = session_update(&opening());
    let tools = payload["session"]["tools"]
        .as_array()
        .expect("a tool list")
        .clone();
    let named: Vec<&str> = tools
        .iter()
        .map(|t| t["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(
        named,
        vec![
            crate::voice::DELEGATE_TOOL,
            crate::voice::ANSWER_TOOL,
            crate::voice::HANGUP_TOOL,
        ]
    );
    for tool in &tools {
        assert_eq!(tool["type"], "function");
    }
}

/// One required argument each, so neither call can land saying nothing. An
/// optional reason is an empty `WorkDelegated` row waiting to happen, and an
/// optional choice is an answer that settles nothing.
///
/// The property is checked BY NAME against `required`, not just counted. Under
/// `additionalProperties: false` a schema whose one property is named something
/// else than the one required leaves the model nothing valid to send.
#[test]
fn the_asking_and_answering_tools_each_take_one_required_argument() {
    let payload = session_update(&opening());
    for (index, arg) in [
        (0, crate::voice::DELEGATE_REASON_ARG),
        (1, crate::voice::ANSWER_CHOICE_ARG),
    ] {
        let parameters = payload["session"]["tools"][index]["parameters"].clone();
        assert_eq!(parameters["required"], serde_json::json!([arg]));
        let properties = parameters["properties"].as_object().expect("properties");
        assert_eq!(properties.len(), 1, "{:?}", properties);
        assert!(
            properties.contains_key(arg),
            "the one property is not {:?}: {:?}",
            arg,
            properties
        );
        assert_eq!(properties[arg]["type"], "string");
    }
}

/// Hanging up carries nothing to record beyond the fact of it, and an argument
/// a model must invent is one it will.
#[test]
fn the_hangup_tool_takes_nothing() {
    let parameters = session_update(&opening())["session"]["tools"][2]["parameters"].clone();
    assert_eq!(parameters["required"], serde_json::json!([]));
    assert!(parameters["properties"]
        .as_object()
        .expect("properties")
        .is_empty());
}

/// The talker decides, so the provider must be free to call any of them.
/// `required` would make every turn a tool call, which is the bug that
/// delegation replaced.
#[test]
fn the_talker_chooses_whether_to_call_anything() {
    assert_eq!(session_update(&opening())["session"]["tool_choice"], "auto");
}

/// The wake has to reach the engine while the talker is still speaking. This
/// frame is what carries it; `response.done` would cost a spoken turn.
#[test]
fn a_finished_tool_call_delegates_before_the_turn_ends() {
    let frame = serde_json::json!({
        "type": "response.function_call_arguments.done",
        "call_id": "call_abc",
        "name": "delegate",
        "arguments": "{\"reason\":\"they want this week's calendar\"}"
    });
    assert_eq!(
        map_event(&frame),
        vec![VoiceEvent::DelegationRequested {
            tool_call_id: "call_abc".to_string(),
            reason: "they want this week's calendar".to_string(),
        }]
    );
}

/// An answer carries the choice id through untouched. Nothing here reads it:
/// the engine looks it up against what is still open.
#[test]
fn an_answer_carries_the_choice_the_talker_handed_back() {
    let frame = serde_json::json!({
        "type": "response.function_call_arguments.done",
        "call_id": "call_abc",
        "name": "answer",
        "arguments": "{\"choice\":\"command:req-1#allow-once\"}"
    });
    assert_eq!(
        map_event(&frame),
        vec![VoiceEvent::AnswerRequested {
            tool_call_id: "call_abc".to_string(),
            choice_id: "command:req-1#allow-once".to_string(),
        }]
    );
}

/// A hangup carries only its own id, and that is the whole of it.
#[test]
fn a_hangup_carries_nothing_but_its_id() {
    let frame = serde_json::json!({
        "type": "response.function_call_arguments.done",
        "call_id": "call_abc",
        "name": "hang_up",
        "arguments": "{}"
    });
    assert_eq!(
        map_event(&frame),
        vec![VoiceEvent::HangupRequested {
            tool_call_id: "call_abc".to_string(),
        }]
    );
}

/// A reason the talker left out still delegates. The caller's question running
/// late beats it never running, and the row says the reason was missing.
#[test]
fn a_delegation_with_no_reason_still_goes_through() {
    for arguments in ["{}", "{\"reason\":\"  \"}", "not json"] {
        let frame = serde_json::json!({
            "type": "response.function_call_arguments.done",
            "call_id": "call_abc",
            "name": "delegate",
            "arguments": arguments
        });
        match map_event(&frame).as_slice() {
            [VoiceEvent::DelegationRequested { reason, .. }] => {
                assert_eq!(reason, "the talker gave no reason", "{}", arguments)
            }
            other => panic!("{} produced {:?}", arguments, other),
        }
    }
}

/// An answer naming no choice is dropped, not guessed at. The whole design is
/// that the engine never picks, and an id it invented would be a pick.
#[test]
fn an_answer_with_no_choice_settles_nothing() {
    for arguments in ["{}", "{\"choice\":\"  \"}", "not json"] {
        let frame = serde_json::json!({
            "type": "response.function_call_arguments.done",
            "call_id": "call_abc",
            "name": "answer",
            "arguments": arguments
        });
        assert!(map_event(&frame).is_empty(), "{}", arguments);
    }
}

/// A function call naming something else is a fourth tool appearing, and the
/// design says there are three. It is dropped rather than routed.
#[test]
fn a_call_to_a_tool_the_talker_does_not_hold_is_ignored() {
    let frame = serde_json::json!({
        "type": "response.function_call_arguments.done",
        "call_id": "call_abc",
        "name": "send_email",
        "arguments": "{\"reason\":\"x\"}"
    });
    assert!(map_event(&frame).is_empty());
}

/// A call with no id cannot be resolved, so it is dropped rather than left
/// dangling in the talker's history.
#[test]
fn a_tool_call_with_no_id_is_dropped() {
    let frame = serde_json::json!({
        "type": "response.function_call_arguments.done",
        "name": "delegate",
        "arguments": "{\"reason\":\"x\"}"
    });
    assert!(map_event(&frame).is_empty());
}

/// End of turn is semantic, never a silence timer. A pause mid-thought must
/// not read as a finished sentence.
#[test]
fn end_of_turn_is_decided_semantically() {
    let payload = session_update(&opening());
    assert_eq!(
        payload["session"]["audio"]["input"]["turn_detection"]["type"],
        "semantic_vad"
    );
}

/// The honesty constraint reaches the session, since it is what stops a
/// tool-less talker inventing an answer.
#[test]
fn the_instructions_reach_the_opening_payload() {
    let payload = session_update(&opening());
    assert_eq!(payload["session"]["instructions"], "You are Lucidos.");
}

/// The resident block is a history ITEM, not part of the instructions. That is
/// what lets a refresh append beside it instead of rewriting it.
#[test]
fn the_resident_block_opens_the_history_rather_than_the_instructions() {
    let payload = session_update(&opening());
    let instructions = payload["session"]["instructions"].as_str().unwrap();
    assert!(!instructions.contains("WHAT YOU ALREADY KNOW"));

    let item = history_item(&opening().resident_block);
    assert_eq!(item["type"], "conversation.item.create");
    assert_eq!(
        item["item"]["content"][0]["text"],
        opening().resident_block.as_str()
    );
}

/// The usage split the plan requires: a total that contains the cached count,
/// and no cache-write count, because this provider charges for none.
#[test]
fn a_finished_turn_reports_the_cached_and_fresh_split() {
    let done = serde_json::json!({
        "type": "response.done",
        "response": {
            "usage": {
                "input_tokens": 1200,
                "output_tokens": 64,
                "input_token_details": { "cached_tokens": 1024 }
            },
            "output": [{ "content": [{ "transcript": "I am checking." }] }]
        }
    });
    let events = map_event(&done);
    assert_eq!(events.len(), 1);
    match &events[0] {
        VoiceEvent::TalkerTurnEnded { transcript, usage } => {
            assert_eq!(transcript, "I am checking.");
            assert_eq!(usage.input_tokens, 1200);
            assert_eq!(usage.output_tokens, 64);
            assert_eq!(usage.cache_read_tokens, 1024);
            assert_eq!(usage.cache_creation_tokens, 0);
            // This frame carries no output detail, so it prices nothing.
            // `a_half_reported_split_is_no_split` states why that is `None`.
            assert_eq!(usage.modality, None);
        }
        other => panic!("expected a finished turn, got {:?}", other),
    }
}

/// A full frame, as the realtime API sends one. Every count has to land, since
/// audio bills at eight times the text input rate and a blended total cannot
/// be priced.
#[test]
fn a_finished_turn_reports_the_modality_split() {
    let done = serde_json::json!({
        "type": "response.done",
        "response": {
            "usage": {
                "total_tokens": 1264,
                "input_tokens": 1200,
                "output_tokens": 64,
                "input_token_details": {
                    "text_tokens": 176,
                    "audio_tokens": 1024,
                    "image_tokens": 0,
                    "cached_tokens": 1024,
                    "cached_tokens_details": {
                        "text_tokens": 100,
                        "audio_tokens": 924,
                        "image_tokens": 0
                    }
                },
                "output_token_details": { "text_tokens": 20, "audio_tokens": 44 }
            },
            "output": [{ "content": [{ "transcript": "I am checking." }] }]
        }
    });
    let usage = done_usage(&done);
    assert_eq!(usage.input_tokens, 1200);
    assert_eq!(usage.output_tokens, 64);
    assert_eq!(usage.cache_read_tokens, 1024);
    assert_eq!(
        usage.modality,
        Some(ModalityUsage {
            input_text_tokens: 176,
            input_audio_tokens: 1024,
            input_image_tokens: 0,
            cache_read_text_tokens: 100,
            cache_read_audio_tokens: 924,
            cache_read_image_tokens: 0,
            output_text_tokens: 20,
            output_audio_tokens: 44,
        })
    );
}

/// The three sums the breakdown promises. A consumer prices the parts and
/// reports the totals, so a split that does not add up double-bills the turn.
#[test]
fn the_parts_add_up_to_the_totals() {
    let done = serde_json::json!({
        "type": "response.done",
        "response": { "usage": {
            "input_tokens": 1200,
            "output_tokens": 64,
            "input_token_details": {
                "text_tokens": 176, "audio_tokens": 1024, "image_tokens": 0,
                "cached_tokens": 1024,
                "cached_tokens_details": { "text_tokens": 100, "audio_tokens": 924, "image_tokens": 0 }
            },
            "output_token_details": { "text_tokens": 20, "audio_tokens": 44 }
        }}
    });
    let usage = done_usage(&done);
    let m = usage.modality.expect("a full frame reports a split");
    assert_eq!(
        m.input_text_tokens + m.input_audio_tokens + m.input_image_tokens,
        usage.input_tokens
    );
    assert_eq!(
        m.cache_read_text_tokens + m.cache_read_audio_tokens + m.cache_read_image_tokens,
        usage.cache_read_tokens
    );
    assert_eq!(
        m.output_text_tokens + m.output_audio_tokens,
        usage.output_tokens
    );
}

/// No detail blocks at all. The four flat counts still parse, and the
/// breakdown is absent rather than zeroed: a struct of zeros would read as a
/// real turn that was all cache and spoke nothing.
#[test]
fn a_frame_with_no_detail_reports_no_split() {
    let done = serde_json::json!({
        "type": "response.done",
        "response": { "usage": { "input_tokens": 900, "output_tokens": 40 } }
    });
    let usage = done_usage(&done);
    assert_eq!(usage.input_tokens, 900);
    assert_eq!(usage.output_tokens, 40);
    assert_eq!(usage.cache_read_tokens, 0);
    assert_eq!(usage.cache_creation_tokens, 0);
    assert_eq!(usage.modality, None);
}

/// Input detail with no output detail. The breakdown is all-or-nothing, so the
/// input half is dropped too.
///
/// It is the safe direction. Half a breakdown reports zero spoken audio on a
/// turn that spoke, and the consumer cannot tell that zero from a real one. A
/// missing breakdown it can tell, and it falls back to the flat totals.
#[test]
fn a_half_reported_split_is_no_split() {
    let done = serde_json::json!({
        "type": "response.done",
        "response": { "usage": {
            "input_tokens": 1200,
            "output_tokens": 64,
            "input_token_details": {
                "text_tokens": 176, "audio_tokens": 1024, "cached_tokens": 1024
            }
        }}
    });
    let usage = done_usage(&done);
    assert_eq!(usage.input_tokens, 1200);
    assert_eq!(usage.cache_read_tokens, 1024);
    assert_eq!(usage.modality, None);
}

/// A provider whose parts do not sum to its totals. Both are stored exactly as
/// reported. Rescaling would invent numbers no frame carried, and it would
/// hide the drift from whoever has to fix it.
#[test]
fn a_split_that_disagrees_is_stored_unrescaled() {
    let done = serde_json::json!({
        "type": "response.done",
        "response": { "usage": {
            "input_tokens": 1200,
            "output_tokens": 64,
            "input_token_details": {
                "text_tokens": 100, "audio_tokens": 200, "cached_tokens": 0
            },
            "output_token_details": { "text_tokens": 1, "audio_tokens": 2 }
        }}
    });
    let usage = done_usage(&done);
    assert_eq!(usage.input_tokens, 1200, "the flat total wins");
    assert_eq!(usage.output_tokens, 64, "the flat total wins");
    let m = usage.modality.expect("a reported split is kept");
    assert_eq!(m.input_text_tokens, 100);
    assert_eq!(m.input_audio_tokens, 200);
    assert_eq!(m.output_text_tokens, 1);
    assert_eq!(m.output_audio_tokens, 2);
}

#[test]
fn audio_arrives_decoded() {
    let encoded = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3, 4]);
    let frame = serde_json::json!({
        "type": "response.output_audio.delta",
        "delta": encoded
    });
    assert_eq!(map_event(&frame), vec![VoiceEvent::Audio(vec![1, 2, 3, 4])]);
}

#[test]
fn a_finished_user_utterance_becomes_one_turn() {
    let frame = serde_json::json!({
        "type": "conversation.item.input_audio_transcription.completed",
        "transcript": "what have I got running"
    });
    assert_eq!(
        map_event(&frame),
        vec![VoiceEvent::UserTurnEnded {
            transcript: "what have I got running".to_string()
        }]
    );
}

/// A provider error must reach the caller as a failure rather than as silence.
#[test]
fn an_error_frame_names_what_went_wrong() {
    let frame = serde_json::json!({
        "type": "error",
        "error": { "message": "invalid_session" }
    });
    assert_eq!(
        map_event(&frame),
        vec![VoiceEvent::Failed {
            message: "invalid_session".to_string()
        }]
    );
}

/// A caller starting to speak is NOT an interruption. `speech_started` fires on
/// every utterance, including the first word of a call with nothing playing.
/// Mapping it would report the talker cut off on every turn.
#[test]
fn the_caller_starting_to_speak_is_not_an_interruption() {
    let frame = serde_json::json!({ "type": "input_audio_buffer.speech_started" });
    assert!(map_event(&frame).is_empty());
}

/// A cancelled response IS one. It still reports the turn, because the tokens
/// were spent whether or not anybody heard the words.
#[test]
fn a_cancelled_response_reports_the_interruption_and_still_bills_it() {
    let frame = serde_json::json!({
        "type": "response.done",
        "response": {
            "status": "cancelled",
            "usage": { "input_tokens": 300, "output_tokens": 8 }
        }
    });
    let events = map_event(&frame);
    assert_eq!(events.len(), 2, "{:?}", events);
    assert_eq!(events[0], VoiceEvent::Interrupted);
    match &events[1] {
        VoiceEvent::TalkerTurnEnded { usage, .. } => assert_eq!(usage.output_tokens, 8),
        other => panic!("expected the turn to still be reported, got {:?}", other),
    }
}

/// A failed response is not a quiet turn end. Reading it as one leaves the
/// caller listening to silence with nothing said.
#[test]
fn a_failed_response_is_a_failure_rather_than_a_turn() {
    let frame = serde_json::json!({
        "type": "response.done",
        "response": {
            "status": "failed",
            "status_details": { "error": { "message": "server_error" } }
        }
    });
    assert_eq!(
        map_event(&frame),
        vec![VoiceEvent::Failed {
            message: "server_error".to_string()
        }]
    );
}

/// A response with no status reads as completed. A provider that stops sending
/// the field then degrades to the ordinary turn rather than to a failure.
#[test]
fn a_response_with_no_status_is_an_ordinary_turn() {
    let frame = serde_json::json!({ "type": "response.done", "response": {} });
    assert!(matches!(
        map_event(&frame).as_slice(),
        [VoiceEvent::TalkerTurnEnded { .. }]
    ));
}

/// A realtime API narrates its own state machine, and the seam models a
/// conversation. Most frames therefore map to nothing, and must not be
/// forwarded as an empty event.
#[test]
fn a_frame_the_seam_has_no_word_for_maps_to_nothing() {
    for kind in [
        "session.created",
        "session.updated",
        "response.created",
        "rate_limits.updated",
    ] {
        let frame = serde_json::json!({ "type": kind });
        assert!(map_event(&frame).is_empty(), "{} produced an event", kind);
    }
}

/// The one test that talks to the real provider. Only it can tell us the
/// opening payload above is still a shape the API accepts.
///
/// Ignored, because it needs a credential and a network. Run it deliberately:
///
/// ```text
/// cargo test -p lucidos-engine --lib voice::realtime -- --ignored --nocapture
/// ```
///
/// It opens a session, sends both opening payloads, and listens. A pass means
/// the provider refused neither and held the socket open. A refusal arrives as
/// an `error` frame within a second, and an unknown field in either payload is
/// a refusal. No audio, so the call costs a handshake.
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
        std::env::var("LUCIDOS_VOICE_TALKER_MODEL").unwrap_or_else(|_| "gpt-realtime".to_string());
    println!("opening a session on {}", model);

    let provider = RealtimeProvider::new(api_key, model);
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

/// A call with no `name` is routed by its argument rather than dropped.
///
/// The code this replaced treated a missing name as a delegation, because
/// dropping the frame loses the caller's question outright. Three tools end
/// that assumption, so the two carrying a distinguishing required argument are
/// recovered from it instead.
#[test]
fn an_unnamed_call_is_recovered_from_the_argument_it_carries() {
    let delegating = serde_json::json!({
        "type": "response.function_call_arguments.done",
        "call_id": "call_abc",
        "arguments": "{\"reason\":\"they want this week's calendar\"}"
    });
    assert_eq!(
        map_event(&delegating),
        vec![VoiceEvent::DelegationRequested {
            tool_call_id: "call_abc".to_string(),
            reason: "they want this week's calendar".to_string(),
        }]
    );

    let answering = serde_json::json!({
        "type": "response.function_call_arguments.done",
        "call_id": "call_abc",
        "arguments": "{\"choice\":\"command:req-1#deny\"}"
    });
    assert_eq!(
        map_event(&answering),
        vec![VoiceEvent::AnswerRequested {
            tool_call_id: "call_abc".to_string(),
            choice_id: "command:req-1#deny".to_string(),
        }]
    );
}

/// Hanging up is NOT recovered from an unnamed call. It takes no argument, so
/// an empty or unreadable payload looks exactly like one. Guessing there would
/// end a call the caller never asked to end.
#[test]
fn an_unnamed_call_with_nothing_readable_is_dropped_rather_than_read_as_a_hangup() {
    for arguments in ["{}", "not json", "{\"something\":\"else\"}"] {
        let frame = serde_json::json!({
            "type": "response.function_call_arguments.done",
            "call_id": "call_abc",
            "arguments": arguments
        });
        assert!(map_event(&frame).is_empty(), "{}", arguments);
    }
}
