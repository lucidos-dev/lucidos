pub mod anthropic;
pub mod anthropic_wire;
pub mod image;
pub mod mock;
pub mod model_registry;
pub mod openai;
pub mod provider;
pub mod provider_build;
pub mod provider_selection;
pub mod routing;
pub mod tool_names;
pub mod tools;
pub mod unconfigured;
pub mod validate;
pub mod vertex;
pub mod web_search;

pub use anthropic::{
    resolve_anthropic_auth, AnthropicAuth, AnthropicAuthSource, AnthropicProvider,
};
pub use image::{ImageProvider, ImageSize};
pub use model_registry::{ModelRegistry, ProviderKind};
pub use openai::{
    resolve_bearer_key, resolve_openai_api_key, OpenAiKeySource, OpenAiProvider,
    OPENAI_DEFAULT_BASE_URL,
};
pub use provider::{ContentBlock, LlmProvider, Message, MessageContent, TokenCallback, ToolCall};
pub use provider_build::{
    boot_without_provider_enabled, build_active_provider, ProviderBuildContext,
    ProviderBuildOutcome, PROVIDER_CREDENTIAL_SERVICES,
};
pub use provider_selection::{select_provider, ProviderSelection, ProviderSelectionInputs};
pub use routing::RoutingProvider;
pub use tools::{
    get_default_tools, get_image_generation_tool, get_navigate_ui_tool, get_notification_tool,
    get_save_thread_image_tool, get_view_image_tool,
};
pub use unconfigured::{UnconfiguredProvider, NO_PROVIDER_MESSAGE};
pub use vertex::VertexProvider;
pub use web_search::{WebSearchChain, WebSearchProvider, NO_SEARCH_BACKEND};

use std::time::Duration;

/// OpenRouter's OpenAI-compatible API root. Single home shared by the LLM
/// provider (`provider_build`) and the builtin `openrouter` proxy
/// (`api::proxy_builtin`).
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Direct Anthropic API root (`{base}/messages`). Single home shared by the
/// LLM provider (`anthropic::chat`) and the builtin `anthropic` proxy
/// (`api::proxy_builtin`).
pub const ANTHROPIC_API_BASE_URL: &str = "https://api.anthropic.com/v1";

/// Max retry attempts for LLM API calls (shared across providers).
pub const MAX_RETRIES: u32 = 3;

/// Map a unified `reasoning_effort` string to the Claude thinking `budget_tokens`
/// value (Vertex + direct Anthropic). Unknown values fall back to the "high"
/// budget — the default each call site picked independently before this was
/// DRYed up. (Gemini 3.x no longer uses a budget — it maps effort to
/// `thinkingConfig.thinkingLevel` in `vertex::gemini::gemini_thinking_level`.)
pub(crate) fn thinking_budget_for_effort(effort: &str) -> u32 {
    match effort {
        "low" => 4096,
        "medium" => 8192,
        "high" => 16384,
        "xhigh" => 24576,
        "max" => 32768,
        _ => 16384,
    }
}

/// Whether an HTTP status code is retryable (429 rate limit, 529 overload, 5xx server error).
pub fn is_retryable_status(status_code: u16) -> bool {
    status_code == 429 || status_code == 529 || status_code >= 500
}

/// True if `code` appears in `haystack` as a standalone alphanumeric token —
/// "HTTP 529" matches, "request id 529abc..." and "1529" do not. Used by
/// `is_transient_error` to keep the HTTP-status heuristic from false-positiving
/// on opaque identifiers.
fn contains_status_token(haystack: &str, code: &str) -> bool {
    haystack
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|tok| tok == code)
}

