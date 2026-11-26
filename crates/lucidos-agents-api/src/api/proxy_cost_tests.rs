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
    assert!(Upstream::Builtin { name: "openai" }.is_model_provider());
}

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
    assert_eq!(call.served_model, None, "this stream names no model");
}

#[test]
fn a_streamed_reply_reports_the_model_it_names() {
    let anthropic = b"data: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-sonnet-5-20260101\",\"usage\":{\"input_tokens\":7}}}\n\n\
        data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":9}}\n\n";
    let request = body(json!({"model": "claude-sonnet-5", "stream": true}));
    let call = reported(anthropic, true, &request, "/messages").unwrap();
    assert_eq!(call.model, "claude-sonnet-5");
    assert_eq!(
        call.served_model.as_deref(),
        Some("claude-sonnet-5-20260101")
    );

    let openai = b"data: {\"model\":\"gpt-6.1-2026\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
        data: {\"model\":\"gpt-6.1-2026\",\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":1}}\n\n\
        data: [DONE]\n\n";
    let request = body(json!({"model": "gpt-6.1", "stream": true}));
    let call = reported(openai, true, &request, "/chat/completions").unwrap();
    assert_eq!(call.model, "gpt-6.1");
    assert_eq!(call.served_model.as_deref(), Some("gpt-6.1-2026"));
}

#[test]
fn a_reply_header_names_the_request_id() {
    let call = || ReportedCall {
        usage: Default::default(),
        model: "gpt-6.1".to_string(),
        served_model: None,
        request_id: None,
        provider: None,
    };
    let mut headers = HeaderMap::new();
    assert_eq!(call().answered_by(&headers).request_id, None);
    headers.insert("x-request-id", "req_openai".parse().unwrap());
    assert_eq!(
        call().answered_by(&headers).request_id.as_deref(),
        Some("req_openai")
    );
}

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

#[test]
fn a_request_naming_no_model_takes_the_replys() {
    let reply = body(json!({"model": "gpt-6.1-2026", "usage": {"prompt_tokens": 3}}));
    let call = reported(&reply, false, b"{}", "/responses").unwrap();
    assert_eq!(call.model, "gpt-6.1-2026");
    assert_eq!(call.served_model.as_deref(), Some("gpt-6.1-2026"));
}

#[test]
fn a_reply_without_usage_records_nothing() {
    assert!(reported(&body(json!({"remaining": 12})), false, b"", "/quota").is_none());
    assert!(reported(b"<html>bad gateway</html>", false, b"", "/v1").is_none());
}

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

#[test]
fn only_a_model_provider_loses_the_callers_accept_encoding() {
    let headers = || {
        let mut h = HeaderMap::new();
        h.insert(ACCEPT_ENCODING, "gzip, br".parse().unwrap());
        h
    };
    let mut model = headers();
    prepare_request(&Upstream::Builtin { name: "openai" }, &mut model);
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

fn origin_headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        crate::core::subprocess_origin::HEADER_AGENT_ORIGIN_TOKEN,
        token.parse().unwrap(),
    );
    h
}

#[test]
fn only_a_verified_origin_token_names_the_cost_thread() {
    use crate::core::subprocess_origin::{init_agent_origin_secret, mint_agent_origin_token};
    let thread = uuid::Uuid::new_v4();
    init_agent_origin_secret("secret-for-actor-tests".into());
    let token = mint_agent_origin_token(Some(thread), 0, None).expect("the secret is installed");
    let owner = cost_owner(&origin_headers(&token));
    assert_eq!((owner.thread, owner.trigger), (Some(thread), None));
    assert_eq!(
        cost_owner(&origin_headers("forged.00ff")),
        CostOwner::default()
    );
    assert_eq!(cost_owner(&HeaderMap::new()), CostOwner::default());
}

#[test]
fn a_trigger_fires_token_names_the_cost_trigger() {
    use crate::core::subprocess_origin::{init_agent_origin_secret, mint_agent_origin_token};
    init_agent_origin_secret("secret-for-actor-tests".into());
    let token = mint_agent_origin_token(None, 1, Some("trg-hourly")).expect("installed");
    let owner = cost_owner(&origin_headers(&token));
    assert_eq!(owner.thread, None);
    assert_eq!(owner.trigger.as_deref(), Some("trg-hourly"));
}

#[test]
fn gateway_usage_keeps_its_provider_for_managed_telemetry() {
    let request = body(json!({"model":"claude-haiku-5-5"}));
    for (reply, streamed) in [
        (body(json!({"model":"claude-haiku-5-5", "usage":{"input_tokens":7,"output_tokens":9}})), false),
        (b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":7}}}\n\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":9}}\n\n".to_vec(), true),
    ] {
        let call = reported(&reply, streamed, &request, "/messages").unwrap()
            .with_upstream(&Upstream::Builtin { name: "claude-gateway" });
        assert_eq!(call.provider.as_deref(), Some("claude-gateway"));
        assert_eq!((call.usage.input_tokens, call.usage.output_tokens), (7,9));
    }
}

#[tokio::test]
async fn forward_request_hands_back_a_streamed_upstream_reply_as_one_buffer() {
    use futures::StreamExt;
    let app = axum::Router::new().fallback(|| async {
        let chunks = futures::stream::iter([0u8, 1]).then(|i| async move {
            if i == 1 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Ok::<_, std::io::Error>(Bytes::from(if i == 0 { "ab" } else { "cd" }))
        });
        Body::from_stream(chunks)
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/x", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let response = crate::api::proxy::forward_request(
        axum::http::Method::GET,
        &url,
        &url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
        crate::api::proxy::Transport::Verified,
        crate::api::proxy_timeout::default_wait(),
    )
    .await;
    let chunks: Vec<_> = response.into_body().into_data_stream().collect().await;
    server.abort();
    assert_eq!(chunks.len(), 1, "the reply must arrive whole");
    assert_eq!(chunks[0].as_ref().unwrap().as_ref(), b"abcd");
}
