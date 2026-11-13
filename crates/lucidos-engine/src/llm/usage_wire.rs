//! A provider's usage block, read into an [`ApiUsage`].
//!
//! The one reader of each provider's usage shape. The chat providers, the
//! Claude Code readers, the keep-alive, the web search backends and the proxy
//! all call it. So no path can drop a field another path reads.

use serde_json::Value;

use crate::llm::api_usage::ApiUsage;
use crate::llm::cache_lifetime::CacheLifetime;

/// The usage a JSON body reports, in whichever provider's shape it carries.
///
/// - Gemini: `usageMetadata`, with thinking billed as output. Its cached
///   count is a part of `promptTokenCount`, and Gemini bills no cache write.
/// - Any other provider: its `usage` block, read by [`from_block`].
///
/// `None` when the body names no prompt tokens, so a reply that is not a
/// model call never reads as one that cost nothing.
pub(crate) fn from_json(body: &Value) -> Option<ApiUsage> {
    from_frame(body).filter(|u| u.input_tokens > 0)
}

/// [`from_json`] without the prompt check, for one frame of a stream: a
/// frame may carry only the output count.
pub(crate) fn from_frame(body: &Value) -> Option<ApiUsage> {
    if let Some(meta) = body.get("usageMetadata") {
        let output = count(meta, "candidatesTokenCount").unwrap_or(0)
            + count(meta, "thoughtsTokenCount").unwrap_or(0);
        return Some(usage(
            count(meta, "promptTokenCount").unwrap_or(0),
            output,
            count(meta, "cachedContentTokenCount").unwrap_or(0),
            0,
        ));
    }
    let echoed = body
        .pointer("/prompt_cache_options/ttl")
        .and_then(|ttl| serde_json::from_value(ttl.clone()).ok());
    read_block(body.get("usage")?, echoed)
}

/// A bare `usage` object, in the engine's `ApiUsage` convention:
/// `input_tokens` counts every prompt token, and both cache counts are parts
/// of it.
///
/// - OpenAI Chat Completions: `prompt_tokens`, with cache reads and writes
///   inside it (writes from OpenRouter's Anthropic models).
/// - OpenAI Responses, Anthropic Messages: `input_tokens`. Anthropic reports
///   cache reads and writes apart from it, so they are added back. OpenAI
///   reports both inside it, writes from GPT-5.6 on.
///
/// `None` when the block names neither a prompt nor an output count.
pub(crate) fn from_block(block: &Value) -> Option<ApiUsage> {
    read_block(block, None)
}