/// Whether an error is a transient network/infrastructure issue (not a logic or auth error).
/// Used to suppress noisy duplicate notifications for triggers.
pub fn is_transient_error(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("error sending request")
        || lower.contains("connection reset")
        || lower.contains("connection closed")
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

/// Whether a stream/parse error message indicates a retryable condition.
/// Superset of `is_transient_error` — also includes stream parsing errors and
/// known intermittent validation blips from regional API replicas (rare 400s
/// that succeed on the next attempt).
pub fn is_retryable_error(err: &str) -> bool {
    if is_transient_error(err) {
        return true;
    }
    let lower = err.to_lowercase();
    lower.contains("server_error")
        || lower.contains("error decoding response body")
        || lower.contains("stream read error")
        // Vertex regional replica drift on Opus 4.7's `adaptive` thinking type.
        // Same call succeeds on retry; ~1 in 550 on global endpoint.
        || err.contains("Input tag 'adaptive' found")
}

/// Whether an HTTP response should trigger a retry — combines status-code and
/// body checks (covers transient 5xx/429/529 plus known 400 validation blips
/// that succeed on retry) and bounds attempts at `MAX_RETRIES`.
pub fn should_retry_http(status: u16, body: &str, attempt: u32) -> bool {
    (is_retryable_status(status) || is_retryable_error(body)) && attempt <= MAX_RETRIES
}

/// Calculate exponential backoff delay for a given attempt (1-indexed).
/// `base_secs` is the starting delay (1 for connect errors, 2 for stream errors).
pub fn retry_delay(attempt: u32, base_secs: u64) -> Duration {
    Duration::from_secs(base_secs << (attempt - 1))
}

/// Log a retry attempt with model context.
pub fn log_retry(model: &str, reason: &str, attempt: u32, delay: Duration) {
    log!(
        "[{}] {} (attempt {}/{}), retrying in {:?}...",
        model,
        reason,
        attempt,
        MAX_RETRIES + 1,
        delay
    );
}

/// Wrap a final error with retry context so logs/notifications show what was attempted.
pub fn with_retry_context(err: impl std::fmt::Display, attempts: u32) -> String {
    if attempts > 1 {
        format!("{} (after {} attempts)", err, attempts)
    } else {
        err.to_string()
    }
}

/// Wall-clock budget for the connect + request + response-headers phase of a
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

/// Outcome of one streaming-request send attempt. Each provider's retry loop
/// matches on it: `Got` → proceed to read the SSE stream; `Retry` → the helper
/// already logged + backed off, the caller does `continue`; `Failed` → the
/// final error after retries are exhausted, the caller returns it.
pub(crate) enum StreamSend {
    Got(reqwest::Response),
    Retry,
    Failed(Box<dyn std::error::Error + Send + Sync>),
}

/// Send a streaming request with the connect+headers phase bounded by
/// `STREAM_HEADER_TIMEOUT_SECS`, folding the four streaming providers' identical
/// network-error retry arm into one place. A transport error OR a header-phase
/// timeout both retry up to `MAX_RETRIES` (both are network stalls), then fail.
/// The caller builds the full `RequestBuilder` (URL + headers + body) and reads
/// the SSE stream itself on `Got` — only the send/retry handling is shared.
pub(crate) async fn send_streaming_request(
    builder: reqwest::RequestBuilder,
    model: &str,
    attempt: u32,
) -> StreamSend {
    send_streaming_request_with_timeout(
        builder,
        model,
        attempt,
        Duration::from_secs(STREAM_HEADER_TIMEOUT_SECS),
    )
    .await
}

/// Inner implementation with the header-phase timeout injected, so tests can
/// drive the timeout path deterministically with a tiny budget instead of
/// waiting `STREAM_HEADER_TIMEOUT_SECS`.
async fn send_streaming_request_with_timeout(
    builder: reqwest::RequestBuilder,
    model: &str,
    attempt: u32,
    header_timeout: Duration,
) -> StreamSend {
    let err: Box<dyn std::error::Error + Send + Sync> =
        match tokio::time::timeout(header_timeout, builder.send()).await {
            Ok(Ok(resp)) => return StreamSend::Got(resp),
            Ok(Err(e)) => Box::new(e),
            // Keep "error sending request" + "timed out" in the message so
            // `is_transient_error` classifies it retryable (suppresses noisy
            // trigger failure notifications) exactly like a real transport stall.
            Err(_elapsed) => format!(
                "error sending request: stream timed out after {:?} waiting for response headers",
                header_timeout
            )
            .into(),
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

/// Clamp an upstream u64 token count (from provider SSE usage blocks) into the
/// u32 our meta/usage structs store. Above-bound values indicate corrupt
/// upstream data; log and clamp rather than panicking or silently truncating
/// so a single bad block can't tank the stream and we don't lose visibility
/// on the corruption. `source` is a short tag (e.g. "OpenAI", "Vertex",
/// "ClaudeCode") used in the log prefix.
pub(crate) fn clamp_provider_token_count(n: u64, source: &str) -> u32 {
    match u32::try_from(n) {
        Ok(v) => v,
        Err(_) => {
            log!(
                "[{}] Token count {} exceeds u32::MAX; clamping (likely corrupt upstream usage block)",
                source,
                n
            );
            u32::MAX
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
        assert!(is_retryable_error("server_error"));
        assert!(is_retryable_error("error decoding response body"));
        assert!(is_retryable_error("Stream read error: connection lost"));
    }

    #[test]
    fn test_is_retryable_error_non_retryable() {
        assert!(!is_retryable_error("invalid JSON in request"));
        assert!(!is_retryable_error("authentication failed"));
        assert!(!is_retryable_error("unknown tool: foo"));
    }

    /// Vertex regional replicas occasionally return HTTP 400 with this body
    /// when they haven't picked up the latest schema for Opus 4.7's `adaptive`
    /// thinking type. The same call usually succeeds on retry. Observed rate
    /// after switching to vertex_region=global: ~1 in 550 calls.
    #[test]
    fn test_is_retryable_error_vertex_adaptive_validation_blip() {
        let body = r#"Claude API error (400 Bad Request): {"type":"error","error":{"type":"invalid_request_error","message":"thinking: Input tag 'adaptive' found using 'type' does not match any of the expected tags: 'disabled', 'enabled'"},"request_id":"req_vrtx_011CaRQ1MT6t454hXfteqBXp"}"#;
        assert!(
            is_retryable_error(body),
            "intermittent adaptive-thinking validation 400 must be retryable"
        );
    }

    #[test]
    fn test_is_transient_error() {
        assert!(is_transient_error("error sending request for url"));
        assert!(is_transient_error("connection reset by peer"));
        assert!(is_transient_error("request timed out"));
        assert!(is_transient_error("rate limit exceeded"));
        assert!(is_transient_error("HTTP 529 overloaded"));
        assert!(is_transient_error("HTTP 503 service unavailable"));
        assert!(!is_transient_error("invalid JSON"));
        assert!(!is_transient_error("authentication failed"));
    }

    /// HTTP status token matching must be word-bounded: "529" inside an opaque
    /// identifier like "request id 529abc..." or a longer number "1529" is not
    /// a status code and must NOT classify the error as transient. Pre-fix
    /// substring matching false-positived on both.
    #[test]
    fn test_is_transient_error_status_token_not_substring() {
        assert!(!is_transient_error("invalid request id 529abc1234"));
        assert!(!is_transient_error("trace 502xy"));
        assert!(!is_transient_error("rpc code 5031 not found"));
        assert!(!is_transient_error("port 1529 closed"));
        // Standalone status tokens still match, regardless of surrounding punctuation.
        assert!(is_transient_error("(529): server overloaded"));
        assert!(is_transient_error("status=502"));
        assert!(is_transient_error("got 503,"));
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

    #[test]
    fn test_retry_delay_exponential_backoff() {
        assert_eq!(retry_delay(1, 1), Duration::from_secs(1));
        assert_eq!(retry_delay(2, 1), Duration::from_secs(2));
        assert_eq!(retry_delay(3, 1), Duration::from_secs(4));
        assert_eq!(retry_delay(1, 2), Duration::from_secs(2));
        assert_eq!(retry_delay(2, 2), Duration::from_secs(4));
    }

    /// Reproduction for the stuck-thread bug: a streaming send to a black-holed
    /// endpoint — connection accepted but response headers NEVER sent — must be
    /// bounded by the header-phase timeout and surface as a retryable network
    /// error, not hang forever. Without `send_streaming_request_with_timeout`'s
    /// `tokio::time::timeout` wrap this `await` never returns (the
    /// `streaming_client` has no `.timeout()`, and the per-chunk stream timeout
    /// only engages AFTER headers arrive), which is exactly what left a chat
    /// thread `running` with no terminal event.
    #[tokio::test]
    async fn stream_send_header_timeout_does_not_hang() {
        // Black-hole: accept connections and hold them open, never replying.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream); // keep each socket open + silent
            }
        });

        let url = format!("http://{}/", addr);
        // attempt > MAX_RETRIES so the helper returns Failed immediately (no backoff sleep).
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
}
