use super::*;
use serde_json::json;

const ALL_KINDS: [fn() -> ProviderErrorKind; 10] = [
    || ProviderErrorKind::Internal,
    || ProviderErrorKind::Overloaded,
    || ProviderErrorKind::RateLimited,
    || ProviderErrorKind::Timeout,
    || ProviderErrorKind::RequestTooLarge,
    || ProviderErrorKind::InvalidRequest,
    || ProviderErrorKind::Authentication,
    || ProviderErrorKind::Permission,
    || ProviderErrorKind::NotFound,
    || ProviderErrorKind::Unknown("odd_code".to_string()),
];

fn error_of(kind: ProviderErrorKind) -> ProviderError {
    ProviderError {
        provider: "Acme".to_string(),
        kind,
        code: Some("some_code".to_string()),
        message: "raw words".to_string(),
        status: None,
        attempts: 0,
    }
}

#[test]
fn the_seed_frame_is_an_internal_error_that_retries() {
    let frame = json!({"type": "api_error", "message": "upstream error"});
    let error = ProviderError::from_stream_error("Anthropic", &frame);
    assert_eq!(error.kind, ProviderErrorKind::Internal);
    assert!(error.kind.is_retryable());
    assert_eq!(
        error.with_attempts(4).to_string(),
        "Anthropic error [api_error]: upstream error. Anthropic had an internal error and is \
         temporarily unavailable. Your request did not cause it. (after 4 attempts)"
    );
}

#[test]
fn every_kind_renders_the_raw_text_and_a_meaning() {
    let raw = "Acme error [some_code]: raw words. ";
    for kind in ALL_KINDS {
        let text = error_of(kind()).to_string();
        assert!(text.starts_with(raw), "{text}");
        assert!(text.len() > raw.len() + 20, "{text}");
    }
}

#[test]
fn only_the_capacity_and_server_kinds_retry() {
    for kind in ALL_KINDS {
        let kind = kind();
        let expected = matches!(
            kind,
            ProviderErrorKind::Internal
                | ProviderErrorKind::Overloaded
                | ProviderErrorKind::RateLimited
                | ProviderErrorKind::Timeout
        );
        assert_eq!(kind.is_retryable(), expected, "{kind:?}");
    }
}

#[test]
fn a_retryable_error_still_reads_as_transient_text() {
    for kind in ALL_KINDS {
        let kind = kind();
        let text = error_of(kind.clone()).to_string();
        assert_eq!(
            crate::llm::is_transient_error(&text),
            kind.is_retryable(),
            "{text}"
        );
    }
}

#[test]
fn an_unknown_code_keeps_its_raw_text() {
    let frame = json!({"type": "brand_new_error", "message": "something odd"});
    let error = ProviderError::from_stream_error("Anthropic", &frame);
    assert_eq!(
        error.kind,
        ProviderErrorKind::Unknown("brand_new_error".to_string())
    );
    let text = error.to_string();
    assert!(text.contains("[brand_new_error]"), "{text}");
    assert!(text.contains("something odd"), "{text}");
    assert!(text.contains("does not recognise"), "{text}");
    assert!(!error.kind.is_retryable());
}

/// A local server or proxy often sends a bare message with no code. Its words
/// still decide the retry, as they did before the kind existed.
#[test]
fn an_unknown_kind_retries_on_transient_words() {
    let retries = |message: &str| {
        let error: Box<dyn std::error::Error + Send + Sync> = Box::new(
            ProviderError::from_stream_error("Local server", &json!({"message": message})),
        );
        crate::llm::error_is_retryable(&*error)
    };
    assert!(retries("Rate limit exceeded"));
    assert!(retries("server overloaded"));
    assert!(!retries("model name is not valid"));
}

#[test]
fn a_known_kind_decides_the_retry_over_its_words() {
    let error: Box<dyn std::error::Error + Send + Sync> =
        Box::new(ProviderError::from_stream_error(
            "OpenAI",
            &json!({"type": "invalid_request_error", "message": "rate limit field is invalid"}),
        ));
    assert!(!crate::llm::error_is_retryable(&*error));
}

#[test]
fn anthropic_http_body_reads_its_type() {
    let body = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
    let error = ProviderError::from_http("Anthropic", 529, body);
    assert_eq!(error.kind, ProviderErrorKind::Overloaded);
    assert_eq!(error.status, Some(529));
    assert!(error
        .to_string()
        .starts_with("Anthropic error (529) [overloaded_error]: Overloaded. "));
}

#[test]
fn openai_http_body_reads_its_type_when_the_code_is_null() {
    let body =
        r#"{"error":{"message":"The server had an error","type":"server_error","code":null}}"#;
    let error = ProviderError::from_http("OpenAI", 500, body);
    assert_eq!(error.kind, ProviderErrorKind::Internal);
    assert_eq!(error.code.as_deref(), Some("server_error"));
}

