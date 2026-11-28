pub mod anthropic;
pub mod anthropic_wire;
pub mod api_usage;
mod aux_provider;
pub mod auxiliary_prompts;
pub mod cache_lifetime;
/// Temporary diagnostic, off unless `LUCIDOS_CACHE_PROBE` is set. Tracked in
/// `docs/temporary-measures.md`.
pub mod cache_probe;
pub mod call_trace;
pub mod cc_model_discovery;
pub mod chatgpt_plan;
pub mod chatgpt_plan_error;
pub mod chatgpt_plan_pause;
pub mod claude_gateway;
pub mod claude_model;
pub(crate) mod claude_prefill;
pub mod coding_agent_models;
pub mod context_tokens;
pub mod image;
pub mod intelligence_rungs;
pub mod judgment;
pub mod metered;
pub mod mock;
pub use mock::MOCK_MODEL;
pub mod model_preferences;
pub mod model_prices;
pub mod model_registry;
pub mod model_registry_db;
pub mod not_served;
pub mod openai;
pub mod openai_wire;
pub mod prefill;
pub mod provider;
pub mod provider_build;
pub mod provider_credentials;
pub mod provider_error;
pub mod provider_factory;
pub mod provider_preferences;
pub mod provider_selection;
pub mod reasoning;
pub mod routing;
pub mod served_model;
pub mod spend_cap;
pub(crate) mod stable_prefix;
pub mod tool_names;
pub mod tools;
pub mod unconfigured;
pub mod usage_wire;
pub mod validate;
pub mod vertex;
pub mod vertex_auth;
pub mod vertex_defaults;
pub mod web_search;

pub use anthropic::AnthropicProvider;
pub use aux_provider::{AuxProvider, NotServedReport};
pub use judgment::{
    Answers, ChoiceAnswer, Judgment, JudgmentProvider, NoulCriteria, Question, SystemOneProvider,
    JEV_DEFAULT_MODEL,
};
pub use model_registry::{ModelRegistry, ProviderKind};
pub use not_served::ModelNotServed;
pub use openai::OpenAiProvider;
pub use openai_wire::OPENAI_DEFAULT_BASE_URL;
pub use provider::{
    CacheMode, ContentBlock, LlmProvider, Message, MessageContent, ModelSelection, ServiceTier,
    TokenCallback, ToolCall,
};
pub use provider_credentials::{
    resolve_anthropic_auth, resolve_bearer_key, resolve_openai_api_key, AnthropicAuth,
    AnthropicAuthSource, OpenAiKeySource, OPENAI_API_KEY_ENV,
};
pub use provider_error::{ProviderError, ProviderErrorKind};
pub use provider_factory::{ImageProvider, ImageSize, ProviderBuildContext, ProviderBuildOutcome};
pub use provider_preferences::{PROVIDER_CREDENTIAL_SERVICES, PROVIDER_PREFERENCE_KEYS};
pub use provider_selection::{select_provider, ProviderSelection, ProviderSelectionInputs};
pub use reasoning::{supported_efforts, EFFORT_LADDER};
pub use routing::{RoutingBackends, RoutingProvider};
pub use tools::{
    chat_tail_tools, get_default_tools, get_image_generation_tool, get_notification_tool,
    get_save_thread_image_tool, get_view_image_tool, ToolCapabilities,
};
pub use unconfigured::{UnconfiguredProvider, NO_PROVIDER_MESSAGE};
pub use vertex::VertexProvider;
pub use web_search::{WebSearchChain, WebSearchProvider, NO_SEARCH_BACKEND};

use std::time::Duration;

pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

pub const OPENROUTER_API_KEY_ENV: &str = "LUCIDOS_OPENROUTER_API_KEY";

pub const XAI_BASE_URL: &str = "https://api.x.ai/v1";

/// INVARIANT: OpenCode's Zen relay, whose free tier answers anonymously. The one base URL
/// reached with no credential: the relay rejects an unrecognized bearer, so the
/// provider is built with an empty key and sends no `Authorization` header.
pub const OPENCODE_FREE_BASE_URL: &str = "https://opencode.ai/zen/v1";

pub const ANTHROPIC_API_BASE_URL: &str = "https://api.anthropic.com/v1";

