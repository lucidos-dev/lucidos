use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::http::header::{HeaderName, CONTENT_LENGTH, CONTENT_TYPE, ETAG};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde_json::Value;

use crate::api::proxy_cost::{is_event_stream, ReportedCall};
use crate::engine::ApiUsage;
use crate::engine::LucidosEngine;

const CONTENT_DIGEST: HeaderName = HeaderName::from_static("content-digest");
const DIGEST: HeaderName = HeaderName::from_static("digest");

const SPEECH_PATH: &str = "audio/speech";

const PER_CHARACTER_MODEL_PREFIX: &str = "tts-1";

const PCM_SAMPLE_RATE: u32 = 24_000;
const PCM_BYTES_PER_SAMPLE: u16 = 2;

#[derive(Debug)]
pub(crate) enum SpeechPlan {
    Untouched,
    Streamed(Container),
    PerCharacter { model: String, characters: usize },
}

#[derive(Debug, PartialEq)]
pub(crate) enum Container {
    AsStreamed(&'static str),
    Wav,
}

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

fn refusal(format: &str) -> (StatusCode, String) {
    (
        StatusCode::BAD_REQUEST,
        format!(
            "OpenAI reports no usage for {format} speech, so its cost would go \
             unrecorded. Ask for mp3, aac, pcm or wav instead (ADR 0423)."
        ),
    )
}

pub(crate) async fn finish(
    engine: &Arc<LucidosEngine>,
    owner: crate::api::proxy_cost::CostOwner,
    plan: SpeechPlan,
    request_body: &[u8],
    response: Response,
) -> Response {
    match plan {
        SpeechPlan::Untouched => response,
        SpeechPlan::Streamed(container) => restore_audio(container, response).await,
        SpeechPlan::PerCharacter { model, characters } => {
            if response.status().is_success() {
                let call = per_character_call(model, characters).answered_by(response.headers());
                crate::api::proxy_cost::record(engine, owner, call, request_body);
            }
            response
        }
    }
}

/// INVARIANT: A per-character call, with the characters in the input count. Its price
/// card is per 1M characters.
fn per_character_call(model: String, characters: usize) -> ReportedCall {
    ReportedCall {
        provider: Some("openai".into()),
        usage: ApiUsage {
            input_tokens: u32::try_from(characters).unwrap_or(u32::MAX),
            ..ApiUsage::default()
        },
        model,
        served_model: None,
        request_id: None,
    }
}

async fn restore_audio(container: Container, response: Response) -> Response {
    if !response.status().is_success() || !is_event_stream(response.headers()) {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let audio = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(stream) => audio_of(&String::from_utf8_lossy(&stream)),
        Err(e) => Err(format!("could not read the reply: {e}")),
    };
    let audio = match audio {
        Ok(audio) => audio,
        Err(e) => {
            crate::log_legacy!("[Proxy] A speech stream did not decode: {}", e);
            let message = format!("the speech stream did not decode: {e}");
            return (StatusCode::BAD_GATEWAY, message).into_response();
        }
    };
    let (content_type, file) = match container {
        Container::AsStreamed(content_type) => (content_type, audio),
        Container::Wav => ("audio/wav", wav_of(&audio)),
    };
    for stale in [CONTENT_LENGTH, ETAG, CONTENT_DIGEST, DIGEST] {
        parts.headers.remove(stale);
    }
    parts
        .headers
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    Response::from_parts(parts, Body::from(file))
}

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

fn wav_of(pcm: &[u8]) -> Vec<u8> {
    let data_len = u32::try_from(pcm.len()).unwrap_or(u32::MAX);
    let block_align = PCM_BYTES_PER_SAMPLE;
    let byte_rate = PCM_SAMPLE_RATE * u32::from(block_align);
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&data_len.saturating_add(36).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
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
