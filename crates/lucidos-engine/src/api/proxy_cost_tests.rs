use super::*;
use serde_json::json;

fn body(value: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&value).unwrap()
}

#[test]
fn a_model_provider_host_is_recognised_and_a_plain_api_is_not() {
    for url in [
        "https://api.openai.com/v1",
        "https://api.anthropic.com",
        "https://openrouter.ai/api/v1",
        "https://api.x.ai/v1",
        "https://api.typesafe.ai/v1",
        "https://europe-west1-aiplatform.googleapis.com/v1",
        "https://aiplatform.eu.rep.googleapis.com/v1",
        "https://generativelanguage.googleapis.com/v1beta",
    ] {
        assert!(is_model_host(url), "{url}");
    }
    for url in [
        "https://api.cloudflare.com/client/v4",
        "http://127.0.0.1:8080",
        "not a url",
    ] {
        assert!(!is_model_host(url), "{url}");
    }
    assert!(Upstream::Builtin.is_model_provider());
}

/// The row keeps both: the model the request asked for, and the one the
/// reply says served it.
#[test]
fn a_json_reply_reports_its_usage_and_serving_model() {
    let reply = body(
        json!({"model": "gpt-6.1-2026", "usage": {"prompt_tokens": 30, "completion_tokens": 4}}),
    );
    let request = body(json!({"model": "gpt-6.1"}));
    let call = reported(&reply, false, &request, "/chat/completions").unwrap();
    assert_eq!((call.usage.input_tokens, call.usage.output_tokens), (30, 4));
    assert_eq!(call.model, "gpt-6.1");
    assert_eq!(call.served_model.as_deref(), Some("gpt-6.1-2026"));
}

#[test]
fn a_streamed_reply_takes_its_model_from_the_request() {
    let reply =
        b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":7}}}\n\n\
        data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":9}}\n\n";
    let request = body(json!({"model": "claude-sonnet-5", "stream": true}));
    let call = reported(reply, true, &request, "/messages").unwrap();
    assert_eq!((call.usage.input_tokens, call.usage.output_tokens), (7, 9));
    assert_eq!(call.model, "claude-sonnet-5");
    assert_eq!(call.served_model, None);
}

/// The stream `api::proxy_speech` asks OpenAI's speech endpoint for. Its last
/// frame carries the usage, and the model comes from the request.
#[test]
fn a_speech_stream_reports_its_usage_and_requested_model() {
    let reply = b"data: {\"type\":\"speech.audio.delta\",\"audio\":\"SUQz\"}\n\n\
        data: {\"type\":\"speech.audio.done\",\"usage\":{\"input_tokens\":2,\"output_tokens\":48}}\n\n\
        data: [DONE]\n\n";
    let request = body(json!({"model": "gpt-4o-mini-tts", "stream_format": "sse"}));
    let call = reported(reply, true, &request, "audio/speech").unwrap();
    assert_eq!((call.usage.input_tokens, call.usage.output_tokens), (2, 48));
    assert_eq!(call.model, "gpt-4o-mini-tts");
}

/// Vertex names its model in the path, not the body.
#[test]
fn a_vertex_reply_takes_its_model_from_the_path() {
    let reply = body(json!({"usageMetadata": {"promptTokenCount": 11, "candidatesTokenCount": 2}}));
    let path =
        "/projects/p/locations/global/publishers/google/models/gemini-3-flash:generateContent";
    assert_eq!(
        reported(&reply, false, b"{}", path).unwrap().model,
        "gemini-3-flash"
    );
}

/// A request that names no model, on a path that names none, is priced
/// under the model the reply names.
#[test]
fn a_request_naming_no_model_takes_the_replys() {
    let reply = body(json!({"model": "gpt-6.1-2026", "usage": {"prompt_tokens": 3}}));
    let call = reported(&reply, false, b"{}", "/responses").unwrap();
    assert_eq!(call.model, "gpt-6.1-2026");
    assert_eq!(call.served_model.as_deref(), Some("gpt-6.1-2026"));
}

