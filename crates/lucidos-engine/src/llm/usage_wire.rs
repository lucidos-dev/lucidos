//! A provider's usage block, read from a raw response body.
//!
//! For callers that hold the bytes rather than an [`LlmResponse`]: a web
//! search backend, and the credentialed proxy. Each maps the block the way the
//! chat providers map their own, so a row prices the same whichever path made
//! the call.
//!
//! [`LlmResponse`]: crate::llm::provider::LlmResponse

use serde_json::Value;

/// The tokens one call reported, in the engine's `ApiUsage` convention:
/// `input_tokens` counts every prompt token the model processed, cached or
/// not, and the two cache counts are parts of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProviderUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
}

/// The usage a JSON body reports, in whichever provider's shape it carries.
///
/// - Gemini: `usageMetadata`, with thinking billed as output.
/// - OpenAI Chat Completions: `usage.prompt_tokens`.
/// - OpenAI Responses, Anthropic Messages: `usage.input_tokens`. Anthropic
///   reports cache reads and writes apart from it, so they are added back.
///   OpenAI reports both inside it, writes from GPT-5.6 on.
///
/// `None` when the body names no prompt tokens, so a reply that is not a
/// model call never reads as one that cost nothing.
pub(crate) fn from_json(body: &Value) -> Option<ProviderUsage> {
    read(body).filter(|u| u.input_tokens > 0)
}

/// [`from_json`] without the prompt check, for one frame of a stream: a
/// frame may carry only the output count.
fn read(body: &Value) -> Option<ProviderUsage> {
    if let Some(meta) = body.get("usageMetadata") {
        let output = count(meta, "candidatesTokenCount").unwrap_or(0)
            + count(meta, "thoughtsTokenCount").unwrap_or(0);
        return Some(usage(
            count(meta, "promptTokenCount").unwrap_or(0),
            output,
            0,
            0,
        ));
    }
    let block = body.get("usage")?;
    if let Some(prompt) = count(block, "prompt_tokens") {
        let cached = block
            .pointer("/prompt_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        return Some(usage(
            prompt,
            count(block, "completion_tokens").unwrap_or(0),
            cached,
            0,
        ));
    }
    let input = count(block, "input_tokens");
    let output = count(block, "output_tokens");
    if input.is_none() && output.is_none() {
        return None;
    }
    let (input, output) = (input.unwrap_or(0), output.unwrap_or(0));
    let cache_write = count(block, "cache_creation_input_tokens").unwrap_or(0);
    let cache_read = count(block, "cache_read_input_tokens").unwrap_or(0);
    let openai_detail = |field: &str| {
        block
            .pointer(&format!("/input_tokens_details/{field}"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Some(usage(
        input.saturating_add(cache_write).saturating_add(cache_read),
        output,
        cache_read + openai_detail("cached_tokens"),
        cache_write + openai_detail("cache_write_tokens"),
    ))
}

/// The usage a stream sent as one JSON array reports, as Gemini's
/// `streamGenerateContent` does without `alt=sse`. Each chunk is one frame.
pub(crate) fn from_chunks(chunks: &[Value]) -> Option<ProviderUsage> {
    fold(chunks.iter().filter_map(read))
}

/// The usage a buffered server-sent-event stream reports.
///
/// Each provider spreads its counters over frames: Anthropic puts the prompt
/// in `message_start` and the output in `message_delta`, OpenAI Responses its
/// totals in `response.completed`. The counters only grow, so the largest
/// value of each across every frame is the call's total.
pub(crate) fn from_event_stream(body: &str) -> Option<ProviderUsage> {
    let frames = body
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
        .flat_map(|frame| {
            [
                read(&frame),
                frame.get("message").and_then(read),
                frame.get("response").and_then(read),
            ]
        })
        .flatten();
    fold(frames)
}

/// The counters only grow, so the largest of each across the frames is the
/// call's total. `None` when no frame named a prompt token.
fn fold(frames: impl Iterator<Item = ProviderUsage>) -> Option<ProviderUsage> {
    frames
        .reduce(|a, b| ProviderUsage {
            input_tokens: a.input_tokens.max(b.input_tokens),
            output_tokens: a.output_tokens.max(b.output_tokens),
            cache_read_tokens: a.cache_read_tokens.max(b.cache_read_tokens),
            cache_creation_tokens: a.cache_creation_tokens.max(b.cache_creation_tokens),
        })
        .filter(|u| u.input_tokens > 0)
}

fn count(block: &Value, key: &str) -> Option<u64> {
    block.get(key).and_then(Value::as_u64)
}

fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> ProviderUsage {
    let clamp = |n| crate::llm::clamp_provider_token_count(n, "usage block");
    ProviderUsage {
        input_tokens: clamp(input),
        output_tokens: clamp(output),
        cache_read_tokens: clamp(cache_read),
        cache_creation_tokens: clamp(cache_write),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn counts(u: ProviderUsage) -> (u32, u32, u32, u32) {
        (
            u.input_tokens,
            u.output_tokens,
            u.cache_read_tokens,
            u.cache_creation_tokens,
        )
    }

    #[test]
    fn anthropic_adds_its_cache_back_into_the_prompt() {
        let body = json!({"usage": {"input_tokens": 10, "output_tokens": 5,
            "cache_read_input_tokens": 100, "cache_creation_input_tokens": 20}});
        assert_eq!(counts(from_json(&body).unwrap()), (130, 5, 100, 20));
    }

    #[test]
    fn openai_counts_its_cached_share_inside_the_prompt() {
        let chat = json!({"usage": {"prompt_tokens": 50, "completion_tokens": 7,
            "prompt_tokens_details": {"cached_tokens": 30}}});
        assert_eq!(counts(from_json(&chat).unwrap()), (50, 7, 30, 0));
        let responses = json!({"usage": {"input_tokens": 40, "output_tokens": 3,
            "input_tokens_details": {"cached_tokens": 12}}});
        assert_eq!(counts(from_json(&responses).unwrap()), (40, 3, 12, 0));
        let written = json!({"usage": {"input_tokens": 40, "output_tokens": 3,
            "input_tokens_details": {"cached_tokens": 12, "cache_write_tokens": 20}}});
        assert_eq!(counts(from_json(&written).unwrap()), (40, 3, 12, 20));
    }

    #[test]
    fn gemini_bills_thinking_as_output() {
        let body = json!({"usageMetadata": {"promptTokenCount": 90,
            "candidatesTokenCount": 8, "thoughtsTokenCount": 4}});
        assert_eq!(counts(from_json(&body).unwrap()), (90, 12, 0, 0));
    }

    /// A body that is not a model call reports nothing, never a free call.
    #[test]
    fn a_body_without_prompt_tokens_reports_nothing() {
        assert!(from_json(&json!({"quota": {"remaining": 3}})).is_none());
        assert!(from_json(&json!({"usage": {"output_tokens": 3}})).is_none());
    }

    #[test]
    fn an_anthropic_stream_joins_its_prompt_and_output_frames() {
        let stream = "event: message_start\n\
            data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"output_tokens\":1}}}\n\n\
            event: message_delta\n\
            data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":40}}\n\n";
        assert_eq!(counts(from_event_stream(stream).unwrap()), (12, 40, 0, 0));
    }

    #[test]
    fn a_responses_stream_reads_its_completed_frame() {
        let stream = "data: {\"type\":\"response.created\",\"response\":{}}\n\n\
            data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":9,\"output_tokens\":2}}}\n\n";
        assert_eq!(counts(from_event_stream(stream).unwrap()), (9, 2, 0, 0));
    }
}
