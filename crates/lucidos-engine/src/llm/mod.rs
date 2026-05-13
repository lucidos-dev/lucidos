pub mod image;
pub mod mock;
pub mod openai;
pub mod provider;
pub mod routing;
pub mod tool_names;
pub mod tools;
pub mod vertex;

pub use image::{ImageProvider, ImageSize};
pub use openai::OpenAiProvider;
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

/// Whether an HTTP status code is retryable (429 rate limit, 529 overload, 5xx server error).
pub fn is_retryable_status(status_code: u16) -> bool {
    status_code == 429 || status_code == 529 || status_code >= 500
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
        || lower.contains("529")
        || lower.contains("503")
        || lower.contains("502")
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
