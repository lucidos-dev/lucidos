//! What a text-to-speech call through the proxy cost.
//!
//! OpenAI's `/audio/speech` reports its usage only on a server-sent event
//! stream, in the closing `speech.audio.done` frame. Its default reply is bare
//! audio with no usage, so `api::proxy_cost` could record nothing for it.
//!
//! So the proxy asks for the stream on the caller's behalf. `proxy_cost` then
//! records the stream's usage, and this module hands the caller the audio the
//! frames carry. `tts-1` streams nothing, so it records the input's length,
//! which OpenAI bills by. A format the proxy cannot record is refused.
//! Why: ADR 0423.

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::http::header::{HeaderName, CONTENT_LENGTH, CONTENT_TYPE, ETAG};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde_json::Value;
use uuid::Uuid;

use crate::api::proxy_cost::{is_event_stream, ReportedCall};
use crate::engine::LucidosEngine;
use crate::llm::usage_wire::ProviderUsage;

const CONTENT_DIGEST: HeaderName = HeaderName::from_static("content-digest");
const DIGEST: HeaderName = HeaderName::from_static("digest");

/// OpenAI's speech endpoint, relative to its API base.
const SPEECH_PATH: &str = "audio/speech";

/// `tts-1` and `tts-1-hd` stream no SSE, and OpenAI bills them per character.
const PER_CHARACTER_MODEL_PREFIX: &str = "tts-1";

/// OpenAI's raw PCM: 24 kHz, mono, signed 16-bit little-endian.
const PCM_SAMPLE_RATE: u32 = 24_000;
const PCM_BYTES_PER_SAMPLE: u16 = 2;

/// What the proxy does with one request.
#[derive(Debug)]
pub(crate) enum SpeechPlan {
    /// Not a speech call it changes.
    Untouched,
    /// Asked for the stream. The caller gets this container back.
    Streamed(Container),
    /// Billed per input character, recorded once the call succeeds.
    PerCharacter { model: String, characters: usize },
}

