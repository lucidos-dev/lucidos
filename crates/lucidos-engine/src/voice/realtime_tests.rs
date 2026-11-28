use super::*;
use crate::voice::provider::AudioFormat;

fn opening() -> SessionOpening {
    SessionOpening {
        instructions: "You are Lucidos.".to_string(),
        resident_block: "[WHAT YOU ALREADY KNOW]\nWorkspace: dev".to_string(),
        voice: "marin".to_string(),
        transcriber: "gpt-realtime-whisper".to_string(),
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

#[test]
fn a_resolved_language_pins_the_transcriber() {
    assert_eq!(transcription_of(&speaking("Norwegian"))["language"], "no");
    assert_eq!(transcription_of(&speaking("Nynorsk"))["language"], "nn");
}

#[test]
fn the_configured_transcriber_is_the_one_asked_for() {
    let picked = SessionOpening {
        transcriber: "gpt-live-transcribe".to_string(),
        ..opening()
    };
    assert_eq!(transcription_of(&picked)["model"], "gpt-live-transcribe");
}

#[test]
fn an_unnamed_transcriber_falls_back_rather_than_going_mute() {
    for blank in ["", "   "] {
        let unnamed = SessionOpening {
            transcriber: blank.to_string(),
            ..opening()
        };
        assert_eq!(
            transcription_of(&unnamed)["model"],
            crate::core::prefs::MODEL_VOICE_TRANSCRIBER.default_text()
        );
    }
}

#[test]
fn an_unresolved_language_leaves_the_transcriber_alone() {
    let untouched = transcription_of(&opening());
    assert_eq!(untouched.get("language"), None);
    assert_eq!(transcription_of(&speaking("Klingon")), untouched);
}

#[test]
fn the_current_transcribers_take_their_language_as_an_array() {
    for model in ["gpt-live-transcribe", "gpt-transcribe"] {
        let config = transcription_of(&transcribing(model, "Norwegian"));
        assert_eq!(config["languages"], serde_json::json!(["no"]), "{}", model);
        assert_eq!(config.get("language"), None, "{} {}", model, config);
    }
    assert!(LANGUAGES_ARRAY_TRANSCRIBERS
        .contains(&crate::core::prefs::MODEL_VOICE_TRANSCRIBER.default_text()));
}

#[test]
fn every_other_transcriber_keeps_the_singular_language() {
    for model in ["gpt-realtime-whisper", "some-new-transcriber"] {
        let config = transcription_of(&transcribing(model, "Norwegian"));
        assert_eq!(config["language"], "no", "{}", model);
        assert_eq!(config.get("languages"), None, "{}", model);
    }
}

#[test]
fn an_unresolved_language_writes_neither_key_whichever_model_transcribes() {
    for model in ["gpt-transcribe", "gpt-realtime-whisper"] {
        for preference in ["", "Klingon"] {
            let config = transcription_of(&transcribing(model, preference));
            assert_eq!(config.get("language"), None, "{} {}", model, preference);
            assert_eq!(config.get("languages"), None, "{} {}", model, preference);
        }
    }
}

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

#[test]
fn the_hangup_tool_takes_nothing() {
    let parameters = session_update(&opening())["session"]["tools"][2]["parameters"].clone();
    assert_eq!(parameters["required"], serde_json::json!([]));
    assert!(parameters["properties"]
        .as_object()
        .expect("properties")
        .is_empty());
}

#[test]
fn the_talker_chooses_whether_to_call_anything() {
    assert_eq!(session_update(&opening())["session"]["tool_choice"], "auto");
}

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

#[test]
fn a_tool_call_with_no_id_is_dropped() {
    let frame = serde_json::json!({
        "type": "response.function_call_arguments.done",
        "name": "delegate",
        "arguments": "{\"reason\":\"x\"}"
    });
    assert!(map_event(&frame).is_empty());
}

#[test]
fn end_of_turn_is_decided_semantically() {
    let payload = session_update(&opening());
    assert_eq!(
        payload["session"]["audio"]["input"]["turn_detection"]["type"],
        "semantic_vad"
    );
}

#[test]
fn the_provider_never_cuts_its_own_reply() {
    let payload = session_update(&opening());
    assert_eq!(
        payload["session"]["audio"]["input"]["turn_detection"]["interrupt_response"],
        false
    );
}

#[test]
fn the_instructions_reach_the_opening_payload() {
    let payload = session_update(&opening());
    assert_eq!(payload["session"]["instructions"], "You are Lucidos.");
}

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
            assert_eq!(usage.modality, None);
        }
        other => panic!("expected a finished turn, got {:?}", other),
    }
}

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