#[test]
fn gemini_http_body_reads_its_status() {
    let body =
        r#"{"error":{"code":503,"message":"The model is overloaded.","status":"UNAVAILABLE"}}"#;
    let error = ProviderError::from_http("Gemini", 503, body);
    assert_eq!(error.kind, ProviderErrorKind::Overloaded);
    assert_eq!(error.status, Some(503));
    assert_eq!(error.code.as_deref(), Some("UNAVAILABLE"));
}

#[test]
fn a_gemini_array_envelope_is_unwrapped() {
    let body = r#"[{"error":{"code":429,"message":"Quota","status":"RESOURCE_EXHAUSTED"}}]"#;
    let error = ProviderError::from_http("Gemini", 429, body);
    assert_eq!(error.kind, ProviderErrorKind::RateLimited);
}

#[test]
fn a_plain_text_body_falls_back_to_the_status() {
    let error = ProviderError::from_http("Claude gateway", 502, "<html>Bad Gateway</html>");
    assert_eq!(error.kind, ProviderErrorKind::Overloaded);
    assert!(error.to_string().contains("<html>Bad Gateway</html>"));
    let teapot = ProviderError::from_http("Acme", 418, "");
    assert_eq!(
        teapot.kind,
        ProviderErrorKind::Unknown("HTTP 418".to_string())
    );
    assert!(teapot.to_string().contains("empty response body"));
}

#[test]
fn a_numeric_code_on_a_relay_frame_is_the_status() {
    let frame = json!({"code": 502, "message": "Provider returned error"});
    let error = ProviderError::from_stream_error("OpenAI", &frame);
    assert_eq!(error.status, Some(502));
    assert_eq!(error.kind, ProviderErrorKind::Overloaded);
    assert_eq!(error.code.as_deref(), Some("502"));
}

#[test]
fn an_unknown_code_on_a_known_status_takes_the_status_kind() {
    let body = r#"{"error":{"type":"brand_new_error","message":"x"}}"#;
    let error = ProviderError::from_http("Acme", 500, body);
    assert_eq!(error.kind, ProviderErrorKind::Internal);
    assert_eq!(error.code.as_deref(), Some("brand_new_error"));
}

#[test]
fn a_code_that_repeats_the_status_prints_once() {
    let frame = json!({"code": 502, "message": "Provider returned error"});
    let text = ProviderError::from_stream_error("OpenAI", &frame).to_string();
    assert!(
        text.starts_with("OpenAI error (502): Provider returned error. "),
        "{text}"
    );
}

#[test]
fn an_empty_type_falls_through_to_the_code() {
    let frame = json!({"type": "", "code": "server_error", "message": "x"});
    let error = ProviderError::from_stream_error("OpenAI", &frame);
    assert_eq!(error.code.as_deref(), Some("server_error"));
    assert_eq!(error.kind, ProviderErrorKind::Internal);
}

/// Files under `llm/` that format an HTTP status into an error and are not a
/// model call: sign-in, token refresh, discovery, model-list and telemetry
/// endpoints, and the prefill probe, which has its own blessed type
/// (`PrefillRefused`).
const NOT_A_MODEL_CALL: &[&str] = &[
    "claude_gateway/",
    "chatgpt_plan/id_token.rs",
    "chatgpt_plan/served_models.rs",
    "chatgpt_plan/token_endpoint.rs",
    "prefill.rs",
    "vertex/adc.rs",
];

/// Default-deny: a model call turns a failed answer into a `ProviderError`
/// through `from_http` or `from_stream_error`. An error built from a status or
/// from "API error" wording in a `format!` is the shape this type replaced.
#[test]
fn no_model_call_formats_its_own_provider_error() {
    let llm = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/llm");
    let statement =
        regex::Regex::new(r"(?s)Err\(\s*format!\((.*?)\)\s*\.into\(\)\)").expect("a regex");
    let status_wording =
        regex::Regex::new(r"(?i)status|api error|streaming error|returned \{").expect("a regex");
    for prefix in NOT_A_MODEL_CALL {
        assert!(llm.join(prefix).exists(), "stale allowlist entry {prefix}");
    }
    let mut hits = Vec::new();
    let mut stack = vec![llm.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("a readable dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path
                .strip_prefix(&llm)
                .expect("under llm")
                .to_string_lossy();
            let is_source = rel.ends_with(".rs") && !rel.ends_with("_tests.rs");
            let allowed = NOT_A_MODEL_CALL.iter().any(|p| rel.starts_with(p));
            if !is_source || allowed || rel == "provider_error.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a readable file");
            let production = text.split("#[cfg(test)]").next().unwrap_or_default();
            for found in statement.captures_iter(production) {
                if status_wording.is_match(&found[1]) {
                    hits.push(format!("{rel}: {}", found[1].trim()));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "build these through ProviderError::from_http or from_stream_error \
         (.claude/rules/provider-errors.md):\n{}",
        hits.join("\n")
    );
}

#[test]
fn a_single_attempt_adds_no_attempt_count() {
    let text = error_of(ProviderErrorKind::Internal)
        .with_attempts(1)
        .to_string();
    assert!(!text.contains("attempts"), "{text}");
}
