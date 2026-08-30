//! `Realtime`: the talker as one speech-to-speech model over a WebSocket.
//!
//! The first implementation behind the seam (the parent plan's decision 5). It
//! speaks OpenAI's Realtime API, and nothing above `provider.rs` knows that.
//!
//! **Opened with exactly one tool.** [`SessionOpening`] still has no tool
//! field: the single `delegate` entry is built here from the words in
//! `voice/mod.rs`, so no caller can add a second.
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
use super::{DELEGATE_REASON_ARG, DELEGATE_TOOL, DELEGATE_TOOL_DESCRIPTION};
use crate::engine::{ApiUsage, ModalityUsage};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Where the talker lives. The one place a provider host is written down, and
/// it never leaves the engine (the plan's decision 3).
const REALTIME_URL: &str = "wss://api.openai.com/v1/realtime";

/// What turns the caller's audio into text when nothing named one.
///
/// The workspace's own choice arrives on [`SessionOpening::transcriber`], and
/// the catalog default is what fills it. This is the last resort under both, so
/// an empty opening still transcribes rather than opening a mute call.
const TRANSCRIBE_MODEL: &str = "gpt-4o-mini-transcribe";

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

    async fn resolve_delegation(
        &mut self,
        delegation_id: &str,
        note: &str,
    ) -> Result<(), BoxError> {
        let frame = json!({
            "type": "conversation.item.create",
            "item": {
                "type": "function_call_output",
                "call_id": delegation_id,
                "output": note,
            }
        });
        self.writer.send(Message::Text(frame.to_string())).await?;
        // No `response.create`. The talker already spoke in the turn that made
        // the call. Asking for another reply here is how the caller gets told
        // twice that work has started.
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

/// The opening payload: who the talker is, what audio it speaks, one tool.
pub fn session_update(opening: &SessionOpening) -> Value {
    json!({
        "type": "session.update",
        "session": {
            "type": "realtime",
            "instructions": opening.instructions,
            "output_modalities": ["audio"],
            "tools": [delegate_tool()],
            "tool_choice": "auto",
            "audio": {
                "input": {
                    "format": {
                        "type": "audio/pcm",
                        "rate": opening.audio.sample_rate_hz,
                    },
                    // Semantic, never a silence timer. See the module doc.
                    "turn_detection": { "type": "semantic_vad" },
                    "transcription": transcription(opening),
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

/// The one tool the talker holds, as this provider spells a function.
///
/// One required argument and nothing optional. A model that can leave the
/// reason out will, and an empty reason is a row that says a wake happened
/// without saying why.
fn delegate_tool() -> Value {
    json!({
        "type": "function",
        "name": DELEGATE_TOOL,
        "description": DELEGATE_TOOL_DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {
                DELEGATE_REASON_ARG: {
                    "type": "string",
                    "description": "A few words on what the user asked for.",
                }
            },
            "required": [DELEGATE_REASON_ARG],
            "additionalProperties": false,
        },
    })
}

/// How the caller's audio is turned into text.
///
/// The `language` key is added only when one resolved. Left off, the model
/// re-guesses per utterance, which is how a Bokmål phrase came back as
/// Nynorsk. The plan is
/// `docs/plans/2026-08-29-a-call-speaks-one-language-and-gives-one-answer.md`.
fn transcription(opening: &SessionOpening) -> Value {
    let model = match opening.transcriber.trim() {
        "" => TRANSCRIBE_MODEL,
        named => named,
    };
    let mut config = json!({ "model": model });
    if let Some(code) = opening.language.as_ref().and_then(|l| l.code.as_deref()) {
        config["language"] = json!(code);
    }
    config
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
        "response.function_call_arguments.done" => delegation(value),
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

/// One finished `delegate` call, as the seam's own event.
///
/// Read from `response.function_call_arguments.done` rather than from
/// `response.done`, because that frame lands while the talker is still
/// speaking. Waiting for the response to finish would cost a full spoken reply
/// of latency on every real question.
///
/// **A missing `name` still delegates.** One tool is declared, so nothing else
/// can arrive here, and dropping the frame would lose the caller's question
/// outright. A name that names something else is dropped, since that is a
/// second tool appearing and the whole design says there is none.
fn delegation(value: &Value) -> Vec<VoiceEvent> {
    let named = value.get("name").and_then(Value::as_str);
    if named.is_some_and(|name| name != DELEGATE_TOOL) {
        log!(
            "[Voice] The talker called {:?}, which it does not hold",
            named
        );
        return vec![];
    }
    let Some(delegation_id) = value.get("call_id").and_then(Value::as_str) else {
        log!("[Voice] A delegation arrived with no id, so it cannot be resolved");
        return vec![];
    };
    vec![VoiceEvent::DelegationRequested {
        delegation_id: delegation_id.to_string(),
        reason: delegation_reason(value),
    }]
}

/// The talker's stated reason, or a stand-in when it gave none.
///
/// The argument is required, so an absent one is the model misbehaving. The
/// delegation still goes through: a wake with a vague reason beats a caller
/// whose question never runs.
fn delegation_reason(value: &Value) -> String {
    let arguments = value.get("arguments").and_then(Value::as_str).unwrap_or("");
    serde_json::from_str::<Value>(arguments)
        .ok()
        .as_ref()
        .and_then(|parsed| parsed.get(DELEGATE_REASON_ARG))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .unwrap_or("the talker gave no reason")
        .to_string()
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

/// The turn's usage, with the cached and fresh split the plan requires, and the
/// per-modality split that prices it.
///
/// `input_tokens` is a TOTAL that already contains `cached_tokens`, which is
/// the convention [`ApiUsage`] documents. `cache_creation_tokens` is zero: this
/// provider charges nothing for a cache write and reports no count for one.
///
/// The realtime API bills audio at eight times the text input rate, so the
/// four flat counts cannot be priced on their own. [`ModalityUsage`] carries
/// the split, and `done_modality` decides whether the frame reported one.
fn done_usage(value: &Value) -> ApiUsage {
    let usage = value.pointer("/response/usage");
    let read = |path: &str| -> u32 {
        usage
            .and_then(|u| u.pointer(path))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32
    };
    let reported = ApiUsage {
        input_tokens: read("/input_tokens"),
        output_tokens: read("/output_tokens"),
        cache_read_tokens: read("/input_token_details/cached_tokens"),
        cache_creation_tokens: 0,
        modality: usage.and_then(done_modality),
    };
    warn_on_modality_drift(&reported);
    reported
}

/// The per-modality breakdown, or `None` when the provider sent no detail.
///
/// All-or-nothing, on purpose. Both detail blocks must be present, and a frame
/// carrying one of them reports `None` rather than half a breakdown. Zeros
/// cannot be told from a real turn: an all-zero output split reads as a reply
/// that spoke nothing, and the consumer would price it as free.
///
/// Inside a present block, an absent count IS zero. The provider omits
/// `image_tokens` on a voice turn, and omits `cached_tokens_details` when
/// nothing was cached.
fn done_modality(usage: &Value) -> Option<ModalityUsage> {
    let input = usage.pointer("/input_token_details")?;
    let output = usage.pointer("/output_token_details")?;
    let count = |block: &Value, path: &str| -> u32 {
        block.pointer(path).and_then(Value::as_u64).unwrap_or(0) as u32
    };
    Some(ModalityUsage {
        input_text_tokens: count(input, "/text_tokens"),
        input_audio_tokens: count(input, "/audio_tokens"),
        input_image_tokens: count(input, "/image_tokens"),
        cache_read_text_tokens: count(input, "/cached_tokens_details/text_tokens"),
        cache_read_audio_tokens: count(input, "/cached_tokens_details/audio_tokens"),
        cache_read_image_tokens: count(input, "/cached_tokens_details/image_tokens"),
        output_text_tokens: count(output, "/text_tokens"),
        output_audio_tokens: count(output, "/audio_tokens"),
    })
}

/// Log a breakdown whose parts do not sum to the flat totals.
///
/// The flat totals win and the parts are stored as reported. Rescaling would
/// invent numbers no frame carried and would hide the drift from whoever has
/// to fix it, so this line is the whole response: it is what tells us the
/// provider changed shape.
fn warn_on_modality_drift(usage: &ApiUsage) {
    let Some(m) = usage.modality else { return };
    // Saturating, because a malformed frame must not panic a live call.
    let input = m
        .input_text_tokens
        .saturating_add(m.input_audio_tokens)
        .saturating_add(m.input_image_tokens);
    let cached = m
        .cache_read_text_tokens
        .saturating_add(m.cache_read_audio_tokens)
        .saturating_add(m.cache_read_image_tokens);
    let output = m.output_text_tokens.saturating_add(m.output_audio_tokens);
    if input != usage.input_tokens
        || cached != usage.cache_read_tokens
        || output != usage.output_tokens
    {
        log!(
            "[Voice] usage modality split disagrees with the totals: input {} vs {}, cached {} vs {}, output {} vs {}. Storing both as reported.",
            input,
            usage.input_tokens,
            cached,
            usage.cache_read_tokens,
            output,
            usage.output_tokens
        );
    }
}

#[cfg(test)]
#[path = "realtime_tests.rs"]
mod tests;