pub fn lucidos_user_agent() -> String {
    format!("Lucidos/{}", env!("CARGO_PKG_VERSION"))
}

pub const MAX_RETRIES: u32 = 3;

pub fn thinking_budget_for_effort(effort: &str) -> u32 {
    match effort {
        "low" => 4096,
        "medium" => 8192,
        "high" => 16384,
        "xhigh" => 24576,
        "max" => 32768,
        _ => 16384,
    }
}

pub fn is_retryable_status(status_code: u16) -> bool {
    status_code == 429 || status_code == 529 || status_code >= 500
}

fn contains_status_token(haystack: &str, code: &str) -> bool {
    haystack
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|tok| tok == code)
}

pub fn is_transient_error(err: &str) -> bool {
    if spend_cap::is_spend_cap_text(err) {
        return false;
    }
    let lower = err.to_lowercase();
    lower.contains("error sending request")
        || lower.contains("connection reset")
        || lower.contains("connection closed")
        || lower.contains("stream truncated")
        || lower.contains("broken pipe")
        || lower.contains("timed out")
        || lower.contains("temporarily unavailable")
        || lower.contains("network error")
        || lower.contains("rate limit")
        || lower.contains("overloaded")
        || contains_status_token(&lower, "529")
        || contains_status_token(&lower, "503")
        || contains_status_token(&lower, "502")
}

pub fn is_capacity_error(err: &str) -> bool {
    if spend_cap::is_spend_cap_text(err) {
        return false;
    }
    const PHRASES: &[&str] = &[
        "rate limit",
        "rate_limit",
        "too many requests",
        "overloaded",
        "resource_exhausted",
        "resource exhausted",
        "quota",
        "timed out",
    ];
    let lower = err.to_lowercase();
    PHRASES.iter().any(|p| lower.contains(p))
        || ["429", "529", "503"]
            .iter()
            .any(|code| contains_status_token(&lower, code))
}

pub fn is_retryable_error(err: &str) -> bool {
    if spend_cap::is_spend_cap_text(err) {
        return false;
    }
    if is_transient_error(err) {
        return true;
    }
    let lower = err.to_lowercase();
    lower.contains("error decoding response body")
        || lower.contains("stream read error")
        // INVARIANT: Vertex regional replica drift on Opus 4.7's `adaptive` thinking type.
        // Same call succeeds on retry; ~1 in 550 on global endpoint.
        || err.contains("Input tag 'adaptive' found")
}

pub fn should_retry_http(status: u16, body: &str, attempt: u32) -> bool {
    !spend_cap::is_spend_cap_http(status, body)
        && !(200..300).contains(&status)
        && (is_retryable_status(status)
            || ProviderError::from_http("", status, body)
                .kind
                .is_retryable()
            || is_retryable_error(body))
        && attempt <= MAX_RETRIES
}

pub fn retry_delay(attempt: u32, base_secs: u64) -> Duration {
    Duration::from_secs(base_secs << (attempt - 1))
}

pub fn log_retry(model: &str, reason: &str, attempt: u32, delay: Duration) {
    log_legacy!(
        "[{}] {} (attempt {}/{}), retrying in {:?}...",
        model,
        reason,
        attempt,
        MAX_RETRIES + 1,
        delay
    );
}