/// [`from_block`], with the `prompt_cache_options.ttl` an OpenAI Responses
/// body echoes beside the block.
fn read_block(block: &Value, echoed: Option<CacheLifetime>) -> Option<ApiUsage> {
    let prompt = count(block, "prompt_tokens");
    let completion = count(block, "completion_tokens");
    if prompt.is_some() || completion.is_some() {
        let details = block.get("prompt_tokens_details");
        return Some(with_openai_split(
            usage(
                prompt.unwrap_or(0),
                completion.unwrap_or(0),
                detail(details, "cached_tokens"),
                detail(details, "cache_write_tokens"),
            ),
            details,
            echoed,
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
    let details = block.get("input_tokens_details");
    let read = usage(
        input.saturating_add(cache_write).saturating_add(cache_read),
        output,
        cache_read.saturating_add(detail(details, "cached_tokens")),
        cache_write.saturating_add(detail(details, "cache_write_tokens")),
    );
    Some(with_openai_split(
        ApiUsage {
            cache_creation_1h_tokens: anthropic_1h_part(block),
            ..read
        },
        details,
        echoed,
    ))
}

/// The 1-hour part of an Anthropic cache write. The rest of the write went to
/// the 5-minute cache.
fn anthropic_1h_part(block: &Value) -> Option<u32> {
    block
        .pointer("/cache_creation/ephemeral_1h_input_tokens")
        .and_then(Value::as_u64)
        .map(clamp)
}

/// `read` with the 30-minute and 12-hour parts of an OpenAI cache write.
///
/// The split in the `*_tokens_details` object wins, zero parts included: a
/// zero part is evidence. A split that names more tokens than the write is
/// dropped, so its lifetime stays unknown rather than claim tokens nobody
/// wrote. With no part named (no object, `null` or `{}`), the `echoed` TTL
/// names the whole write's lifetime.
fn with_openai_split(
    read: ApiUsage,
    details: Option<&Value>,
    echoed: Option<CacheLifetime>,
) -> ApiUsage {
    let written = read.cache_creation_tokens;
    if written == 0 {
        return read;
    }
    let split = details.and_then(|d| d.get("cache_write_tokens_details"));
    let part = |field: &str| split.and_then(|s| count(s, field)).map(clamp);
    let (split_30m, split_12h) = match (part("ttl_30m_tokens"), part("ttl_12h_tokens")) {
        (None, None) => match echoed {
            Some(CacheLifetime::ThirtyMinutes) => (Some(written), None),
            Some(CacheLifetime::TwelveHours) => (None, Some(written)),
            _ => return read,
        },
        (split_30m, split_12h) => {
            let named = u64::from(split_30m.unwrap_or(0)) + u64::from(split_12h.unwrap_or(0));
            if named > u64::from(written) {
                return read;
            }
            (split_30m, split_12h)
        }
    };
    ApiUsage {
        cache_creation_30m_tokens: split_30m,
        cache_creation_12h_tokens: split_12h,
        ..read
    }
}

/// One count from a `*_tokens_details` object, `0` when absent.
fn detail(details: Option<&Value>, field: &str) -> u64 {
    details.and_then(|d| count(d, field)).unwrap_or(0)
}

/// The usage a stream sent as one JSON array reports, as Gemini's
/// `streamGenerateContent` does without `alt=sse`. Each chunk is one frame.
pub(crate) fn from_chunks(chunks: &[Value]) -> Option<ApiUsage> {
    fold(chunks.iter().filter_map(from_frame))
}

/// The usage a buffered server-sent-event stream reports.
///
/// Each provider spreads its counters over frames: Anthropic puts the prompt
/// in `message_start` and the output in `message_delta`, OpenAI Responses its
/// totals in `response.completed`.
pub(crate) fn from_event_stream(body: &str) -> Option<ApiUsage> {
    let frames = body
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
        .flat_map(|frame| {
            [
                from_frame(&frame),
                frame.get("message").and_then(from_frame),
                frame.get("response").and_then(from_frame),
            ]
        })
        .flatten();
    fold(frames)
}

/// The frames merged into the call's total. `None` when no frame named a
/// prompt token.
fn fold(frames: impl Iterator<Item = ApiUsage>) -> Option<ApiUsage> {
    frames.reduce(merge).filter(|u| u.input_tokens > 0)
}

/// Two frames of one call, merged. The counters only grow, so the larger of
/// each is the call's total, and a split part a frame omits keeps the other
/// frame's.
pub(crate) fn merge(a: ApiUsage, b: ApiUsage) -> ApiUsage {
    let part = |a: Option<u32>, b: Option<u32>| a.max(b);
    ApiUsage {
        input_tokens: a.input_tokens.max(b.input_tokens),
        output_tokens: a.output_tokens.max(b.output_tokens),
        cache_read_tokens: a.cache_read_tokens.max(b.cache_read_tokens),
        cache_creation_tokens: a.cache_creation_tokens.max(b.cache_creation_tokens),
        modality: b.modality.or(a.modality),
        service_tier: b.service_tier.or(a.service_tier),
        cache_creation_1h_tokens: part(a.cache_creation_1h_tokens, b.cache_creation_1h_tokens),
        cache_creation_30m_tokens: part(a.cache_creation_30m_tokens, b.cache_creation_30m_tokens),
        cache_creation_12h_tokens: part(a.cache_creation_12h_tokens, b.cache_creation_12h_tokens),
        cache_creation_requested_ttl: b
            .cache_creation_requested_ttl
            .or(a.cache_creation_requested_ttl),
    }
}

fn count(block: &Value, key: &str) -> Option<u64> {
    block.get(key).and_then(Value::as_u64)
}

fn clamp(n: u64) -> u32 {
    crate::llm::clamp_provider_token_count(n, "usage block")
}

fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> ApiUsage {
    ApiUsage {
        input_tokens: clamp(input),
        output_tokens: clamp(output),
        cache_read_tokens: clamp(cache_read),
        cache_creation_tokens: clamp(cache_write),
        ..ApiUsage::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn counts(u: ApiUsage) -> (u32, u32, u32, u32) {
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

    /// OpenRouter reports an Anthropic cache write inside `prompt_tokens`, as it
    /// does the read.
    #[test]
    fn chat_completions_counts_its_cache_write_inside_the_prompt() {
        let chat = json!({"usage": {"prompt_tokens": 50, "completion_tokens": 7,
            "prompt_tokens_details": {"cached_tokens": 10, "cache_write_tokens": 25}}});
        assert_eq!(counts(from_json(&chat).unwrap()), (50, 7, 10, 25));
    }

    #[test]
    fn gemini_bills_thinking_as_output() {
        let body = json!({"usageMetadata": {"promptTokenCount": 90,
            "candidatesTokenCount": 8, "thoughtsTokenCount": 4}});
        assert_eq!(counts(from_json(&body).unwrap()), (90, 12, 0, 0));
    }

    /// `promptTokenCount` already counts the cached part, as in the chat path.
    #[test]
    fn gemini_counts_its_cache_hit_inside_the_prompt() {
        let body = json!({"usageMetadata": {"promptTokenCount": 5000,
            "cachedContentTokenCount": 4096, "candidatesTokenCount": 3}});
        assert_eq!(counts(from_json(&body).unwrap()), (5000, 3, 4096, 0));
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

    /// The write's lifetime parts: 1 hour, 30 minutes, 12 hours.
    fn split_of(usage: Option<ApiUsage>) -> (Option<u32>, Option<u32>, Option<u32>) {
        let u = usage.expect("usage");
        (
            u.cache_creation_1h_tokens,
            u.cache_creation_30m_tokens,
            u.cache_creation_12h_tokens,
        )
    }

    #[test]
    fn an_anthropic_body_keeps_its_write_split() {
        let body = json!({"usage": {"input_tokens": 10, "output_tokens": 5,
            "cache_creation_input_tokens": 20,
            "cache_creation": {"ephemeral_1h_input_tokens": 20, "ephemeral_5m_input_tokens": 0}}});
        assert_eq!(split_of(from_json(&body)), (Some(20), None, None));
    }

    #[test]
    fn an_anthropic_stream_keeps_its_write_split() {
        let stream = "event: message_start\n\
            data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":30,\"cache_creation\":{\"ephemeral_1h_input_tokens\":30},\"output_tokens\":1}}}\n\n\
            event: message_delta\n\
            data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":40}}\n\n";
        assert_eq!(split_of(from_event_stream(stream)), (Some(30), None, None));
    }

    #[test]
    fn a_responses_body_keeps_its_write_split() {
        let body = json!({"usage": {"input_tokens": 40, "output_tokens": 3,
            "input_tokens_details": {"cached_tokens": 0, "cache_write_tokens": 20,
                "cache_write_tokens_details": {"ttl_30m_tokens": 0, "ttl_12h_tokens": 20}}}});
        assert_eq!(split_of(from_json(&body)), (None, Some(0), Some(20)));
    }

    /// A split that claims more than the write is no evidence, echo or not.
    #[test]
    fn an_over_summed_responses_split_is_dropped() {
        let body = json!({"prompt_cache_options": {"ttl": "30m"},
            "usage": {"input_tokens": 40, "output_tokens": 3,
            "input_tokens_details": {"cache_write_tokens": 20,
                "cache_write_tokens_details": {"ttl_30m_tokens": 15, "ttl_12h_tokens": 15}}}});
        let usage = from_json(&body).unwrap();
        assert_eq!(usage.cache_creation_30m_tokens, None);
        assert_eq!(usage.cache_creation_12h_tokens, None);
    }

    #[test]
    fn the_echoed_ttl_names_a_write_with_no_split() {
        let body = json!({"prompt_cache_options": {"ttl": "12h"},
            "usage": {"input_tokens": 40, "output_tokens": 3,
            "input_tokens_details": {"cache_write_tokens": 20}}});
        assert_eq!(split_of(from_json(&body)), (None, None, Some(20)));
    }

    /// A split object that names no part is no split, so the echo still counts.
    #[test]
    fn the_echoed_ttl_names_a_write_with_an_empty_split() {
        for split in [json!(null), json!({})] {
            let body = json!({"prompt_cache_options": {"ttl": "12h"},
                "usage": {"input_tokens": 40, "output_tokens": 3,
                "input_tokens_details": {"cache_write_tokens": 20,
                    "cache_write_tokens_details": split}}});
            assert_eq!(
                from_json(&body).unwrap().cache_creation_12h_tokens,
                Some(20)
            );
        }
    }

    /// A later frame that omits the split keeps the one an earlier frame
    /// reported.
    #[test]
    fn a_merge_keeps_a_split_one_frame_omits() {
        let start = from_block(
            &json!({"input_tokens": 2, "cache_creation_input_tokens": 30,
            "cache_creation": {"ephemeral_1h_input_tokens": 30}, "output_tokens": 1}),
        )
        .unwrap();
        let delta = from_block(&json!({"output_tokens": 40})).unwrap();
        let merged = merge(start, delta);
        assert_eq!(counts(merged), (32, 40, 0, 30));
        assert_eq!(merged.cache_creation_1h_tokens, Some(30));
    }

    /// The `[DONE]` line after the usage frame is not JSON, so it is skipped
    /// rather than ending the parse.
    #[test]
    fn a_speech_stream_reads_its_done_frame() {
        let stream = "data: {\"type\":\"speech.audio.delta\",\"audio\":\"SUQz\"}\n\n\
            data: {\"type\":\"speech.audio.done\",\"usage\":{\"input_tokens\":51,\"output_tokens\":420,\"total_tokens\":471}}\n\n\
            data: [DONE]\n\n";
        assert_eq!(counts(from_event_stream(stream).unwrap()), (51, 420, 0, 0));
    }
}
