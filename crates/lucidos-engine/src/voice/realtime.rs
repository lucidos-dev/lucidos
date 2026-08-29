//! `Realtime`: the talker as one speech-to-speech model over a WebSocket.
//!
//! The first implementation behind the seam (the parent plan's decision 5). It
//! speaks OpenAI's Realtime API, and nothing above `provider.rs` knows that.
//!
//! **Opened with no tools.** ADR 0149 makes the talker tool-less, and
//! [`SessionOpening`] has no tool field to fill. The payload states it anyway,
//! so the model is told rather than merely left unconfigured.
//!
//! **End of turn is semantic.** `semantic_vad` asks the model whether the
//! caller finished a thought. A silence timer cannot tell a pause from a full
//! stop, which is the invariant the plan carries.

use async_trait::async_trait;
use base64::Engine as _;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use super::provider::{SessionOpening, VoiceEvent, VoiceProvider, VoiceSession};
use crate::engine::ApiUsage;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Where the talker lives. The one place a provider host is written down, and
/// it never leaves the engine (the plan's decision 3).
const REALTIME_URL: &str = "wss://api.openai.com/v1/realtime";

/// How many talker events may queue before the reader waits.
///
/// Audio arrives faster than a caller consumes it, so a little slack keeps the
/// reader off the critical path. Bounded, because an unbounded channel turns a
/// stalled caller into unbounded memory.
const EVENT_QUEUE: usize = 64;

/// A talker reached over OpenAI's Realtime API.
pub struct RealtimeProvider {
    api_key: String,
    model: String,
}

impl RealtimeProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self { api_key, model }
    }
}

#[async_trait]
impl VoiceProvider for RealtimeProvider {
    fn name(&self) -> &'static str {
        "realtime"
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn open(&self, opening: SessionOpening) -> Result<Box<dyn VoiceSession>, BoxError> {
        let url = format!("{}?model={}", REALTIME_URL, self.model);
        let mut request = url.into_client_request()?;
        request
            .headers_mut()
            .insert("Authorization", format!("Bearer {}", self.api_key).parse()?);

        let (stream, _) = tokio_tungstenite::connect_async(request).await?;
        let (mut writer, mut reader) = stream.split();

        writer
            .send(Message::Text(session_update(&opening).to_string()))
            .await?;
        writer
            .send(Message::Text(
                history_item(&opening.resident_block).to_string(),
            ))
            .await?;

        let (tx, rx) = mpsc::channel(EVENT_QUEUE);
        tokio::spawn(async move {
            while let Some(Ok(message)) = reader.next().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    log!("[Voice] The talker sent something that is not JSON");
                    continue;
                };
                for event in map_event(&value) {
                    if tx.send(event).await.is_err() {
                        return;
                    }
                }
            }
        });

        Ok(Box::new(RealtimeSession { writer, rx }))
    }
}

struct RealtimeSession {
    writer: futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    rx: mpsc::Receiver<VoiceEvent>,
}

#[async_trait]
impl VoiceSession for RealtimeSession {
    async fn push_audio(&mut self, pcm: &[u8]) -> Result<(), BoxError> {
        let audio = base64::engine::general_purpose::STANDARD.encode(pcm);
        let frame = json!({ "type": "input_audio_buffer.append", "audio": audio });
        self.writer.send(Message::Text(frame.to_string())).await?;
        Ok(())
    }

    async fn append_context(&mut self, note: &str) -> Result<(), BoxError> {
        self.writer
            .send(Message::Text(history_item(note).to_string()))
            .await?;
        Ok(())
    }

    async fn speak(&mut self, note: &str) -> Result<(), BoxError> {
        self.append_context(note).await?;
        // The second frame is the whole difference. Without it the item sits in
        // the history and the caller hears nothing.
        let frame = json!({ "type": "response.create" });
        self.writer.send(Message::Text(frame.to_string())).await?;
        Ok(())
    }

    async fn next(&mut self) -> Option<VoiceEvent> {
        self.rx.recv().await
    }

    async fn cancel(&mut self) -> Result<(), BoxError> {
        let frame = json!({ "type": "response.cancel" });
        self.writer.send(Message::Text(frame.to_string())).await?;
        Ok(())
    }

    async fn close(&mut self) {
        let _ = self.writer.close().await;
    }
}