pub(crate) async fn retry_after_stream_error(
    model: &str,
    err: &(dyn std::error::Error + Send + Sync + 'static),
    attempt: u32,
) -> bool {
    if !error_is_retryable(err) || attempt > MAX_RETRIES {
        return false;
    }
    let delay = retry_delay(attempt, 2);
    log_retry(model, &format!("Stream error: {}", err), attempt, delay);
    tokio::time::sleep(delay).await;
    true
}

pub(crate) fn error_is_retryable(err: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    match err.downcast_ref::<ProviderError>() {
        Some(ProviderError {
            kind: ProviderErrorKind::Unknown(_),
            message,
            ..
        }) => is_retryable_error(message),
        Some(provider) => provider.kind.is_retryable(),
        None => is_retryable_error(&err.to_string()),
    }
}

pub(crate) fn stream_failure(
    err: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    rendered_any: bool,
    provider_tag: &str,
) -> Box<dyn std::error::Error + Send + Sync> {
    let err = err.into();
    if !rendered_any || !error_is_retryable(&*err) {
        return err;
    }
    crate::log_legacy!(
        "[{}] Stream failed after text had streamed, so not retrying: {}",
        provider_tag,
        err
    );
    STREAM_FAILED_AFTER_TEXT.into()
}

const STREAM_FAILED_AFTER_TEXT: &str = "The reply was cut off after it had started \
     streaming. Retrying would render that text twice, so the turn stops here.";

/// INVARIANT: Whether `err` is a [`stream_failure`] that stopped a turn after text had
/// rendered. The cause was transient, but the wording must not read as
/// retryable, so a caller deduplicating transient failures asks this too.
pub fn is_stream_cut_after_text(err: &str) -> bool {
    err.contains(STREAM_FAILED_AFTER_TEXT)
}

pub fn with_retry_context(err: impl std::fmt::Display, attempts: u32) -> String {
    if attempts > 1 {
        format!("{} (after {} attempts)", err, attempts)
    } else {
        err.to_string()
    }
}

pub(crate) fn retry_failure(
    err: Box<dyn std::error::Error + Send + Sync>,
    attempts: u32,
) -> Box<dyn std::error::Error + Send + Sync> {
    match err.downcast::<ProviderError>() {
        Ok(provider) => Box::new(provider.with_attempts(attempts)),
        Err(other) => with_retry_context(other, attempts).into(),
    }
}

/// INVARIANT: Wall-clock budget for the connect + request + response-headers phase of a
/// streaming LLM call. The streaming *body* is bounded separately by each
/// provider's per-chunk timeout inside `parse_*_stream`; the `streaming_client`
/// is deliberately built WITHOUT an overall `.timeout()` so long valid streams
/// aren't capped — which left THIS pre-stream phase unbounded. A black-holed
/// connection there (TCP established, but the server never returns response
/// headers) hung a chat turn forever: `send().await` never resolved, the
/// agentic loop's `tokio::select!` only races the user's Stop button, so the
/// response task never returned and the thread sat `running` with no terminal
/// event. Bounding it converts that hang into a normal retryable network error.
pub(crate) const STREAM_HEADER_TIMEOUT_SECS: u64 = 120;

pub(crate) enum StreamSend {
    Got(reqwest::Response),
    Retry,
    Failed(Box<dyn std::error::Error + Send + Sync>),
}

pub(crate) async fn send_streaming_request(
    builder: reqwest::RequestBuilder,
    model: &str,
    attempt: u32,
    attempt_timeout: Option<Duration>,
) -> StreamSend {
    send_streaming_request_with_timeout(
        cap_attempt(builder, attempt_timeout),
        model,
        attempt,
        Duration::from_secs(STREAM_HEADER_TIMEOUT_SECS),
    )
    .await
}

/// INVARIANT: Cap one HTTP attempt, the body included, at `attempt_timeout`.
///
/// Per request rather than per client, because the router's backends are
/// shared by turns and auxiliary calls. The request's timeout overrides the
/// client's, so a 900s client still gives up within an auxiliary budget.
pub(crate) fn cap_attempt(
    builder: reqwest::RequestBuilder,
    attempt_timeout: Option<Duration>,
) -> reqwest::RequestBuilder {
    match attempt_timeout {
        Some(timeout) => builder.timeout(timeout),
        None => builder,
    }
}

async fn send_streaming_request_with_timeout(
    builder: reqwest::RequestBuilder,
    model: &str,
    attempt: u32,
    header_timeout: Duration,
) -> StreamSend {
    call_trace::attempt_began();
    let err: Box<dyn std::error::Error + Send + Sync> =
        match tokio::time::timeout(header_timeout, builder.send()).await {
            Ok(Ok(resp)) => {
                call_trace::response_received(
                    resp.status().as_u16(),
                    call_trace::request_id_of(resp.headers()).as_deref(),
                );
                return StreamSend::Got(resp);
            }
            Ok(Err(e)) => {
                call_trace::transport_failed(if e.is_timeout() {
                    call_trace::CallFailure::Timeout
                } else {
                    call_trace::CallFailure::Connection
                });
                Box::new(e)
            }
            // INVARIANT: The message keeps "error sending request" and "timed out". So
            // `is_transient_error` calls it retryable, like a real transport stall,
            // and trigger failure notifications stay quiet.
            Err(_elapsed) => {
                call_trace::transport_failed(call_trace::CallFailure::Timeout);
                format!(
                "error sending request: stream timed out after {:?} waiting for response headers",
                header_timeout
            )
                .into()
            }
        };
    if attempt <= MAX_RETRIES {
        let delay = retry_delay(attempt, 1);
        log_retry(model, &format!("Network error: {}", err), attempt, delay);
        tokio::time::sleep(delay).await;
        StreamSend::Retry
    } else {
        StreamSend::Failed(with_retry_context(err, attempt).into())
    }
}

pub fn clamp_provider_token_count(n: u64, source: &str) -> u32 {
    match u32::try_from(n) {
        Ok(v) => v,
        Err(_) => {
            log_legacy!(
                "[{}] Token count {} exceeds u32::MAX; clamping (likely corrupt upstream usage block)",
                source,
                n
            );
            u32::MAX
        }
    }
}

/// INVARIANT: Append an SSE byte chunk to `out`, holding back a character the transport
/// split in two.
///
/// `bytes_stream` yields whatever the socket read, so a multi-byte character
/// can straddle two chunks. Decoding each chunk alone replaces both halves with
/// U+FFFD, which silently corrupts the model's text. Complete characters go to
/// `out` and the incomplete tail waits in `carry` for the next chunk. A byte
/// that can never start a character is replaced, as `from_utf8_lossy` would.
pub(crate) fn push_utf8_chunk(carry: &mut Vec<u8>, chunk: &[u8], out: &mut String) {
    carry.extend_from_slice(chunk);
    loop {
        let (valid, bad_len) = match std::str::from_utf8(carry.as_slice()) {
            Ok(decoded) => {
                out.push_str(decoded);
                carry.clear();
                return;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                out.push_str(&String::from_utf8_lossy(&carry[..valid]));
                (valid, e.error_len())
            }
        };
        match bad_len {
            Some(bad) => {
                out.push(char::REPLACEMENT_CHARACTER);
                carry.drain(..valid + bad);
            }
            None => {
                carry.drain(..valid);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_retryable_error_network_errors() {
        assert!(is_retryable_error(
            "error sending request for url https://example.com/streamRawPredict"
        ));
        assert!(is_retryable_error("connection reset by peer"));
        assert!(is_retryable_error(
            "Connection closed before message completed"
        ));
        assert!(is_retryable_error("broken pipe"));
        assert!(is_retryable_error("request timed out"));
        assert!(is_retryable_error("Resource temporarily unavailable"));
    }

    #[test]
    fn test_is_retryable_error_api_errors() {
        assert!(is_retryable_error("rate limit exceeded"));
        assert!(is_retryable_error("Rate limit exceeded"));
        assert!(is_retryable_error("overloaded"));
        assert!(is_retryable_error(
            "Claude streaming error [overloaded]: overloaded"
        ));
        assert!(is_retryable_error("error decoding response body"));
        assert!(is_retryable_error("Stream read error: connection lost"));
    }

    #[test]
    fn test_is_retryable_error_non_retryable() {
        assert!(!is_retryable_error("invalid JSON in request"));
        assert!(!is_retryable_error("authentication failed"));
        assert!(!is_retryable_error("unknown tool: foo"));
    }

    #[test]
    fn test_is_retryable_error_vertex_adaptive_validation_blip() {
        let body = r#"Claude API error (400 Bad Request): {"type":"error","error":{"type":"invalid_request_error","message":"thinking: Input tag 'adaptive' found using 'type' does not match any of the expected tags: 'disabled', 'enabled'"},"request_id":"req_vrtx_011CaRQ1MT6t454hXfteqBXp"}"#;
        assert!(
            is_retryable_error(body),
            "intermittent adaptive-thinking validation 400 must be retryable"
        );
    }

    #[test]
    fn a_stream_cut_after_text_is_recognised_but_never_retried() {
        let err = stream_failure("Stream read error: reset".to_string(), true, "Test").to_string();
        assert!(is_stream_cut_after_text(&with_retry_context(&err, 2)));
        assert!(!is_retryable_error(&err));
        assert!(!is_stream_cut_after_text("Stream read error: reset"));
    }

    #[test]
    fn test_is_transient_error() {
        assert!(is_transient_error("error sending request for url"));
        assert!(is_transient_error("connection reset by peer"));
        assert!(is_transient_error("request timed out"));
        assert!(is_transient_error("rate limit exceeded"));
        assert!(is_transient_error("HTTP 529 overloaded"));
        assert!(is_transient_error("HTTP 503 service unavailable"));
        assert!(is_transient_error(
            "Claude stream truncated: Vertex closed the stream before message_delta"
        ));
        assert!(!is_transient_error("invalid JSON"));
        assert!(!is_transient_error("authentication failed"));
    }

    #[test]
    fn test_is_transient_error_status_token_not_substring() {
        assert!(!is_transient_error("invalid request id 529abc1234"));
        assert!(!is_transient_error("trace 502xy"));
        assert!(!is_transient_error("rpc code 5031 not found"));
        assert!(!is_transient_error("port 1529 closed"));
        assert!(is_transient_error("(529): server overloaded"));
        assert!(is_transient_error("status=502"));
        assert!(is_transient_error("got 503,"));
    }

    #[test]
    fn a_capacity_error_is_a_rate_limit_an_overload_or_a_timeout() {
        for err in [
            "API error 429: Too Many Requests",
            "rate_limit_exceeded",
            "HTTP 529 overloaded_error",
            "RESOURCE_EXHAUSTED: quota exceeded for model",
            "status 503",
            "timed out after 120s",
        ] {
            assert!(is_capacity_error(err), "{err}");
        }
        for err in [
            "API error 401: invalid x-api-key",
            "API error 400: prompt is too long",
            "request id 4291abc",
            "the model returned an empty line",
        ] {
            assert!(!is_capacity_error(err), "{err}");
        }
    }

    #[test]
    fn test_with_retry_context() {
        assert_eq!(
            with_retry_context("connection failed", 1),
            "connection failed"
        );
        assert_eq!(
            with_retry_context("connection failed", 3),
            "connection failed (after 3 attempts)"
        );
    }

    #[tokio::test]
    async fn a_stream_error_stops_retrying_when_spent_or_not_transient() {
        let spent: Box<dyn std::error::Error + Send + Sync> = "connection reset by peer".into();
        assert!(
            !retry_after_stream_error("test-model", &*spent, MAX_RETRIES + 1).await,
            "attempts are spent"
        );
        let auth: Box<dyn std::error::Error + Send + Sync> = "authentication failed".into();
        assert!(
            !retry_after_stream_error("test-model", &*auth, 1).await,
            "an auth failure is not transient"
        );
    }

    fn provider_error(kind: ProviderErrorKind) -> Box<dyn std::error::Error + Send + Sync> {
        Box::new(ProviderError {
            provider: "Acme".to_string(),
            kind,
            code: None,
            message: "words that name no retry phrase".to_string(),
            status: None,
            attempts: 0,
        })
    }

    #[test]
    fn a_provider_error_retries_by_its_kind() {
        assert!(error_is_retryable(&*provider_error(
            ProviderErrorKind::Internal
        )));
        assert!(!error_is_retryable(&*provider_error(
            ProviderErrorKind::InvalidRequest
        )));
        let plain: Box<dyn std::error::Error + Send + Sync> = "connection reset".into();
        assert!(
            error_is_retryable(&*plain),
            "other errors keep the text rule"
        );
    }

    #[test]
    fn a_retry_failure_keeps_a_provider_error_typed() {
        let failure = retry_failure(provider_error(ProviderErrorKind::Overloaded), 4);
        let typed = failure.downcast_ref::<ProviderError>().expect("typed");
        assert_eq!(typed.attempts, 4);
        assert!(failure.to_string().ends_with("(after 4 attempts)"));
        let other = retry_failure("connection failed".into(), 3);
        assert_eq!(other.to_string(), "connection failed (after 3 attempts)");
    }

    #[test]
    fn a_provider_error_after_rendered_text_is_cut_off() {
        let err = stream_failure(provider_error(ProviderErrorKind::Internal), true, "Test");
        assert!(is_stream_cut_after_text(&err.to_string()));
        let fresh = stream_failure(provider_error(ProviderErrorKind::Internal), false, "Test");
        assert!(fresh.is::<ProviderError>());
        let refused = stream_failure(provider_error(ProviderErrorKind::Permission), true, "Test");
        assert!(refused.is::<ProviderError>());
    }

    #[test]
    fn test_retry_delay_exponential_backoff() {
        assert_eq!(retry_delay(1, 1), Duration::from_secs(1));
        assert_eq!(retry_delay(2, 1), Duration::from_secs(2));
        assert_eq!(retry_delay(3, 1), Duration::from_secs(4));
        assert_eq!(retry_delay(1, 2), Duration::from_secs(2));
        assert_eq!(retry_delay(2, 2), Duration::from_secs(4));
    }

    #[tokio::test]
    async fn stream_send_header_timeout_does_not_hang() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });

        let url = format!("http://{}/", addr);
        let builder = reqwest::Client::new().post(&url).body("{}");
        let outcome = send_streaming_request_with_timeout(
            builder,
            "test-model",
            MAX_RETRIES + 1,
            Duration::from_millis(50),
        )
        .await;

        match outcome {
            StreamSend::Failed(e) => {
                let msg = e.to_string();
                let lower = msg.to_lowercase();
                assert!(
                    lower.contains("timed out") && lower.contains("header"),
                    "expected a header-phase timeout error, got: {msg}"
                );
                assert!(
                    is_transient_error(&msg),
                    "a header-phase timeout must classify as transient/retryable so it routes through retry then a clean ResponseFailed, got: {msg}"
                );
            }
            StreamSend::Got(_) => panic!("black-hole endpoint must not return a response"),
            StreamSend::Retry => panic!("attempt > MAX_RETRIES must Fail, not Retry"),
        }
    }

    #[tokio::test]
    async fn an_auxiliary_attempt_gives_up_within_its_cap() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });

        let builder = reqwest::Client::new()
            .post(format!("http://{}/", addr))
            .body("{}");
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            send_streaming_request(
                builder,
                "test-model",
                MAX_RETRIES + 1,
                Some(Duration::from_millis(200)),
            ),
        )
        .await
        .expect("the attempt cap must end the attempt long before the header bound");
        assert!(matches!(outcome, StreamSend::Failed(_)));
    }

    #[test]
    fn only_a_capped_call_carries_a_request_timeout() {
        let request = |cap| {
            cap_attempt(reqwest::Client::new().post("http://localhost/"), cap)
                .build()
                .expect("build the request")
        };
        assert_eq!(request(None).timeout(), None);
        let cap = Duration::from_secs(20);
        assert_eq!(request(Some(cap)).timeout(), Some(&cap));
    }

    #[test]
    fn a_character_split_across_two_chunks_survives() {
        let text = "hei på deg 😀 日本語";
        let bytes = text.as_bytes();
        for split in 1..bytes.len() {
            let mut carry = Vec::new();
            let mut out = String::new();
            push_utf8_chunk(&mut carry, &bytes[..split], &mut out);
            push_utf8_chunk(&mut carry, &bytes[split..], &mut out);
            assert_eq!(out, text, "split at byte {split}");
            assert!(carry.is_empty(), "nothing held back, split at byte {split}");
        }
    }

    #[test]
    fn an_invalid_byte_still_becomes_the_replacement_character() {
        let mut carry = Vec::new();
        let mut out = String::new();
        push_utf8_chunk(&mut carry, b"a\xffb", &mut out);
        assert_eq!(out, "a\u{fffd}b");
        assert!(carry.is_empty());
    }

    #[test]
    fn test_is_retryable_status() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(529));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
        assert!(!is_retryable_status(404));
    }

    #[test]
    fn a_successful_response_is_never_retried_whatever_its_body_says() {
        let body = r#"{"text":"the rate limit timed out","promptTokenCount": 503}"#;
        assert!(is_retryable_error(body));
        for status in [200, 201, 204, 299] {
            assert!(!should_retry_http(status, body, 1), "status {status}");
        }
        assert!(should_retry_http(400, body, 1));
    }

    #[test]
    fn a_body_naming_a_retryable_kind_retries_on_a_client_status() {
        let overloaded = r#"{"type":"error","error":{"type":"overloaded_error","message":"x"}}"#;
        assert!(should_retry_http(400, overloaded, 1));
        let invalid = r#"{"type":"error","error":{"type":"invalid_request_error","message":"x"}}"#;
        assert!(!should_retry_http(400, invalid, 1));
    }
}