/// The audio file the caller gets from a stream's PCM or encoded frames.
#[derive(Debug, PartialEq)]
pub(crate) enum Container {
    /// The frames are already the file, with this `Content-Type`.
    AsStreamed(&'static str),
    /// The frames are PCM, wrapped in a WAV header.
    Wav,
}

/// Decide what to do with a request, and the body to forward. A speech
/// format whose cost no reply reports is refused before it leaves.
pub(crate) fn ask_for_usage(
    base_url: &str,
    path: &str,
    body: Bytes,
) -> Result<(Bytes, SpeechPlan), (StatusCode, String)> {
    let Some(mut request) = speech_request(base_url, path, &body) else {
        return Ok((body, SpeechPlan::Untouched));
    };
    let Some(model) = request.get("model").and_then(Value::as_str) else {
        return Ok((body, SpeechPlan::Untouched));
    };
    if model.starts_with(PER_CHARACTER_MODEL_PREFIX) {
        let characters = request
            .get("input")
            .and_then(Value::as_str)
            .map_or(0, |input| input.chars().count());
        let model = model.to_string();
        return Ok((body, SpeechPlan::PerCharacter { model, characters }));
    }
    let stream_format = request.get("stream_format").map(Value::as_str);
    if !matches!(stream_format, None | Some(Some("audio"))) {
        return Ok((body, SpeechPlan::Untouched));
    }
    let format = request
        .get("response_format")
        .map_or(Some("mp3"), Value::as_str);
    let (asked, container) = match format {
        Some("mp3") => ("mp3", Container::AsStreamed("audio/mpeg")),
        Some("aac") => ("aac", Container::AsStreamed("audio/aac")),
        Some("pcm") => ("pcm", Container::AsStreamed("audio/pcm")),
        Some("wav") => ("pcm", Container::Wav),
        Some(format @ ("flac" | "opus")) => return Err(refusal(format)),
        _ => return Ok((body, SpeechPlan::Untouched)),
    };
    request.insert("stream_format".into(), "sse".into());
    request.insert("response_format".into(), asked.into());
    match serde_json::to_vec(&request) {
        Ok(streamed) => Ok((streamed.into(), SpeechPlan::Streamed(container))),
        Err(_) => Ok((body, SpeechPlan::Untouched)),
    }
}

/// The JSON body of a request to OpenAI's speech endpoint, if it is one.
fn speech_request(
    base_url: &str,
    path: &str,
    body: &[u8],
) -> Option<serde_json::Map<String, Value>> {
    use crate::engine::tools::credentials::host_of;
    let openai = host_of(crate::llm::openai::OPENAI_DEFAULT_BASE_URL);
    let path = path.trim_matches('/');
    let speech = path == SPEECH_PATH || path.ends_with(&format!("/{SPEECH_PATH}"));
    if host_of(base_url) != openai || !speech {
        return None;
    }
    serde_json::from_slice(body).ok()
}

/// A `flac` or `opus` stream loses its container, and the plain reply reports
/// no usage. So neither can be served with its cost recorded.
fn refusal(format: &str) -> (StatusCode, String) {
    (
        StatusCode::BAD_REQUEST,
        format!(
            "OpenAI reports no usage for {format} speech, so its cost would go \
             unrecorded. Ask for mp3, aac, pcm or wav instead (ADR 0423)."
        ),
    )
}

/// Hand the caller what it asked for, and record a per-character call.
pub(crate) async fn finish(
    engine: &Arc<LucidosEngine>,
    cost_thread: Option<Uuid>,
    plan: SpeechPlan,
    request_body: &[u8],
    response: Response,
) -> Response {
    match plan {
        SpeechPlan::Untouched => response,
        SpeechPlan::Streamed(container) => restore_audio(container, response).await,
        SpeechPlan::PerCharacter { model, characters } => {
            if response.status().is_success() {
                let call = per_character_call(model, characters);
                crate::api::proxy_cost::record(engine, cost_thread, call, request_body);
            }
            response
        }
    }
}

/// A per-character call, with the characters in the input count. Its price
/// card is per 1M characters.
fn per_character_call(model: String, characters: usize) -> ReportedCall {
    ReportedCall {
        usage: ProviderUsage {
            input_tokens: u32::try_from(characters).unwrap_or(u32::MAX),
            ..ProviderUsage::default()
        },
        model,
        served_model: None,
    }
}

/// Turn a rewritten call's stream back into the audio the caller asked for.
///
/// An error reply, or one that is not a stream, passes through unchanged.
async fn restore_audio(container: Container, response: Response) -> Response {
    if !response.status().is_success() || !is_event_stream(response.headers()) {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    // `forward_request` buffers every reply whole, so this reads memory.
    let audio = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(stream) => audio_of(&String::from_utf8_lossy(&stream)),
        Err(e) => Err(format!("could not read the reply: {e}")),
    };
    let audio = match audio {
        Ok(audio) => audio,
        Err(e) => {
            crate::log!("[Proxy] A speech stream did not decode: {}", e);
            let message = format!("the speech stream did not decode: {e}");
            return (StatusCode::BAD_GATEWAY, message).into_response();
        }
    };
    let (content_type, file) = match container {
        Container::AsStreamed(content_type) => (content_type, audio),
        Container::Wav => ("audio/wav", wav_of(&audio)),
    };
    // Each of these describes the stream's bytes, not the audio's.
    for stale in [CONTENT_LENGTH, ETAG, CONTENT_DIGEST, DIGEST] {
        parts.headers.remove(stale);
    }
    parts
        .headers
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    Response::from_parts(parts, Body::from(file))
}

/// The audio a stream's `speech.audio.delta` frames carry, in order. The
/// closing `[DONE]` line is not JSON and is skipped.
///
/// A stream that stops before `speech.audio.done` was cut off mid-speech, so
/// its partial audio is an upstream failure, not a reply.
fn audio_of(stream: &str) -> Result<Vec<u8>, String> {
    let mut audio = Vec::new();
    let mut done = false;
    let frames = stream
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok());
    for frame in frames {
        match frame.get("type").and_then(Value::as_str) {
            Some("speech.audio.delta") => {}
            Some("speech.audio.done") => {
                done = true;
                continue;
            }
            _ => continue,
        }
        let chunk = frame
            .get("audio")
            .and_then(Value::as_str)
            .ok_or("a delta frame carried no audio")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(chunk)
            .map_err(|e| format!("a delta frame's audio is not base64: {e}"))?;
        audio.extend(bytes);
    }
    if !done {
        return Err("the stream ended before speech.audio.done".into());
    }
    if audio.is_empty() {
        return Err("the stream carried no audio".into());
    }
    Ok(audio)
}

/// OpenAI's PCM inside the 44-byte header of a plain PCM WAV file.
fn wav_of(pcm: &[u8]) -> Vec<u8> {
    let data_len = u32::try_from(pcm.len()).unwrap_or(u32::MAX);
    let block_align = PCM_BYTES_PER_SAMPLE;
    let byte_rate = PCM_SAMPLE_RATE * u32::from(block_align);
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&data_len.saturating_add(36).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&PCM_SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&(PCM_BYTES_PER_SAMPLE * 8).to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

#[cfg(test)]
#[path = "proxy_speech_tests.rs"]
mod tests;
