use async_trait::async_trait;
use base64::Engine as _;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use super::provider::{drain_held, SessionOpening, VoiceEvent, VoiceProvider, VoiceSession};
use super::{
    ANSWER_CHOICE_ARG, ANSWER_TOOL, ANSWER_TOOL_DESCRIPTION, DELEGATE_REASON_ARG, DELEGATE_TOOL,
    DELEGATE_TOOL_DESCRIPTION, HANGUP_TOOL, HANGUP_TOOL_DESCRIPTION,
};
use crate::engine::{ApiUsage, ModalityUsage};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const REALTIME_URL: &str = "wss://api.openai.com/v1/realtime";

/// INVARIANT: The transcribers that spell their language pin as an array.
///
/// A list, because `transcription()` branches on these ids and the picker in
/// `VoiceSection.tsx` offers them. That function says what the branch does.
const LANGUAGES_ARRAY_TRANSCRIBERS: &[&str] = &["gpt-live-transcribe", "gpt-transcribe"];

const EVENT_QUEUE: usize = 64;

const CANCEL_FOUND_NO_REPLY: &str = "response_cancel_not_active";

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
            loop {
                let message = match reader.next().await {
                    Some(Ok(message)) => message,
                    Some(Err(e)) => {
                        log_legacy!("[Voice] The Realtime socket failed: {}", e);
                        return;
                    }
                    None => return,
                };
                let Message::Text(text) = message else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    log_legacy!("[Voice] The talker sent something that is not JSON");
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
        let frame = json!({ "type": "response.create" });
        self.writer.send(Message::Text(frame.to_string())).await?;
        Ok(())
    }

    async fn resolve_tool_call(&mut self, tool_call_id: &str, note: &str) -> Result<(), BoxError> {
        let frame = json!({
            "type": "conversation.item.create",
            "item": {
                "type": "function_call_output",
                "call_id": tool_call_id,
                "output": note,
            }
        });
        self.writer.send(Message::Text(frame.to_string())).await?;
        // INVARIANT: No `response.create`. The talker already spoke in the turn that made
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

    async fn close(&mut self) -> Vec<VoiceEvent> {
        let _ = self.writer.close().await;
        drain_held(&mut self.rx).await
    }
}