/// The opening payload: who the talker is, what audio it speaks, and no tools.
pub fn session_update(opening: &SessionOpening) -> Value {
    json!({
        "type": "session.update",
        "session": {
            "type": "realtime",
            "instructions": opening.instructions,
            "output_modalities": ["audio"],
            "tools": [],
            "audio": {
                "input": {
                    "format": {
                        "type": "audio/pcm",
                        "rate": opening.audio.sample_rate_hz,
                    },
                    // Semantic, never a silence timer. See the module doc.
                    "turn_detection": { "type": "semantic_vad" },
                    "transcription": { "model": "gpt-4o-mini-transcribe" },
                },
                "output": {
                    "format": {
                        "type": "audio/pcm",
                        "rate": opening.audio.sample_rate_hz,
                    },
                    "voice": opening.voice,
                },
            },
        }
    })
}

/// One appended history item. Append-only: nothing here can edit an earlier
/// one, and creating an item does not ask the talker to answer it.
fn history_item(text: &str) -> Value {
    json!({
        "type": "conversation.item.create",
        "item": {
            "type": "message",
            "role": "system",
            "content": [{ "type": "input_text", "text": text }],
        }
    })
}

/// Map one provider frame onto zero or more [`VoiceEvent`].
///
/// Zero for the frames the seam has no word for, which is most of them: a
/// realtime API narrates its own state machine, and the seam models a
/// conversation.
pub fn map_event(value: &Value) -> Vec<VoiceEvent> {
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return vec![];
    };
    match kind {
        "response.output_audio.delta" => decoded_audio(value)
            .map(|pcm| vec![VoiceEvent::Audio(pcm)])
            .unwrap_or_default(),
        "response.output_audio_transcript.delta" => {
            match value.get("delta").and_then(Value::as_str) {
                Some(text) => vec![VoiceEvent::TalkerTranscript {
                    text: text.to_string(),
                }],
                None => vec![],
            }
        }
        "conversation.item.input_audio_transcription.completed" => {
            match value.get("transcript").and_then(Value::as_str) {
                Some(transcript) => vec![VoiceEvent::UserTurnEnded {
                    transcript: transcript.to_string(),
                }],
                None => vec![],
            }
        }
        "response.done" => done_events(value),
        "error" => vec![VoiceEvent::Failed {
            message: value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("the talker reported an error with no message")
                .to_string(),
        }],
        _ => vec![],
    }
}

/// What one finished response means.
///
/// The status decides. A CANCELLED response is the talker cut off mid-word, so
/// it reports the interruption as well as the turn: the tokens were spent
/// either way. A FAILED one is not a quiet turn end, and treating it as one
/// leaves the caller listening to silence with nothing said.
///
/// **The interruption is read here, never from the caller starting to speak.**
/// `input_audio_buffer.speech_started` fires on every utterance, including the
/// first word of a call with nothing playing, so mapping it would report an
/// interruption on every turn. The client detects its own barge-in anyway (the
/// plan's decision 3) and tells us with a `barge_in` control.
fn done_events(value: &Value) -> Vec<VoiceEvent> {
    let status = value
        .pointer("/response/status")
        .and_then(Value::as_str)
        .unwrap_or("completed");
    if status == "failed" {
        let message = value
            .pointer("/response/status_details/error/message")
            .and_then(Value::as_str)
            .unwrap_or("the talker could not finish a reply");
        return vec![VoiceEvent::Failed {
            message: message.to_string(),
        }];
    }

    let ended = VoiceEvent::TalkerTurnEnded {
        transcript: done_transcript(value),
        usage: done_usage(value),
    };
    match status {
        "cancelled" => vec![VoiceEvent::Interrupted, ended],
        _ => vec![ended],
    }
}

fn decoded_audio(value: &Value) -> Option<Vec<u8>> {
    let delta = value.get("delta").and_then(Value::as_str)?;
    base64::engine::general_purpose::STANDARD.decode(delta).ok()
}

/// What the talker said this turn, gathered from the finished response.
fn done_transcript(value: &Value) -> String {
    let Some(output) = value.pointer("/response/output").and_then(Value::as_array) else {
        return String::new();
    };
    output
        .iter()
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
        .filter_map(|part| part.get("transcript").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The turn's usage, with the cached and fresh split the plan requires.
///
/// `input_tokens` is a TOTAL that already contains `cached_tokens`, which is
/// the convention [`ApiUsage`] documents. `cache_creation_tokens` is zero: this
/// provider charges nothing for a cache write and reports no count for one.
fn done_usage(value: &Value) -> ApiUsage {
    let usage = value.pointer("/response/usage");
    let read = |path: &str| -> u32 {
        usage
            .and_then(|u| u.pointer(path))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32
    };
    ApiUsage {
        input_tokens: read("/input_tokens"),
        output_tokens: read("/output_tokens"),
        cache_read_tokens: read("/input_token_details/cached_tokens"),
        cache_creation_tokens: 0,
    }
}

#[cfg(test)]
#[path = "realtime_tests.rs"]
mod tests;