#[test]
fn a_caller_partial_becomes_one_transcript_event() {
    let frame = serde_json::json!({
        "type": "conversation.item.input_audio_transcription.delta",
        "delta": "what have I"
    });
    assert_eq!(
        map_event(&frame),
        vec![VoiceEvent::UserTranscript {
            text: "what have I".to_string()
        }]
    );
}

#[test]
fn a_partial_with_no_words_is_dropped() {
    let absent = serde_json::json!({
        "type": "conversation.item.input_audio_transcription.delta"
    });
    assert_eq!(map_event(&absent), vec![]);

    let empty = serde_json::json!({
        "type": "conversation.item.input_audio_transcription.delta",
        "delta": ""
    });
    assert_eq!(map_event(&empty), vec![]);
}

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

#[test]
fn a_cancel_that_found_no_reply_is_not_a_failure() {
    let frame = serde_json::json!({
        "type": "error",
        "event_id": "event_1",
        "error": {
            "type": "invalid_request_error",
            "code": "response_cancel_not_active",
            "message": "Cancellation failed: no active response found",
            "param": null,
            "event_id": null
        }
    });
    assert_eq!(map_event(&frame), vec![]);
}

#[test]
fn another_invalid_request_error_still_ends_the_call() {
    let frame = serde_json::json!({
        "type": "error",
        "error": {
            "type": "invalid_request_error",
            "code": "invalid_value",
            "message": "Invalid value: 'pcm17'"
        }
    });
    assert_eq!(
        map_event(&frame),
        vec![VoiceEvent::Failed {
            message: "Invalid value: 'pcm17'".to_string()
        }]
    );
}

#[test]
fn the_caller_starting_to_speak_is_its_own_event_and_not_an_interruption() {
    let frame = serde_json::json!({ "type": "input_audio_buffer.speech_started" });
    assert_eq!(map_event(&frame), vec![VoiceEvent::CallerStartedSpeaking]);
    assert!(!map_event(&frame).contains(&VoiceEvent::Interrupted));
}

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

#[test]
fn a_response_with_no_status_is_an_ordinary_turn() {
    let frame = serde_json::json!({ "type": "response.done", "response": {} });
    assert!(matches!(
        map_event(&frame).as_slice(),
        [VoiceEvent::TalkerTurnEnded { .. }]
    ));
}

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

#[tokio::test]
#[ignore]
async fn a_real_session_accepts_the_opening_payload() {
    crate::net_config::install_crypto_provider();
    let Ok(api_key) = std::env::var("OPENAI_API_KEY") else {
        crate::log_legacy!("skipped: OPENAI_API_KEY is not set");
        return;
    };
    let model = std::env::var("LUCIDOS_VOICE_TALKER_MODEL")
        .unwrap_or_else(|_| "gpt-realtime-2.1".to_string());
    crate::log_legacy!("opening a session on {}", model);

    let pinned = SessionOpening {
        transcriber: crate::core::prefs::MODEL_VOICE_TRANSCRIBER
            .default_text()
            .to_string(),
        ..speaking("Norwegian")
    };
    let provider = RealtimeProvider::new(api_key, model);
    let mut session = provider.open(pinned).await.expect("connect");

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
