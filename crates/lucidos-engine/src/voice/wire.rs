//! What travels over `/api/v1/voice`, and nothing else.
//!
//! **The client stays dumb** (the plan's decision 3). No provider name, model
//! id, endpoint or credential appears in any type here. The test below asserts
//! the full key set of every frame, so a new field has to pass it to arrive.
//!
//! Audio does not travel as JSON. A binary frame IS the audio, in the PCM the
//! opening frame names, so nothing here carries a sample.

use serde::{Deserialize, Serialize};

use super::provider::AudioFormat;
use crate::engine::thread_events::VoiceSessionEndReason;

/// The PCM both directions speak, named once at the top of the call.
///
/// Serialize only. `encoding` is a `&'static str`, which serde can read back
/// only from a borrowed-for-static input, so a `Deserialize` here would be a
/// derive nobody can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AudioSpec {
    pub sample_rate_hz: u32,
    pub channels: u8,
    /// The sample encoding, spelled out so a client never has to assume one.
    pub encoding: &'static str,
}

impl From<AudioFormat> for AudioSpec {
    fn from(format: AudioFormat) -> Self {
        Self {
            sample_rate_hz: format.sample_rate_hz,
            channels: format.channels,
            encoding: "pcm_s16le",
        }
    }
}

/// A text frame from the client. Its binary frames are microphone audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientControl {
    /// The caller started speaking over the talker. Stop it mid-word.
    BargeIn,
    /// The caller rang off. The ordinary end of a call.
    HangUp,
}

/// A text frame to the client. Its binary frames are talker audio.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    /// The call is up. Carries the PCM to send and to expect.
    SessionStarted { audio: AudioSpec },
    /// The caller stopped talking, and this is what was heard.
    UserTurnEnded { transcript: String },
    /// A piece of what the talker is saying, as it says it.
    TalkerTranscript { text: String },
    /// The talker finished its reply.
    TalkerTurnEnded,
    /// The talker was cut off mid-word by the caller.
    Interrupted,
    /// The call is over, and why.
    SessionEnded { reason: VoiceSessionEndReason },
    /// The call could not start, or could not go on. Plain English, because a
    /// person reads it: never a provider name and never a status code.
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(value: &serde_json::Value) -> Vec<String> {
        let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    }

    /// The client-ignorance invariant, asserted over the whole key set rather
    /// than over a guessed field name. A provider name, model id, endpoint or
    /// key could only arrive by failing this.
    #[test]
    fn client_frames_name_nothing() {
        let cases = [
            (
                ServerFrame::SessionStarted {
                    audio: AudioFormat::default().into(),
                },
                vec!["audio", "type"],
            ),
            (
                ServerFrame::UserTurnEnded {
                    transcript: "hello".into(),
                },
                vec!["transcript", "type"],
            ),
            (
                ServerFrame::TalkerTranscript { text: "hi".into() },
                vec!["text", "type"],
            ),
            (ServerFrame::TalkerTurnEnded, vec!["type"]),
            (ServerFrame::Interrupted, vec!["type"]),
            (
                ServerFrame::SessionEnded {
                    reason: VoiceSessionEndReason::Hangup,
                },
                vec!["reason", "type"],
            ),
            (
                ServerFrame::Error {
                    message: "no voice model is configured".into(),
                },
                vec!["message", "type"],
            ),
        ];
        for (frame, expected) in cases {
            let json = serde_json::to_value(&frame).unwrap();
            assert_eq!(keys(&json), expected, "{:?}", frame);
        }
    }

    /// The audio spec is the one nested object, so its keys are checked too.
    #[test]
    fn the_audio_spec_names_only_the_pcm() {
        let json = serde_json::to_value(AudioSpec::from(AudioFormat::default())).unwrap();
        assert_eq!(keys(&json), vec!["channels", "encoding", "sample_rate_hz"]);
        assert_eq!(json["encoding"], "pcm_s16le");
        assert_eq!(json["sample_rate_hz"], 24_000);
    }

    #[test]
    fn the_client_can_only_say_two_things() {
        let barge: ClientControl = serde_json::from_str(r#"{"type":"barge_in"}"#).unwrap();
        assert_eq!(barge, ClientControl::BargeIn);
        let hang: ClientControl = serde_json::from_str(r#"{"type":"hang_up"}"#).unwrap();
        assert_eq!(hang, ClientControl::HangUp);
        assert!(serde_json::from_str::<ClientControl>(r#"{"type":"open"}"#).is_err());
    }
}