/// A quota read or an error page spent nothing measurable, so nothing is
/// recorded rather than a free call.
#[test]
fn a_reply_without_usage_records_nothing() {
    assert!(reported(&body(json!({"remaining": 12})), false, b"", "/quota").is_none());
    assert!(reported(b"<html>bad gateway</html>", false, b"", "/v1").is_none());
}

/// The proxy route captures its path with no leading slash, and Gemini names
/// the serving model as `modelVersion`.
#[test]
fn a_gemini_reply_takes_its_model_from_the_bare_path_and_model_version() {
    let path = "models/gemini-2.5-flash:generateContent";
    let plain = reported(
        &body(json!({"usageMetadata": {"promptTokenCount": 4}})),
        false,
        b"{}",
        path,
    )
    .unwrap();
    assert_eq!(plain.model, "gemini-2.5-flash");
    assert_eq!(plain.served_model, None);
    let versioned = body(json!({
        "modelVersion": "gemini-2.5-flash-002",
        "usageMetadata": {"promptTokenCount": 4},
    }));
    let call = reported(&versioned, false, b"{}", path).unwrap();
    assert_eq!(call.model, "gemini-2.5-flash");
    assert_eq!(call.served_model.as_deref(), Some("gemini-2.5-flash-002"));
}

/// `streamGenerateContent` without `alt=sse` answers with one JSON array of
/// chunks, whose counters grow chunk by chunk.
#[test]
fn a_gemini_stream_sent_as_a_json_array_reports_its_usage() {
    let reply = body(json!([
        {"usageMetadata": {"promptTokenCount": 20, "candidatesTokenCount": 1}},
        {"usageMetadata": {"promptTokenCount": 20, "candidatesTokenCount": 9},
         "modelVersion": "gemini-3-flash"},
    ]));
    let call = reported(&reply, false, b"{}", "models/x:streamGenerateContent").unwrap();
    assert_eq!((call.usage.input_tokens, call.usage.output_tokens), (20, 9));
    assert_eq!(call.model, "x");
    assert_eq!(call.served_model.as_deref(), Some("gemini-3-flash"));
}

/// A model provider is asked for a plain reply, which the cost parse can
/// read. Any other upstream keeps the caller's encoding.
#[test]
fn only_a_model_provider_loses_the_callers_accept_encoding() {
    let headers = || {
        let mut h = HeaderMap::new();
        h.insert(ACCEPT_ENCODING, "gzip, br".parse().unwrap());
        h
    };
    let mut model = headers();
    prepare_request(&Upstream::Builtin, &mut model);
    assert!(model.get(ACCEPT_ENCODING).is_none());
    let mut plain = headers();
    prepare_request(
        &Upstream::Configured {
            base_url: "https://api.cloudflare.com/client/v4",
        },
        &mut plain,
    );
    assert_eq!(plain.get(ACCEPT_ENCODING).unwrap(), "gzip, br");
}

/// A proxied call's cost lands on the thread whose subprocess the origin
/// token proves made it. No token, or a forged one, names no thread.
#[test]
fn only_a_verified_origin_token_names_the_cost_thread() {
    use crate::api::actor::HEADER_AGENT_ORIGIN_TOKEN;
    use crate::api::actor::{init_agent_origin_secret, mint_agent_origin_token};
    let thread = uuid::Uuid::new_v4();
    init_agent_origin_secret("secret-for-actor-tests".into());
    let token = mint_agent_origin_token(Some(thread), 0, None).expect("the secret is installed");
    let with = |value: &str| {
        let mut h = HeaderMap::new();
        h.insert(HEADER_AGENT_ORIGIN_TOKEN, value.parse().unwrap());
        h
    };
    assert_eq!(cost_thread(&with(&token)), Some(thread));
    assert_eq!(cost_thread(&with("forged.00ff")), None);
    assert_eq!(cost_thread(&HeaderMap::new()), None);
}