pub fn session_update(opening: &SessionOpening) -> Value {
    json!({
        "type": "session.update",
        "session": {
            "type": "realtime",
            "instructions": opening.instructions,
            "output_modalities": ["audio"],
            "tools": tools(),
            "tool_choice": "auto",
            "audio": {
                "input": {
                    "format": {
                        "type": "audio/pcm",
                        "rate": opening.audio.sample_rate_hz,
                    },
                    // INVARIANT: Semantic, never a silence timer, and never a cut. See
                    // the module doc.
                    "turn_detection": {
                        "type": "semantic_vad",
                        "interrupt_response": false,
                    },
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

fn tools() -> Vec<Value> {
    vec![
        function(
            DELEGATE_TOOL,
            DELEGATE_TOOL_DESCRIPTION,
            Some((
                DELEGATE_REASON_ARG,
                "A few words on what the user asked for.",
            )),
        ),
        function(
            ANSWER_TOOL,
            ANSWER_TOOL_DESCRIPTION,
            Some((
                ANSWER_CHOICE_ARG,
                "The id of the choice the user picked, copied exactly.",
            )),
        ),
        function(HANGUP_TOOL, HANGUP_TOOL_DESCRIPTION, None),
    ]
}

fn function(name: &str, description: &str, argument: Option<(&str, &str)>) -> Value {
    let (properties, required) = match argument {
        Some((arg, detail)) => (
            json!({ arg: { "type": "string", "description": detail } }),
            json!([arg]),
        ),
        None => (json!({}), json!([])),
    };
    json!({
        "type": "function",
        "name": name,
        "description": description,
        "parameters": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

fn transcription(opening: &SessionOpening) -> Value {
    let model = match opening.transcriber.trim() {
        "" => crate::core::prefs::MODEL_VOICE_TRANSCRIBER.default_text(),
        named => named,
    };
    let mut config = json!({ "model": model });
    if let Some(code) = opening.language.as_ref().and_then(|l| l.code.as_deref()) {
        if LANGUAGES_ARRAY_TRANSCRIBERS.contains(&model) {
            config["languages"] = json!([code]);
        } else {
            config["language"] = json!(code);
        }
    }
    config
}

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
        "conversation.item.input_audio_transcription.delta" => {
            match value.get("delta").and_then(Value::as_str) {
                Some(text) if !text.is_empty() => vec![VoiceEvent::UserTranscript {
                    text: text.to_string(),
                }],
                _ => vec![],
            }
        }
        "input_audio_buffer.speech_started" => vec![VoiceEvent::CallerStartedSpeaking],
        "conversation.item.input_audio_transcription.completed" => {
            match value.get("transcript").and_then(Value::as_str) {
                Some(transcript) => vec![VoiceEvent::UserTurnEnded {
                    transcript: transcript.to_string(),
                }],
                None => vec![],
            }
        }
        "response.function_call_arguments.done" => tool_call(value),
        "response.done" => done_events(value),
        "error" => error_events(value),
        _ => vec![],
    }
}

/// INVARIANT: What one provider error means to the call.
///
/// A failure, with one exception: a cancel that found no reply to cut. A
/// barge-in races the reply's own end, so the provider can finish first and
/// then refuse the cancel. The session is untouched, and ending the call there
/// hangs up on a caller who only interrupted.
///
/// **The exception is one code, never a class.** Its type also covers a
/// rejected session payload, and swallowing that leaves the caller in silence.
fn error_events(value: &Value) -> Vec<VoiceEvent> {
    let message = value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("the talker reported an error with no message");
    if value.pointer("/error/code").and_then(Value::as_str) == Some(CANCEL_FOUND_NO_REPLY) {
        log_legacy!(
            "[Voice] A cancel arrived after the reply ended: {}",
            message
        );
        return vec![];
    }
    vec![VoiceEvent::Failed {
        message: message.to_string(),
    }]
}

/// INVARIANT: One finished tool call, as the seam's own event.
///
/// Read from `response.function_call_arguments.done` rather than from
/// `response.done`, because that frame lands while the talker is still
/// speaking. Waiting for the response to finish would cost a full spoken reply
/// of latency on every real question.
///
/// **An unnamed call is routed by its ARGUMENT, not dropped.** With one tool a
/// missing `name` could simply be assumed, and the code this replaced said so:
/// dropping the frame would lose the caller's question outright. Three tools
/// end the assumption, but not the risk it guarded. So the two tools with a
/// distinguishing required argument are recovered from it instead.
///
/// A name naming none of the three is dropped for the older reason: that is a
/// fourth tool appearing, and the design says there is none.
fn tool_call(value: &Value) -> Vec<VoiceEvent> {
    let Some(tool_call_id) = value.get("call_id").and_then(Value::as_str) else {
        log_legacy!("[Voice] A tool call arrived with no id, so it cannot be resolved");
        return vec![];
    };
    let tool_call_id = tool_call_id.to_string();
    let Some(name) = value.get("name").and_then(Value::as_str) else {
        return unnamed_tool_call(value, tool_call_id);
    };
    match name {
        DELEGATE_TOOL => vec![VoiceEvent::DelegationRequested {
            tool_call_id,
            reason: argument(value, DELEGATE_REASON_ARG)
                .unwrap_or_else(|| "the talker gave no reason".to_string()),
        }],
        ANSWER_TOOL => match argument(value, ANSWER_CHOICE_ARG) {
            Some(choice_id) => vec![VoiceEvent::AnswerRequested {
                tool_call_id,
                choice_id,
            }],
            None => {
                log_legacy!("[Voice] An answer arrived naming no choice, so it settles nothing");
                vec![]
            }
        },
        HANGUP_TOOL => vec![VoiceEvent::HangupRequested { tool_call_id }],
        _ => {
            log_legacy!(
                "[Voice] The talker called {:?}, which it does not hold",
                name
            );
            vec![]
        }
    }
}

/// INVARIANT: Which tool a call with no `name` was, read off its arguments.
///
/// Only the two carrying a distinguishing required argument are recoverable,
/// and each is recovered by the presence of its own. Hanging up is NOT: it
/// takes no argument, so an empty or unreadable payload looks exactly like one.
/// Guessing there would end a call the caller never asked to end.
fn unnamed_tool_call(value: &Value, tool_call_id: String) -> Vec<VoiceEvent> {
    if let Some(reason) = argument(value, DELEGATE_REASON_ARG) {
        return vec![VoiceEvent::DelegationRequested {
            tool_call_id,
            reason,
        }];
    }
    if let Some(choice_id) = argument(value, ANSWER_CHOICE_ARG) {
        return vec![VoiceEvent::AnswerRequested {
            tool_call_id,
            choice_id,
        }];
    }
    log_legacy!("[Voice] A tool call arrived unnamed and unreadable, so it cannot be routed");
    vec![]
}

fn argument(value: &Value, key: &str) -> Option<String> {
    let arguments = value.get("arguments").and_then(Value::as_str)?;
    let parsed = serde_json::from_str::<Value>(arguments).ok()?;
    let text = parsed.get(key)?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// INVARIANT: What one finished response means.
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
        service_tier: None,
        cache_creation_1h_tokens: None,
        cache_creation_30m_tokens: None,
        cache_creation_12h_tokens: None,
        cache_creation_requested_ttl: None,
    };
    warn_on_modality_drift(&reported);
    reported
}

/// INVARIANT: The per-modality breakdown, or `None` when the provider sent no detail.
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

fn warn_on_modality_drift(usage: &ApiUsage) {
    let Some(m) = usage.modality else { return };
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
        log_legacy!(
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
