use super::*;
use axum::http::HeaderMap;
use serde_json::json;

const OPENAI: &str = crate::llm::openai::OPENAI_DEFAULT_BASE_URL;

fn body(value: serde_json::Value) -> Bytes {
    serde_json::to_vec(&value).unwrap().into()
}

fn speech(extra: serde_json::Value) -> Bytes {
    let mut request = json!({"model": "gpt-4o-mini-tts", "voice": "coral", "input": "Hi."});
    request
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    body(request)
}

fn sent(body: &Bytes) -> serde_json::Value {
    serde_json::from_slice(body).unwrap()
}

fn plan(path: &str, request: Bytes) -> (Bytes, SpeechPlan) {
    ask_for_usage(OPENAI, path, request).expect("not refused")
}

#[test]
fn a_default_speech_request_asks_for_the_stream() {
    let (forwarded, plan) = plan("audio/speech", speech(json!({})));
    assert_eq!(sent(&forwarded)["stream_format"], "sse");
    assert_eq!(sent(&forwarded)["response_format"], "mp3");
    assert_eq!(sent(&forwarded)["input"], "Hi.");
    assert!(matches!(
        plan,
        SpeechPlan::Streamed(Container::AsStreamed("audio/mpeg"))
    ));
}

/// A caller's explicit `audio` is the default spelled out. The proxy buffers
/// the reply whole either way, so no streaming is lost.
#[test]
fn an_explicit_audio_stream_format_is_asked_for_the_stream_too() {
    let request = speech(json!({"stream_format": "audio", "response_format": "aac"}));
    let (forwarded, plan) = plan("/v1/audio/speech/", request);
    assert_eq!(sent(&forwarded)["stream_format"], "sse");
    assert!(matches!(
        plan,
        SpeechPlan::Streamed(Container::AsStreamed("audio/aac"))
    ));
}

/// A `wav` stream loses its header, so the proxy asks for PCM and adds one.
#[test]
fn a_wav_request_asks_for_pcm() {
    let (forwarded, plan) = plan("audio/speech", speech(json!({"response_format": "wav"})));
    assert_eq!(sent(&forwarded)["response_format"], "pcm");
    assert_eq!(sent(&forwarded)["stream_format"], "sse");
    assert!(matches!(plan, SpeechPlan::Streamed(Container::Wav)));
}

#[test]
fn a_flac_or_opus_request_is_refused_before_it_leaves() {
    for format in ["flac", "opus"] {
        let request = speech(json!({"response_format": format}));
        let (status, message) = ask_for_usage(OPENAI, "audio/speech", request).unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(message.contains(format), "{message}");
        assert!(message.contains("mp3, aac, pcm or wav"), "{message}");
    }
}

/// A caller that asked for the stream itself gets its usage recorded, so
/// even `flac` passes.
#[test]
fn a_caller_chosen_stream_is_never_refused() {
    let request = speech(json!({"stream_format": "sse", "response_format": "flac"}));
    let (forwarded, plan) = plan("audio/speech", request.clone());
    assert_eq!(forwarded, request);
    assert!(matches!(plan, SpeechPlan::Untouched));
}

/// `tts-1` reports no usage, so the input's characters are what it costs.
#[test]
fn a_tts_1_request_is_billed_by_its_characters() {
    for model in ["tts-1", "tts-1-hd"] {
        let request = speech(json!({"model": model, "input": "Héllo wörld"}));
        let (forwarded, plan) = plan("audio/speech", request.clone());
        assert_eq!(forwarded, request, "{model} is forwarded untouched");
        let SpeechPlan::PerCharacter {
            model: billed,
            characters,
        } = plan
        else {
            panic!("{model} is billed per character: {plan:?}");
        };
        assert_eq!((billed.as_str(), characters), (model, 11));
        let call = per_character_call(billed, characters);
        assert_eq!(call.usage.input_tokens, 11);
        assert_eq!(call.usage.output_tokens, 0);
    }
}

/// Every one of these reaches the upstream byte for byte.
#[test]
fn only_an_openai_speech_request_is_touched() {
    let untouched = [
        (
            OPENAI,
            "audio/speech",
            speech(json!({"response_format": "mp4"})),
        ),
        (OPENAI, "chat/completions", speech(json!({}))),
        (OPENAI, "v1/notaudio/speech", speech(json!({}))),
        (OPENAI, "audio/speech", Bytes::from_static(b"not json")),
        (
            "http://127.0.0.1:8080/v1",
            "audio/speech",
            speech(json!({})),
        ),
    ];
    for (base_url, path, request) in untouched {
        let (forwarded, plan) = ask_for_usage(base_url, path, request.clone()).unwrap();
        assert_eq!(forwarded, request, "{base_url} {path}");
        assert!(matches!(plan, SpeechPlan::Untouched), "{base_url} {path}");
    }
}

