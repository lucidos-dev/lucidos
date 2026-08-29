use super::*;
use crate::voice::provider::AudioFormat;

fn opening() -> SessionOpening {
    SessionOpening {
        instructions: "You are Lucidos.".to_string(),
        resident_block: "[WHAT YOU ALREADY KNOW]\nWorkspace: dev".to_string(),
        voice: "marin".to_string(),
        audio: AudioFormat::default(),
    }
}

/// ADR 0149's structural guarantee, checked on the wire. `SessionOpening` has
/// no tool field, and the payload says so out loud as well.
#[test]
fn the_talker_is_opened_with_an_empty_tool_list() {
    let payload = session_update(&opening());
    assert_eq!(payload["session"]["tools"], serde_json::json!([]));
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
        }
        other => panic!("expected a finished turn, got {:?}", other),
    }
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
