pub mod anthropic;
pub mod anthropic_wire;
pub mod image;
pub mod mock;
pub mod model_registry;
pub mod openai;
pub mod provider;
pub mod routing;
pub mod tool_names;
pub mod tools;
pub mod validate;
pub mod vertex;

pub use anthropic::{AnthropicAuth, AnthropicProvider};
pub use image::{ImageProvider, ImageSize};
pub use model_registry::{ModelRegistry, ProviderKind};
pub use openai::{
    resolve_bearer_key, resolve_openai_api_key, OpenAiKeySource, OpenAiProvider,
    OPENAI_DEFAULT_BASE_URL,
};
pub use provider::{ContentBlock, LlmProvider, Message, MessageContent, TokenCallback, ToolCall};
pub use routing::RoutingProvider;
pub use tools::{
    get_default_tools, get_image_generation_tool, get_manage_repositories_tool, get_mcp_tools,
    get_navigate_ui_tool, get_notification_tool, get_read_notifications_tool,
    get_save_thread_image_tool,
};
pub use vertex::VertexProvider;

use std::time::Duration;

/// Max retry attempts for LLM API calls (shared across providers).
pub const MAX_RETRIES: u32 = 3;

/// Map a unified `reasoning_effort` string to the thinking-budget token count
/// shared by the Claude `budget_tokens` field (Vertex + direct Anthropic) and
/// the Gemini-3 `thinkingConfig.thinkingBudget`. Unknown values fall back to
/// the "high" budget — the default each call site picked independently before
/// this was DRYed up. Provider-neutral, so it lives here rather than in any one
/// provider module.
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