fn delta(audio: &[u8]) -> String {
    let chunk = base64::engine::general_purpose::STANDARD.encode(audio);
    format!(
        "data: {}\n\n",
        json!({"type": "speech.audio.delta", "audio": chunk})
    )
}

const DONE: &str = "data: {\"type\":\"speech.audio.done\",\"usage\":{\"input_tokens\":2,\"output_tokens\":48}}\n\ndata: [DONE]\n\n";

fn upstream_reply(status: StatusCode, content_type: &str, body: String) -> Response {
    let mut response = Response::new(Body::from(body.clone()));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, content_type.parse().unwrap());
    headers.insert(CONTENT_LENGTH, body.len().into());
    response
}

async fn read(response: Response) -> (StatusCode, HeaderMap, Vec<u8>) {
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    (parts.status, parts.headers, bytes.to_vec())
}

const MP3: Container = Container::AsStreamed("audio/mpeg");

#[tokio::test]
async fn the_caller_gets_the_audio_the_deltas_carry() {
    let stream = format!("{}{}{DONE}", delta(b"\xff\xf3ab"), delta(b"cd"));
    let mut reply = upstream_reply(StatusCode::OK, "text/event-stream; charset=utf-8", stream);
    let headers = reply.headers_mut();
    headers.insert(ETAG, "\"stream\"".parse().unwrap());
    headers.insert(CONTENT_DIGEST, "sha-256=:AAAA=:".parse().unwrap());
    let (status, headers, audio) = read(restore_audio(MP3, reply).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(audio, b"\xff\xf3abcd");
    assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "audio/mpeg");
    // These described the stream, so none may survive with the stream's value.
    assert!(headers
        .get(CONTENT_LENGTH)
        .is_none_or(|v| v.to_str().unwrap() == audio.len().to_string()));
    assert!(headers.get(ETAG).is_none());
    assert!(headers.get(CONTENT_DIGEST).is_none());
}

/// The header OpenAI's own `wav` reply carries: PCM, mono, 24 kHz, 16-bit.
#[tokio::test]
async fn a_wav_caller_gets_its_pcm_inside_a_wav_header() {
    let pcm = [1u8, 0, 2, 0, 3, 0];
    let stream = format!("{}{DONE}", delta(&pcm));
    let reply = upstream_reply(StatusCode::OK, "text/event-stream", stream);
    let (_, headers, wav) = read(restore_audio(Container::Wav, reply).await).await;
    assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "audio/wav");
    assert_eq!(wav.len(), 44 + pcm.len());
    let u16_at = |i: usize| u16::from_le_bytes([wav[i], wav[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(wav[i..i + 4].try_into().unwrap());
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(u32_at(4), 36 + 6);
    assert_eq!(&wav[8..16], b"WAVEfmt ");
    assert_eq!((u32_at(16), u16_at(20), u16_at(22)), (16, 1, 1));
    assert_eq!((u32_at(24), u32_at(28)), (24_000, 48_000));
    assert_eq!((u16_at(32), u16_at(34)), (2, 16));
    assert_eq!(&wav[36..40], b"data");
    assert_eq!(u32_at(40), 6);
    assert_eq!(&wav[44..], pcm);
}

#[tokio::test]
async fn an_upstream_error_reaches_the_caller_unchanged() {
    let error = json!({"error": {"message": "bad voice"}}).to_string();
    let reply = upstream_reply(StatusCode::BAD_REQUEST, "application/json", error.clone());
    let (status, headers, bytes) = read(restore_audio(MP3, reply).await).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "application/json");
    assert_eq!(bytes, error.as_bytes());
}

/// An upstream that ignored the stream format sent audio already.
#[tokio::test]
async fn a_reply_that_is_not_a_stream_passes_through() {
    let reply = upstream_reply(StatusCode::OK, "audio/mpeg", "ID3".into());
    let (_, headers, bytes) = read(restore_audio(MP3, reply).await).await;
    assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "audio/mpeg");
    assert_eq!(bytes, b"ID3");
}

/// Each would otherwise reach the caller as an empty or cut-off file with a
/// 200: no audio, a delta with no audio, and a stream with no done frame.
#[tokio::test]
async fn a_stream_without_whole_audio_is_a_bad_gateway() {
    for stream in [
        DONE.to_string(),
        format!("data: {{\"type\":\"speech.audio.delta\"}}\n\n{DONE}"),
        delta(b"\xff\xf3ab"),
    ] {
        let reply = upstream_reply(StatusCode::OK, "text/event-stream", stream);
        let (status, _, _) = read(restore_audio(MP3, reply).await).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
    }
}
