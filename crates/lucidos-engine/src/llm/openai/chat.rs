//! OpenAI Chat Completions API path (non-codex models) for `OpenAiProvider`.
//! The struct, shared stream types, and dispatch live in the parent `openai`
//! module.

use crate::llm::anthropic_wire::{ephemeral_cache_marker, MAX_CACHE_BREAKPOINTS};
use crate::llm::cache_lifetime::CacheLifetime;
use crate::llm::prefill::{pair_of, BeatShape, PrefillPair, StableEnd, TurnRoute};
use crate::llm::provider::{
    ContentBlock, LlmResponse, Message, MessageContent, TokenCallback, ToolDefinition,
};
use crate::llm::ModelSelection;
use futures::StreamExt;
use std::collections::BTreeSet;
use std::time::Duration;

use super::{
    AccumulatedToolCall, OpenAiProvider, StreamMeta, CHUNK_TIMEOUT_SECS,
    DEFAULT_MAX_COMPLETION_TOKENS, STREAM_LOG_TAG,
};

/// OpenRouter's namespace for Anthropic models. It is the one model family on
/// this wire that caches only what carries a `cache_control` marker.
const ANTHROPIC_NAMESPACE: &str = "anthropic/";

/// What a text part is, for the cache marker's sake.
#[derive(Clone, Copy, PartialEq)]
enum PartKind {
    Body,
    /// Volatile engine notes, rewritten every round. Never a marker anchor.
    Tail,
    View,
}

struct TextPart {
    text: String,
    kind: PartKind,
}

impl TextPart {
    fn new(text: &str, kind: PartKind) -> Self {
        Self {
            text: text.to_string(),
            kind,
        }
    }
}

/// The text and image parts of one user or assistant wire message, kept apart
/// so a cache marker can land on a single part. A `tool` message is never one:
/// it takes a plain string.
struct Parts {
    texts: Vec<TextPart>,
    images: Vec<serde_json::Value>,
    /// Indexes into [`Parts::cacheable_texts`] that carry a marker.
    marks: BTreeSet<usize>,
}

impl Parts {
    fn new(texts: Vec<TextPart>, images: Vec<serde_json::Value>) -> Self {
        Self {
            texts,
            images,
            marks: BTreeSet::new(),
        }
    }

    /// The text parts the API accepts: it rejects an empty one.
    fn cacheable_texts(&self) -> Vec<&TextPart> {
        self.texts.iter().filter(|t| !t.text.is_empty()).collect()
    }

    /// The part a message-level marker lands on: the last that is not a tail.
    /// `None` for a message with nothing but tail text, such as the one left
    /// when a round's tool results split off into `tool` messages. The marker
    /// then goes to the message before it.
    fn anchor(&self) -> Option<usize> {
        self.cacheable_texts()
            .iter()
            .rposition(|t| t.kind != PartKind::Tail)
    }

    fn views(&self) -> Vec<usize> {
        self.cacheable_texts()
            .iter()
            .enumerate()
            .filter(|(_, t)| t.kind == PartKind::View)
            .map(|(i, _)| i)
            .collect()
    }

    /// Mark one part. Returns whether a new marker landed, so a part marked
    /// twice counts once against the cap.
    fn mark(&mut self, at: usize) -> bool {
        self.marks.insert(at)
    }

    /// The content every model but Anthropic gets: the text joined into one
    /// string, or one text part beside the images.
    fn joined_content(&self) -> Option<serde_json::Value> {
        let joined = (!self.texts.is_empty()).then(|| {
            self.texts
                .iter()
                .map(|t| t.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        });
        if self.images.is_empty() {
            return joined.map(serde_json::Value::String);
        }
        let mut content: Vec<serde_json::Value> = joined
            .map(|text| serde_json::json!({ "type": "text", "text": text }))
            .into_iter()
            .collect();
        content.extend(self.images.iter().cloned());
        Some(serde_json::Value::Array(content))
    }

    /// The content an Anthropic model gets: one part per text block, so a
    /// marker lands on exactly one block. The blocks stay the same with or
    /// without a marker, because a cache entry is keyed on them. One unmarked
    /// text stays a plain string, which renders as the same single block.
    fn split_content(&self) -> Option<serde_json::Value> {
        let texts = self.cacheable_texts();
        if texts.is_empty() && self.images.is_empty() {
            return self.joined_content();
        }
        if texts.len() == 1 && self.images.is_empty() && self.marks.is_empty() {
            return Some(serde_json::Value::String(texts[0].text.clone()));
        }
        let mut content: Vec<serde_json::Value> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let mut part = serde_json::json!({ "type": "text", "text": t.text });
                if self.marks.contains(&i) {
                    part["cache_control"] = ephemeral_cache_marker();
                }
                part
            })
            .collect();
        content.extend(self.images.iter().cloned());
        Some(serde_json::Value::Array(content))
    }
}

/// One Chat Completions wire message. `value` holds everything but the
/// content of a message that has [`Parts`].
struct ChatMessage {
    value: serde_json::Value,
    parts: Option<Parts>,
}

impl ChatMessage {
    fn plain(value: serde_json::Value) -> Self {
        Self { value, parts: None }
    }

    fn with_parts(value: serde_json::Value, parts: Parts) -> Self {
        Self {
            value,
            parts: Some(parts),
        }
    }

    fn into_value(mut self, split_text: bool) -> serde_json::Value {
        if let Some(parts) = &self.parts {
            let content = if split_text {
                parts.split_content()
            } else {
                parts.joined_content()
            };
            if let Some(content) = content {
                self.value["content"] = content;
            }
        }
        self.value
    }
}

/// Place cache markers on an Anthropic request, in the order
/// `anthropic_wire` uses: the last message that can hold one, the one before
/// it, each memory view, then the system block. At most
/// [`MAX_CACHE_BREAKPOINTS`]. Tool definitions and `tool` messages take none.
/// Returns whether the system block took a marker.
fn place_cache_markers(messages: &mut [ChatMessage], has_system: bool) -> bool {
    let anchored: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.parts.as_ref().is_some_and(|p| p.anchor().is_some()))
        .map(|(i, _)| i)
        .collect();

    let mut breakpoints = 0;
    for &i in anchored.iter().rev().take(2) {
        if let Some(parts) = messages[i].parts.as_mut() {
            if let Some(anchor) = parts.anchor() {
                breakpoints += usize::from(parts.mark(anchor));
            }
        }
    }
    'views: for &i in &anchored {
        let Some(parts) = messages[i].parts.as_mut() else {
            continue;
        };
        for view in parts.views() {
            if breakpoints == MAX_CACHE_BREAKPOINTS {
                break 'views;
            }
            breakpoints += usize::from(parts.mark(view));
        }
    }
    has_system && breakpoints < MAX_CACHE_BREAKPOINTS
}

/// The last memory view that took a marker, as `(message, part)`. Views sit
/// in fixed places, so the next turn marks the same one. The rolling marks
/// on the last two messages move every turn, so they never end a stable cut.
fn last_marked_view(messages: &[ChatMessage]) -> Option<(usize, usize)> {
    messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| {
            let parts = message.parts.as_ref()?;
            if message.value["role"] != "user" {
                return None;
            }
            let part = parts
                .views()
                .into_iter()
                .rev()
                .find(|view| parts.marks.contains(view))?;
            Some((index, part))
        })
}

/// The most output a Chat Completions prefill allows. Probe P4 found that
/// OpenRouter ignores `max_tokens: 0`, and honours 1.
const CHAT_OUTPUT_CAP: u32 = 1;

/// The pair a beat on a Chat Completions request sends, cut at `end`.
fn prefill_pair(
    body: &serde_json::Value,
    end: StableEnd,
    route: TurnRoute<'_>,
) -> Option<PrefillPair> {
    let mut value = body.clone();
    let object = value.as_object_mut()?;
    object.remove("max_completion_tokens");
    object.remove("stream_options");
    object.insert("max_tokens".into(), CHAT_OUTPUT_CAP.into());
    object.insert("stream".into(), false.into());
    pair_of(
        value,
        "messages",
        end,
        BeatShape::json_reply(CHAT_OUTPUT_CAP),
        route,
    )
}

/// A Chat Completions request, and where a beat's stable cut on it ends.
struct ChatBody {
    body: serde_json::Value,
    /// `None` where a beat may not go: a model outside the Anthropic
    /// namespace, or a request with no marker to read.
    stable_end: Option<StableEnd>,
}

impl OpenAiProvider {
    /// Convert internal messages to OpenAI Chat Completions format.
    fn convert_messages_chat(messages: &[Message]) -> Vec<ChatMessage> {
        let mut openai_messages: Vec<ChatMessage> = Vec::new();

        for msg in messages {
            match &msg.content {
                MessageContent::Text(text) => {
                    openai_messages.push(ChatMessage::with_parts(
                        serde_json::json!({ "role": msg.role }),
                        Parts::new(vec![TextPart::new(text, PartKind::Body)], Vec::new()),
                    ));
                }
                MessageContent::Blocks(blocks) => {
                    let mut text_parts: Vec<TextPart> = Vec::new();
                    let mut image_parts: Vec<serde_json::Value> = Vec::new();
                    let mut tool_calls: Vec<serde_json::Value> = Vec::new();
                    let mut tool_results: Vec<(String, String)> = Vec::new();

                    for block in blocks {
                        match block {
                            // A tail block is ordinary text here. Only the
                            // engine and the cache markers care who wrote it.
                            ContentBlock::Text { text } | ContentBlock::StableContext { text } => {
                                text_parts.push(TextPart::new(text, PartKind::Body));
                            }
                            ContentBlock::EngineTail { text } => {
                                text_parts.push(TextPart::new(text, PartKind::Tail));
                            }
                            ContentBlock::MemoryView { text } => {
                                text_parts.push(TextPart::new(text, PartKind::View));
                            }
                            ContentBlock::Image {
                                media_type, data, ..
                            } => {
                                image_parts.push(serde_json::json!({
                                    "type": "image_url",
                                    "image_url": {
                                        "url": format!("data:{};base64,{}", media_type, data),
                                    }
                                }));
                            }
                            ContentBlock::ToolUse {
                                id, name, input, ..
                            } => {
                                tool_calls.push(serde_json::json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": serde_json::to_string(input)
                                            .unwrap_or_else(|e| {
                                                crate::log!("[OpenAI] Failed to serialize tool arguments: {}", e);
                                                "{}".to_string()
                                            }),
                                    }
                                }));
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                            } => {
                                tool_results.push((tool_use_id.clone(), content.clone()));
                            }
                        }
                    }

                    let has_tool_calls = !tool_calls.is_empty();

                    // 1. Assistant `tool_calls` message first — it owns any text
                    //    as its `content` field.
                    if has_tool_calls {
                        let assistant_msg = serde_json::json!({
                            "role": "assistant",
                            "tool_calls": tool_calls,
                        });
                        openai_messages.push(if text_parts.is_empty() {
                            ChatMessage::plain(assistant_msg)
                        } else {
                            ChatMessage::with_parts(
                                assistant_msg,
                                Parts::new(std::mem::take(&mut text_parts), Vec::new()),
                            )
                        });
                    }

                    // 2. Tool-result `tool` messages MUST immediately follow the
                    //    assistant `tool_calls` message — before any user
                    //    text/image content — or strict providers (Moonshot/Kimi,
                    //    OpenAI) reject with HTTP 400 "an assistant message with
                    //    'tool_calls' must be followed by tool messages responding
                    //    to each 'tool_call_id'". The agentic loop packs the
                    //    ToolResult block(s) and a trailing instruction Text into
                    //    ONE user `Message::Blocks`, so emitting the text before
                    //    the tool messages would wedge a `user` message between the
                    //    assistant `tool_calls` and its responses (the observed
                    //    kimi-k3 failure — thread 85239abe).
                    for (tool_call_id, content) in tool_results {
                        openai_messages.push(ChatMessage::plain(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": tool_call_id,
                            "content": content,
                        })));
                    }

                    // 3. Remaining user text/image content, AFTER the tool
                    //    messages. Skipped when this block was an assistant
                    //    `tool_calls` message (step 1 already consumed its text).
                    let has_user_content = !text_parts.is_empty() || !image_parts.is_empty();
                    if !has_tool_calls && has_user_content {
                        openai_messages.push(ChatMessage::with_parts(
                            serde_json::json!({ "role": msg.role }),
                            Parts::new(text_parts, image_parts),
                        ));
                    }
                }
            }
        }

        openai_messages
    }

    /// Convert internal tool definitions to Chat Completions function tool format.
    fn convert_tools_chat(tools: &[ToolDefinition]) -> Option<Vec<serde_json::Value>> {
        if tools.is_empty() {
            return None;
        }
        Some(
            tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect(),
        )
    }

    /// The Chat Completions `tool_choice` that makes the model call `tool`.
    fn forced_tool_choice_chat(tool: &str) -> serde_json::Value {
        serde_json::json!({ "type": "function", "function": { "name": tool } })
    }

    fn build_chat_body(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_prompt: Option<&str>,
        reasoning_effort: Option<&str>,
    ) -> ChatBody {
        // OpenRouter caches an Anthropic model's prompt only where a text part
        // carries `cache_control`. Every other model keeps its body as it was,
        // because a strict or local server can reject a key it does not know.
        let explicit_cache = model.starts_with(ANTHROPIC_NAMESPACE);
        let mut chat_messages = Self::convert_messages_chat(messages);
        let has_system_text = system_prompt.is_some_and(|s| !s.is_empty());
        let mark_system =
            explicit_cache && place_cache_markers(&mut chat_messages, has_system_text);
        // Only an Anthropic model carries markers, so only it gets a cut.
        let system_offset = usize::from(system_prompt.is_some());
        let stable_end = match last_marked_view(&chat_messages) {
            Some((message, part)) => Some(StableEnd::Part {
                item: message + system_offset,
                part,
            }),
            None => mark_system.then_some(StableEnd::Placeholder { keep: 1 }),
        };

        let mut openai_messages = Vec::new();
        if let Some(system) = system_prompt {
            let content = if mark_system {
                serde_json::json!([{
                    "type": "text",
                    "text": system,
                    "cache_control": ephemeral_cache_marker(),
                }])
            } else {
                serde_json::Value::String(system.to_string())
            };
            openai_messages.push(serde_json::json!({
                "role": "system",
                "content": content,
            }));
        }
        openai_messages.extend(
            chat_messages
                .into_iter()
                .map(|message| message.into_value(explicit_cache)),
        );

        let mut body = serde_json::json!({
            "model": model,
            "stream": true,
            // Ask OpenAI to append a final chunk carrying token usage so the
            // provider parser can populate `LlmResponse.{input,output}_tokens`
            // — without this, streaming Chat Completions silently drops
            // usage, breaking cross-provider cost analytics.
            "stream_options": { "include_usage": true },
            "max_completion_tokens": DEFAULT_MAX_COMPLETION_TOKENS,
            "messages": openai_messages,
        });

        if let Some(tool_defs) = Self::convert_tools_chat(tools) {
            body["tools"] = serde_json::Value::Array(tool_defs);
        }

        // Verbatim, deliberately. Which tiers this model supports is decided
        // once, in `llm::reasoning`, and enforced by `RoutingProvider`'s clamp.
        // This builder also serves OpenRouter and local servers, so it cannot
        // tell whose vocabulary applies from the model id alone: rewriting
        // `max` into `xhigh` here because the id was not `gpt-5.6` is what 400'd
        // a local turn on 2026-08-12 (see `llm/reasoning.rs`).
        if let Some(effort) = reasoning_effort {
            body["reasoning_effort"] = serde_json::Value::String(effort.to_string());
        }

        ChatBody { body, stable_end }
    }

    /// Parse an SSE stream from the Chat Completions API.
    async fn parse_chat_stream(
        &self,
        response: reqwest::Response,
        on_token: &Option<TokenCallback>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        // Bytes of a character the transport split across two chunks.
        let mut carry: Vec<u8> = Vec::new();
        let mut content = String::new();
        let mut tool_call_map: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();
        let chunk_timeout = Duration::from_secs(CHUNK_TIMEOUT_SECS);

        'outer: loop {
            let chunk = match tokio::time::timeout(chunk_timeout, stream.next()).await {
                Ok(Some(Ok(bytes))) => bytes,
                Ok(Some(Err(e))) => {
                    return Err(crate::llm::stream_failure(
                        format!("Stream read error: {}", e),
                        on_token.is_some() && !content.is_empty(),
                        STREAM_LOG_TAG,
                    ))
                }
                Ok(None) => break,
                Err(_) => {
                    return Err(crate::llm::stream_failure(
                        format!(
                            "OpenAI stream timed out (no data for {}s)",
                            CHUNK_TIMEOUT_SECS
                        ),
                        on_token.is_some() && !content.is_empty(),
                        STREAM_LOG_TAG,
                    ))
                }
            };

            crate::llm::push_utf8_chunk(&mut carry, &chunk, &mut buffer);

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                if let Some(data_str) = line.strip_prefix("data: ") {
                    if data_str.trim() == "[DONE]" {
                        break 'outer;
                    }

                    let prev_len = content.len();
                    Self::process_chat_chunk(data_str, &mut content, &mut tool_call_map, &mut meta)
                        .map_err(|e| {
                            crate::llm::stream_failure(
                                e.to_string(),
                                on_token.is_some() && prev_len > 0,
                                STREAM_LOG_TAG,
                            )
                        })?;
                    if content.len() > prev_len {
                        if let Some(cb) = on_token {
                            // Floor defensively — `prev_len` is a byte length
                            // captured before the chunk appended, so a future
                            // accumulator change could leave it mid-codepoint.
                            cb(&content[content.floor_char_boundary(prev_len)..]);
                        }
                    }
                }
            }
        }

        Self::build_llm_response(content, tool_call_map, meta)
    }

    /// Process a single Chat Completions SSE chunk. The final usage chunk
    /// (requested via `stream_options.include_usage`) arrives with
    /// `choices: []` and a top-level `usage` object — we extract token
    /// counts from there. `finish_reason` rides the LAST per-choice chunk
    /// alongside (or instead of) the delta payload.
    fn process_chat_chunk(
        data_str: &str,
        content: &mut String,
        tool_call_map: &mut Vec<AccumulatedToolCall>,
        meta: &mut StreamMeta,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let data: serde_json::Value = serde_json::from_str(data_str)?;

        if let Some(error) = data.get("error") {
            return Err(format_stream_error(error).into());
        }

        // Top-level usage object (final include_usage chunk has choices: []).
        if let Some(usage) = data.get("usage") {
            meta.absorb_chat_usage(usage);
        }
        // Every chunk names the model; the first one that does is kept.
        if meta.served_model.is_none() {
            meta.served_model = crate::llm::served_model::served_model_of(&data);
        }

        let choices = match data.get("choices").and_then(|c| c.as_array()) {
            Some(c) => c,
            None => return Ok(()),
        };

        let choice = match choices.first() {
            Some(c) => c,
            None => return Ok(()),
        };

        // finish_reason rides on the final per-choice chunk. May coexist
        // with an empty `delta` or a delta carrying only the role field.
        if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
            meta.stop_reason = Some(reason.to_string());
        }

        let delta = match choice.get("delta") {
            Some(d) => d,
            None => return Ok(()),
        };

        if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
            content.push_str(text);
        }

        if let Some(tc_array) = delta.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tc_array {
                let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;

                // The index comes straight off provider JSON, and an
                // OpenAI-compatible host (OpenRouter, a local server, the keyless
                // relay) is untrusted. A huge index would grow `tool_call_map` to
                // billions of empty entries and OOM the engine. Legitimate indexes
                // are small and contiguous, so anything past the cap is malformed.
                if index >= MAX_STREAMED_TOOL_CALLS {
                    return Err(format!(
                        "provider streamed tool_call index {index}, past the {MAX_STREAMED_TOOL_CALLS} cap"
                    )
                    .into());
                }

                while tool_call_map.len() <= index {
                    tool_call_map.push(AccumulatedToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments_json: String::new(),
                    });
                }

                let accumulated = &mut tool_call_map[index];

                if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                    accumulated.id = id.to_string();
                }

                if let Some(function) = tc.get("function") {
                    if let Some(name) = function.get("name").and_then(|n| n.as_str()) {
                        accumulated.name = name.to_string();
                    }
                    if let Some(args) = function.get("arguments").and_then(|a| a.as_str()) {
                        accumulated.arguments_json.push_str(args);
                    }
                }
            }
        }

        Ok(())
    }

    /// Chat Completions flow with retry logic.
    pub(super) async fn chat_completions(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        selection: ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let model = selection.model.unwrap_or(&self.model);
        let reasoning_effort = selection.reasoning_effort;
        let ChatBody {
            mut body,
            stable_end,
        } = self.build_chat_body(model, messages, tools, system_prompt, reasoning_effort);
        if let Some(tool) = selection.forced_tool {
            body["tool_choice"] = Self::forced_tool_choice_chat(tool);
        }
        // A marker with no `ttl` asks for Anthropic's 5-minute cache.
        let requested_ttl = stable_end.as_ref().map(|_| CacheLifetime::FiveMinutes);
        let prefill = stable_end.and_then(|end| {
            prefill_pair(
                &body,
                end,
                TurnRoute {
                    wire_model: model,
                    reasoning_effort,
                    betas: &[],
                },
            )
        });

        let mut attempt = 0u32;
        loop {
            attempt += 1;

            let builder = self
                .apply_headers(self.streaming_client.post(&self.chat_url))
                .json(&body);
            let resp = match crate::llm::send_streaming_request(
                builder,
                model,
                attempt,
                selection.attempt_timeout,
            )
            .await
            {
                crate::llm::StreamSend::Got(r) => r,
                crate::llm::StreamSend::Retry => continue,
                crate::llm::StreamSend::Failed(e) => return Err(e),
            };

            let status = resp.status();
            if !status.is_success() {
                let error_body = resp.text().await.unwrap_or_default();

                if crate::llm::is_retryable_status(status.as_u16())
                    && attempt <= crate::llm::MAX_RETRIES
                {
                    let delay = crate::llm::retry_delay(attempt, 1);
                    crate::llm::log_retry(model, &format!("HTTP {}", status), attempt, delay);
                    tokio::time::sleep(delay).await;
                    continue;
                }

                return Err(crate::llm::not_served::failure(
                    status.as_u16(),
                    &error_body,
                    crate::llm::with_retry_context(
                        format!("OpenAI API error ({}): {}", status, error_body),
                        attempt,
                    ),
                ));
            }

            match self.parse_chat_stream(resp, &on_token).await {
                Ok(response) => {
                    return Ok(LlmResponse {
                        prefill,
                        cache_creation_requested_ttl: requested_ttl,
                        ..response
                    })
                }
                Err(e) => {
                    let err_str = e.to_string();
                    if crate::llm::retry_after_stream_error(model, &err_str, attempt).await {
                        continue;
                    }
                    return Err(crate::llm::with_retry_context(e, attempt).into());
                }
            }
        }
    }
}

/// Longest provider detail we append. The whole string lands in a
/// `ResponseFailed` payload the UI renders, so an unbounded provider blob
/// would be unreadable there.
const PROVIDER_DETAIL_MAX: usize = 300;

/// Upper bound on a streamed tool-call index before we treat the frame as
/// malformed. Real turns emit a handful of tool calls. This sits far above any
/// legitimate stream and exists only to stop an untrusted host OOMing the
/// engine with a huge index.
const MAX_STREAMED_TOOL_CALLS: usize = 1024;

/// Render one Chat Completions SSE error frame as a single readable line.
///
/// This builder serves OpenRouter and local servers as well as OpenAI, and
/// they disagree about which field names the failure. OpenAI sends `type`;
/// OpenRouter sends `code` plus a `metadata` object and no `type` at all. A
/// `type`-only reader therefore labelled EVERY OpenRouter failure `unknown`
/// and dropped the one field saying why. The Responses path already falls
/// back to `code`, so this is the two paths agreeing rather than a new rule.
fn format_stream_error(error: &serde_json::Value) -> String {
    let label = scalar_label(error.get("type"))
        .or_else(|| scalar_label(error.get("code")))
        .unwrap_or_else(|| "unknown".to_string());

    // A local OpenAI-compatible server often sends `{"error": "..."}` with no
    // object around it. Reading only `error.message` there discarded the only
    // text the frame had and reported "Unknown streaming error".
    let message = error
        .as_str()
        .or_else(|| error.get("message").and_then(|m| m.as_str()))
        .filter(|m| !m.is_empty())
        .unwrap_or("Unknown streaming error");

    match provider_detail(error.get("metadata")) {
        Some(detail) => format!("OpenAI streaming error [{label}]: {message} ({detail})"),
        None => format!("OpenAI streaming error [{label}]: {message}"),
    }
}

/// A JSON scalar as a bare label: `429`, never `"429"`. `None` for absent,
/// null, empty, or a container, so the caller can fall through to the next
/// candidate field.
fn scalar_label(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The upstream cause OpenRouter tucks into `metadata`, as one bounded line.
/// `raw` holds the provider's own error body, which is what explains a
/// message as bare as "ERROR"; `provider_name` says who produced it.
fn provider_detail(metadata: Option<&serde_json::Value>) -> Option<String> {
    let metadata = metadata?.as_object()?;
    let provider = metadata
        .get("provider_name")
        .and_then(|p| p.as_str())
        .filter(|p| !p.is_empty());
    let raw = metadata.get("raw").and_then(collapse_to_one_line);

    let detail = match (provider, raw) {
        (Some(p), Some(r)) => format!("{p}: {r}"),
        (Some(p), None) => p.to_string(),
        (None, Some(r)) => r,
        (None, None) => return None,
    };
    Some(truncate_detail(detail))
}

/// Flatten a `raw` value to one whitespace-normalised line. Providers send it
/// as a string or as a nested object, and either can carry the newlines that
/// would break the single-line error.
///
/// `None` for a null or empty `raw`. A present-but-null one is what a provider
/// sends when it failed with no body, and rendering it appended a literal
/// "(null)" to the error.
fn collapse_to_one_line(value: &serde_json::Value) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let text = match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then_some(collapsed)
}

fn truncate_detail(mut detail: String) -> String {
    if detail.len() > PROVIDER_DETAIL_MAX {
        detail.truncate(detail.floor_char_boundary(PROVIDER_DETAIL_MAX));
        detail.push('…');
    }
    detail
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_stream_rejects_an_out_of_range_tool_call_index() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        // A hostile OpenAI-compatible host sends a tool_call index near u32::MAX.
        // Without the cap this grows `tools` to billions of entries and OOMs.
        let result = OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":4000000000,"id":"x","function":{"name":"f","arguments":"{}"}}]}}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        );
        assert!(result.is_err(), "out-of-range tool_call index must error");
        assert!(tools.is_empty(), "must not grow the tool-call map");
    }

    /// The Chat Completions stream MUST surface `finish_reason` (from the
    /// last per-choice chunk) and token usage (from the final empty-choices
    /// chunk added by `stream_options.include_usage`). Without this, the
    /// OpenAI provider was discarding both, breaking cost/context analytics
    /// parity with Vertex (which captures the same data via `message_start`
    /// / `message_delta`).
    #[test]
    fn chat_stream_captures_finish_reason_and_usage() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        // Mid-stream text chunk: no finish_reason yet, no usage.
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        // Per-choice terminal chunk: empty delta, finish_reason set.
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        // include_usage final chunk: empty choices, usage carried alone.
        OpenAiProvider::process_chat_chunk(
            r#"{"model":"google/gemini-3.8-flash","choices":[],"usage":{"prompt_tokens":42,"completion_tokens":7,"total_tokens":49,"prompt_tokens_details":{"cached_tokens":12}}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        assert_eq!(content, "hi");
        assert_eq!(meta.stop_reason.as_deref(), Some("stop"));
        // 42 processed, 12 of them a cache read. The two overlap, because a
        // stored `input_tokens` is the whole prompt the model read.
        assert_eq!(meta.input_tokens, Some(42));
        assert_eq!(meta.output_tokens, Some(7));
        assert_eq!(meta.cache_read_tokens, Some(12));

        let resp = OpenAiProvider::build_llm_response(content, tools, meta).unwrap();
        assert_eq!(resp.content.as_deref(), Some("hi"));
        assert_eq!(resp.stop_reason.as_deref(), Some("stop"));
        assert_eq!(resp.input_tokens, Some(42));
        assert_eq!(resp.output_tokens, Some(7));
        assert_eq!(resp.cache_read_tokens, Some(12));
        // OpenAI doesn't separate cache writes from reads, so this stays None.
        assert_eq!(resp.cache_creation_tokens, None);
        assert_eq!(
            resp.served_model.as_deref(),
            Some("google/gemini-3.8-flash")
        );
    }

    /// The overlap, on its own, so it is not buried in a stream test. The
    /// cached subset is recorded beside the prompt total, never out of it. That
    /// is what makes the stored figure the size of the whole prompt.
    #[test]
    fn a_cached_prefix_overlaps_the_prompt_total_rather_than_reducing_it() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":42,"completion_tokens":0,"prompt_tokens_details":{"cached_tokens":12}}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        assert_eq!(meta.input_tokens, Some(42));
        assert_eq!(meta.cache_read_tokens, Some(12));
    }

    /// OpenRouter reports an Anthropic cache write beside the read, both
    /// inside `prompt_tokens`. A zero write stays unset, like a zero read.
    #[test]
    fn a_cache_write_is_recorded_as_part_of_the_prompt_total() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":5476,"completion_tokens":9,"prompt_tokens_details":{"cached_tokens":0,"cache_write_tokens":5435}}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        assert_eq!(meta.input_tokens, Some(5476));
        assert_eq!(meta.cache_read_tokens, None);
        assert_eq!(meta.cache_write_tokens, Some(5435));

        let resp = OpenAiProvider::build_llm_response(content, tools, meta).unwrap();
        assert_eq!(resp.cache_creation_tokens, Some(5435));
    }

    #[test]
    fn a_zero_cache_write_stays_unset() {
        let mut meta = StreamMeta::default();
        meta.absorb_chat_usage(&serde_json::json!({
            "prompt_tokens": 40,
            "prompt_tokens_details": {"cached_tokens": 12, "cache_write_tokens": 0}
        }));
        assert_eq!(meta.cache_read_tokens, Some(12));
        assert_eq!(meta.cache_write_tokens, None);
    }

    /// A prompt that read no cache leaves the count unset, so "read nothing"
    /// stays distinct from "this server reports no cached count at all".
    #[test]
    fn a_prompt_with_no_cache_reports_no_cache_read() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":42,"completion_tokens":0}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        assert_eq!(meta.input_tokens, Some(42));
        assert_eq!(meta.cache_read_tokens, None);
    }

    /// More cached than prompt is a corrupt block. The wire records what
    /// arrived rather than repairing it, and the one consumer that subtracts
    /// saturates there instead.
    #[test]
    fn a_cached_count_over_the_prompt_total_is_recorded_as_reported() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":0,"prompt_tokens_details":{"cached_tokens":99}}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        assert_eq!(meta.input_tokens, Some(10));
        assert_eq!(meta.cache_read_tokens, Some(99));
    }

    /// Empty content with `finish_reason=length` is the truncation case. We
    /// must surface it so the empty-completion diagnostic can distinguish
    /// "the model said nothing" from "we hit max_completion_tokens".
    #[test]
    fn chat_stream_captures_truncation_when_content_empty() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":16000,"completion_tokens":0,"total_tokens":16000}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        let resp = OpenAiProvider::build_llm_response(content, tools, meta).unwrap();
        assert_eq!(resp.content, None);
        assert!(resp.tool_calls.is_empty());
        assert_eq!(resp.stop_reason.as_deref(), Some("length"));
        assert_eq!(resp.input_tokens, Some(16000));
        assert_eq!(resp.output_tokens, Some(0));
    }

    /// The incident this guard exists for: `finish_reason: stop`, no content,
    /// no tool call, and no usage chunk at all. `include_usage` is
    /// unconditional, so a compliant server always closes with usage. Its
    /// absence means the terminal frame never arrived, which is ADR 0089's
    /// truncation reached by a different signal than Claude's missing
    /// `message_delta`. Without this the turn reached
    /// `classify_empty_completion` as a clean stop with `output_tokens`
    /// defaulted to 0, graded benign, and the thread ended Idle in silence.
    #[test]
    fn a_clean_stop_that_reported_no_usage_at_all_is_a_truncation() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{"role":"assistant"},"finish_reason":null}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        assert_eq!(meta.stop_reason.as_deref(), Some("stop"));
        assert_eq!(meta.input_tokens, None, "no usage chunk arrived");

        let err = OpenAiProvider::build_llm_response(content, tools, meta)
            .expect_err("a usage-less empty stream must not build a response")
            .to_string();
        // The literal `is_transient_error` matches on, which is what routes
        // this into the retry loop rather than failing the turn.
        assert!(err.contains("stream truncated"), "wording: {err}");
        assert!(crate::llm::is_retryable_error(&err), "wording: {err}");
    }

    /// Text that has streamed, then a mid-stream `error` frame that reads as
    /// retryable. A retry re-sends the whole request.
    const TEXT_THEN_SERVER_ERROR: &str = concat!(
        r#"data: {"choices":[{"delta":{"content":"Here is the plan."}}]}"#,
        "\n\n",
        r#"data: {"error":{"type":"server_error","message":"The server had an error"}}"#,
        "\n\n"
    );

    fn sse_response(body: &'static str) -> reqwest::Response {
        reqwest::Response::from(axum::http::Response::new(reqwest::Body::from(body)))
    }

    /// ADR 0089 on the OpenAI-compatible path: once text has rendered, a
    /// retryable stream error stops the turn, or the retry renders it twice.
    #[tokio::test]
    async fn an_error_frame_after_streamed_text_reports_instead_of_retrying() {
        let provider = OpenAiProvider::new("k".to_string(), "gpt-5.5".to_string()).unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let sink = seen.clone();
        let on_token: Option<TokenCallback> = Some(Box::new(move |t: &str| {
            sink.lock().unwrap().push_str(t);
        }));

        let err = provider
            .parse_chat_stream(sse_response(TEXT_THEN_SERVER_ERROR), &on_token)
            .await
            .expect_err("an error frame fails the parse");

        assert_eq!(seen.lock().unwrap().as_str(), "Here is the plan.");
        let msg = err.to_string();
        assert!(
            !crate::llm::is_retryable_error(&msg),
            "retrying would render the streamed text twice, got: {msg}"
        );
    }

    /// With no callback nothing reached the user, so the same frame retries.
    #[tokio::test]
    async fn an_error_frame_with_nothing_rendered_still_retries() {
        let provider = OpenAiProvider::new("k".to_string(), "gpt-5.5".to_string()).unwrap();
        let err = provider
            .parse_chat_stream(sse_response(TEXT_THEN_SERVER_ERROR), &None)
            .await
            .expect_err("an error frame fails the parse");
        let msg = err.to_string();
        assert!(
            crate::llm::is_retryable_error(&msg),
            "nothing reached the user, so the error must retry, got: {msg}"
        );
    }

    /// A stream that already streamed text stays a success even with no usage
    /// chunk. The token callback has rendered that text to the frontend, so a
    /// retry would render the answer twice. That is the alternative ADR 0089
    /// rejected outright.
    #[test]
    fn a_stream_that_rendered_text_is_never_a_truncation() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{"content":"partial answer"},"finish_reason":"stop"}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        let resp = OpenAiProvider::build_llm_response(content, tools, meta).unwrap();
        assert_eq!(resp.content.as_deref(), Some("partial answer"));
        assert_eq!(resp.input_tokens, None);
    }

    /// A stream that produced a tool call stays a success even with no usage
    /// chunk. Retrying would run the tool a second time, and a side-effecting
    /// tool cannot be replayed for free.
    #[test]
    fn a_stream_that_produced_a_tool_call_is_never_a_truncation() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"path\":\"a.txt\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        let resp = OpenAiProvider::build_llm_response(content, tools, meta).unwrap();
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].name, "read_file");
        assert_eq!(resp.input_tokens, None);
    }

    /// A server that reports only one half of the token pair is odd, not
    /// truncated. Either count proves the terminal frame arrived, so this must
    /// not retry every silent turn such a server produces.
    #[test]
    fn a_partial_usage_block_still_proves_the_stream_completed() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[],"usage":{"completion_tokens":0}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        assert_eq!(meta.input_tokens, None, "this server omits prompt_tokens");
        let resp = OpenAiProvider::build_llm_response(content, tools, meta).unwrap();
        assert_eq!(resp.content, None);
        assert_eq!(resp.output_tokens, Some(0));
    }

    /// The case ADR 0009 protects: the model finished cleanly and chose to say
    /// nothing. The usage chunk arrived, so the stream completed. This stays a
    /// benign empty completion and must not be dragged into a retry.
    #[test]
    fn an_empty_turn_that_reported_usage_stays_a_benign_completion() {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();

        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();
        OpenAiProvider::process_chat_chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":900,"completion_tokens":0,"total_tokens":900}}"#,
            &mut content,
            &mut tools,
            &mut meta,
        )
        .unwrap();

        let resp = OpenAiProvider::build_llm_response(content, tools, meta).unwrap();
        assert_eq!(resp.content, None);
        assert!(resp.tool_calls.is_empty());
        assert_eq!(resp.stop_reason.as_deref(), Some("stop"));
        assert_eq!(resp.input_tokens, Some(900));
        assert_eq!(resp.output_tokens, Some(0));
    }

    /// Feed one SSE error frame through the chunk parser and return the
    /// message it fails with. Every error-frame test goes through the real
    /// entry point rather than calling the formatter directly.
    fn stream_error_for(frame: &str) -> String {
        let mut content = String::new();
        let mut tools: Vec<AccumulatedToolCall> = Vec::new();
        let mut meta = StreamMeta::default();
        OpenAiProvider::process_chat_chunk(frame, &mut content, &mut tools, &mut meta)
            .expect_err("an error frame must fail the chunk")
            .to_string()
    }

    /// OpenRouter sends `code` and `metadata` and no `type`, so a
    /// `type`-only reader called every one of its failures `[unknown]` and
    /// threw away the upstream text. Label from `code`, and name the
    /// provider that actually produced the error.
    #[test]
    fn an_openrouter_error_frame_labels_the_code_and_names_the_provider() {
        let msg = stream_error_for(
            r#"{"error":{"code":429,"message":"Provider returned error",
                "metadata":{"provider_name":"Chutes","raw":"rate limit exceeded"}}}"#,
        );
        assert_eq!(
            msg,
            "OpenAI streaming error [429]: Provider returned error \
             (Chutes: rate limit exceeded)"
        );
    }

    /// The frame that ended the diagnosed turn: a bare message, no `type`
    /// and no `code`. It still reads `[unknown]`, because there is genuinely
    /// nothing else in the frame to say.
    #[test]
    fn an_error_frame_with_no_type_and_no_code_stays_unknown() {
        assert_eq!(
            stream_error_for(r#"{"error":{"message":"ERROR"}}"#),
            "OpenAI streaming error [unknown]: ERROR"
        );
    }

    /// Regression: OpenAI's own frames carry `type`, which still wins over
    /// `code` when both are present.
    #[test]
    fn an_openai_error_frame_still_labels_the_type() {
        assert_eq!(
            stream_error_for(
                r#"{"error":{"type":"server_error","code":"internal",
                    "message":"upstream failed"}}"#
            ),
            "OpenAI streaming error [server_error]: upstream failed"
        );
    }

    /// A provider can put its whole error body in `raw`, newlines and all.
    /// The line stays one line and stays bounded, because this string is
    /// persisted in a `ResponseFailed` payload and rendered in the UI.
    #[test]
    fn a_long_multiline_provider_detail_is_flattened_and_bounded() {
        let raw = format!("upstream said:\n{}", "x".repeat(1000));
        let frame = serde_json::json!({
            "error": {
                "code": 502,
                "message": "Provider returned error",
                "metadata": { "provider_name": "Chutes", "raw": raw },
            }
        })
        .to_string();

        let msg = stream_error_for(&frame);
        assert!(!msg.contains('\n'), "must stay one line: {msg}");
        assert!(msg.ends_with("…)"), "must be marked as truncated: {msg}");
        assert!(msg.len() < 400, "must stay bounded, got {}", msg.len());
        assert!(msg.starts_with("OpenAI streaming error [502]: Provider returned error (Chutes: "));
    }

    /// An error frame carrying nothing usable must still fail the stream,
    /// rather than being read as an ordinary empty chunk.
    #[test]
    fn an_empty_error_object_still_fails_the_stream() {
        assert_eq!(
            stream_error_for(r#"{"error":{}}"#),
            "OpenAI streaming error [unknown]: Unknown streaming error"
        );
    }

    /// A local OpenAI-compatible server often sends the error as a bare
    /// string. Reading only `error.message` threw away the only text the
    /// frame carried.
    #[test]
    fn a_bare_string_error_keeps_its_text() {
        assert_eq!(
            stream_error_for(r#"{"error":"context window exceeded"}"#),
            "OpenAI streaming error [unknown]: context window exceeded"
        );
    }

    /// A provider that failed with no body sends `raw: null`. Rendering that
    /// appended a literal "(null)" to an otherwise clean error.
    #[test]
    fn a_null_provider_body_appends_nothing() {
        assert_eq!(
            stream_error_for(
                r#"{"error":{"code":502,"message":"Provider returned error",
                    "metadata":{"provider_name":"Chutes","raw":null}}}"#
            ),
            "OpenAI streaming error [502]: Provider returned error (Chutes)"
        );
    }

    /// Deliberate consequence of carrying `code`: the retry classifier reads
    /// the formatted string, so a transient upstream status now reaches it.
    /// Before, every OpenRouter frame read `[unknown]` and no retry fired.
    /// A genuine client error still must not retry.
    #[test]
    fn a_transient_upstream_status_now_reaches_the_retry_classifier() {
        let transient =
            stream_error_for(r#"{"error":{"code":502,"message":"Provider returned error"}}"#);
        assert!(
            crate::llm::is_retryable_error(&transient),
            "502 must retry: {transient}"
        );

        let client_error = stream_error_for(
            r#"{"error":{"code":400,"message":"invalid request: bad tool schema"}}"#,
        );
        assert!(
            !crate::llm::is_retryable_error(&client_error),
            "400 must not retry: {client_error}"
        );
    }

    /// Regression: after a tool call the agentic loop packs the ToolResult and
    /// a trailing instruction Text into ONE user `Message::Blocks`. The
    /// serializer must emit the `tool` message IMMEDIATELY after the assistant
    /// `tool_calls` message — never wedge the user text between them — or strict
    /// providers (Moonshot/Kimi, OpenAI) reject with HTTP 400 "an assistant
    /// message with 'tool_calls' must be followed by tool messages … the
    /// following tool_call_ids did not have response messages: load_knowhow:0".
    /// (Observed on moonshotai/kimi-k3 via OpenRouter, thread 85239abe.)
    #[test]
    fn tool_result_immediately_follows_assistant_tool_calls() {
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "load_knowhow:0".to_string(),
                    name: "load_knowhow".to_string(),
                    input: serde_json::json!({ "id": "browser-learning/reflection" }),
                    thought_signature: None,
                }]),
            },
            Message {
                role: "user".to_string(),
                content: MessageContent::Blocks(vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "load_knowhow:0".to_string(),
                        content: "…knowhow body…".to_string(),
                    },
                    ContentBlock::Text {
                        text: "Results above. Do NOT repeat analysis you already gave.".to_string(),
                    },
                ]),
            },
        ];

        let wire: Vec<serde_json::Value> = OpenAiProvider::convert_messages_chat(&messages)
            .into_iter()
            .map(|message| message.into_value(false))
            .collect();

        // Exactly three wire messages, in order:
        //   assistant(tool_calls) → tool(result) → user(text)
        assert_eq!(
            wire.len(),
            3,
            "expected assistant, tool, user; got {:?}",
            wire
        );
        assert_eq!(wire[0]["role"], "assistant");
        assert_eq!(wire[0]["tool_calls"][0]["id"], "load_knowhow:0");
        assert_eq!(wire[1]["role"], "tool");
        assert_eq!(wire[1]["tool_call_id"], "load_knowhow:0");
        assert_eq!(wire[2]["role"], "user");
        assert!(wire[2]["content"]
            .as_str()
            .unwrap()
            .contains("Results above"));

        // The tool response must be ADJACENT to the assistant tool_calls — no
        // `user` message wedged between (that is the Moonshot/Kimi 400).
        let assistant_idx = wire.iter().position(|m| m["role"] == "assistant").unwrap();
        let tool_idx = wire
            .iter()
            .position(|m| m["role"] == "tool" && m["tool_call_id"] == "load_knowhow:0")
            .unwrap();
        assert_eq!(
            tool_idx,
            assistant_idx + 1,
            "tool result must immediately follow the assistant tool_calls message"
        );
    }

    /// `stream_options.include_usage` must be present in the Chat
    /// Completions request body — without it, OpenAI never sends the final
    /// usage chunk and `meta.input_tokens` / `meta.output_tokens` stay None.
    #[test]
    fn chat_body_requests_usage_in_stream() {
        let provider = OpenAiProvider::new("k".to_string(), "gpt-4o".to_string()).unwrap();
        let body = provider
            .build_chat_body("gpt-4o", &[], &[], None, None)
            .body;
        let opts = body
            .get("stream_options")
            .expect("stream_options must be sent");
        assert_eq!(opts["include_usage"], serde_json::Value::Bool(true));
    }

    fn msg(role: &str, blocks: Vec<ContentBlock>) -> Message {
        Message {
            role: role.to_string(),
            content: MessageContent::Blocks(blocks),
        }
    }

    fn text(text: &str) -> ContentBlock {
        ContentBlock::Text {
            text: text.to_string(),
        }
    }

    fn view(text: &str) -> ContentBlock {
        ContentBlock::MemoryView {
            text: text.to_string(),
        }
    }

    fn tail(text: &str) -> ContentBlock {
        ContentBlock::EngineTail {
            text: text.to_string(),
        }
    }

    const CLAUDE: &str = "anthropic/claude-haiku-5.5";

    fn body_for(model: &str, messages: &[Message], system: Option<&str>) -> serde_json::Value {
        OpenAiProvider::new("k".to_string(), model.to_string())
            .unwrap()
            .build_chat_body(model, messages, &[], system, None)
            .body
    }

    fn marker_count(body: &serde_json::Value) -> usize {
        body.to_string().matches("\"cache_control\"").count()
    }

    /// The wire indexes of the messages whose content carries a marker.
    fn marked_messages(body: &serde_json::Value) -> Vec<usize> {
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, m)| m.to_string().contains("\"cache_control\""))
            .map(|(i, _)| i)
            .collect()
    }

    fn history() -> Vec<Message> {
        vec![
            msg("user", vec![text("first question")]),
            msg("assistant", vec![text("first answer")]),
            msg("user", vec![view("memory view"), text("second question")]),
        ]
    }

    /// Every other model on this wire gets the body it always got. A strict or
    /// local server can reject a key it does not know.
    #[test]
    fn a_model_outside_the_anthropic_namespace_gets_no_cache_control() {
        for model in [
            "gpt-5.4",
            "z-ai/glm-5.2",
            "google/gemini-3.8-flash",
            "muse-glimmer:30b-mlx",
        ] {
            let body = body_for(model, &history(), Some("system"));
            assert_eq!(marker_count(&body), 0, "{model}: {body}");
            assert_eq!(body["messages"][0]["content"], "system");
            assert_eq!(body["messages"][1]["content"], "first question");
            assert_eq!(
                body["messages"][3]["content"], "memory view\nsecond question",
                "{model} joined its text parts as before"
            );
        }
    }

    /// Priority mirrors `anthropic_wire`: the last message, the one before it,
    /// each memory view, then the system block.
    #[test]
    fn an_anthropic_request_marks_last_messages_views_then_system() {
        let body = body_for(CLAUDE, &history(), Some("system"));
        let wire = &body["messages"];
        let marker = serde_json::json!({"type": "ephemeral"});

        assert_eq!(wire[0]["content"][0]["cache_control"], marker, "system");
        assert!(wire[1]["content"].is_string(), "oldest message stays plain");
        assert_eq!(
            wire[2]["content"][0]["cache_control"], marker,
            "penultimate"
        );
        // The last message is marked at its final non-tail part, and its view
        // part holds a marker of its own.
        assert_eq!(wire[3]["content"][1]["text"], "second question");
        assert_eq!(wire[3]["content"][1]["cache_control"], marker, "last");
        assert_eq!(wire[3]["content"][0]["cache_control"], marker, "view");
        assert_eq!(marker_count(&body), 4);
    }

    /// The stable context block is plain text on this wire (I8). A GPT model
    /// gets it joined, and an Anthropic model gets a part of its own that
    /// carries no marker.
    #[test]
    fn a_stable_context_block_is_plain_text_and_takes_no_marker() {
        let stable = ContentBlock::StableContext {
            text: "profile".to_string(),
        };
        let messages = vec![msg("user", vec![stable, text("question")])];
        let gpt = body_for("gpt-5.4", &messages, None);
        assert_eq!(gpt["messages"][0]["content"], "profile\nquestion");
        assert_eq!(marker_count(&gpt), 0);
        let claude = body_for(CLAUDE, &messages, None);
        let content = &claude["messages"][0]["content"];
        assert_eq!(content[0]["text"], "profile");
        assert!(content[0].get("cache_control").is_none());
        assert_eq!(content[1]["cache_control"]["type"], "ephemeral");
    }

    /// A tail block carries the engine's volatile notes. It is rewritten every
    /// round, so it never anchors a marker.
    #[test]
    fn an_engine_tail_never_anchors_a_marker() {
        let messages = vec![msg("user", vec![text("question"), tail("volatile note")])];
        let body = body_for(CLAUDE, &messages, None);
        let content = &body["messages"][0]["content"];
        assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
        assert!(content[1].get("cache_control").is_none(), "{content}");
        assert_eq!(content[1]["text"], "volatile note");
    }

    /// Tool results split off into `tool` messages, which take no marker. The
    /// tail then sits alone in a user message, and the marker goes to the
    /// message before the tool round instead.
    #[test]
    fn a_message_of_only_tail_text_takes_no_marker() {
        let messages = vec![
            msg("user", vec![text("question")]),
            msg(
                "assistant",
                vec![
                    text("Checking."),
                    ContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "load_knowhow".to_string(),
                        input: serde_json::json!({}),
                        thought_signature: None,
                    },
                ],
            ),
            msg(
                "user",
                vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: "result body".to_string(),
                    },
                    tail("volatile note"),
                ],
            ),
        ];
        let body = body_for(CLAUDE, &messages, None);
        let wire = body["messages"].as_array().unwrap();
        assert_eq!(wire.len(), 4, "{body}");
        assert_eq!(wire[3]["role"], "user");
        assert!(!wire[3].to_string().contains("cache_control"), "{body}");
        assert_eq!(marked_messages(&body), vec![0, 1]);
    }

    #[test]
    fn an_anthropic_request_never_exceeds_four_markers() {
        let mut messages = Vec::new();
        for i in 0..6 {
            messages.push(msg(
                "user",
                vec![view(&format!("view {i}")), text(&format!("question {i}"))],
            ));
            messages.push(msg("assistant", vec![text(&format!("answer {i}"))]));
        }
        let body = body_for(CLAUDE, &messages, Some("system"));
        assert_eq!(marker_count(&body), 4, "{body}");
        // The two newest messages hold two of them. The system block is last in
        // priority, and the views spent the rest.
        let marked = marked_messages(&body);
        assert!(marked.contains(&12) && marked.contains(&11), "{marked:?}");
        assert!(
            body["messages"][0]["content"].is_string(),
            "system lost its slot to the views"
        );
    }

    /// A tool round: the last wire message is a `tool` message, which takes
    /// only a string. The marker walks back to the nearest message that can
    /// hold one, and no part is empty.
    #[test]
    fn a_tool_round_marks_only_messages_that_take_a_part_array() {
        let messages = vec![
            msg("user", vec![text("look it up"), text("")]),
            msg(
                "assistant",
                vec![
                    text("Checking."),
                    ContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "load_knowhow".to_string(),
                        input: serde_json::json!({}),
                        thought_signature: None,
                    },
                ],
            ),
            msg(
                "user",
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "result body".to_string(),
                }],
            ),
        ];
        let body = body_for(CLAUDE, &messages, None);
        let wire = body["messages"].as_array().unwrap();

        assert_eq!(wire.len(), 3, "user, assistant, tool: {body}");
        assert_eq!(wire[1]["role"], "assistant");
        assert_eq!(wire[2]["role"], "tool");
        assert_eq!(wire[2]["tool_call_id"], "call_1");
        assert_eq!(wire[2]["content"], "result body");
        assert_eq!(marked_messages(&body), vec![0, 1]);
        assert!(!body.to_string().contains(r#""text":"""#), "{body}");
    }

    /// An assistant message holding only `tool_calls` has no text to mark.
    #[test]
    fn a_message_with_no_text_is_not_marked() {
        let messages = vec![msg(
            "assistant",
            vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "load_knowhow".to_string(),
                input: serde_json::json!({}),
                thought_signature: None,
            }],
        )];
        let body = body_for(CLAUDE, &messages, None);
        assert_eq!(marker_count(&body), 0, "{body}");
    }

    /// The tool result must still follow its assistant `tool_calls` message.
    #[test]
    fn an_anthropic_tool_round_keeps_the_result_adjacent() {
        let messages = vec![
            msg(
                "assistant",
                vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "load_knowhow".to_string(),
                    input: serde_json::json!({}),
                    thought_signature: None,
                }],
            ),
            msg(
                "user",
                vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: "result body".to_string(),
                    },
                    text("Results above."),
                ],
            ),
        ];
        let body = body_for(CLAUDE, &messages, None);
        let roles: Vec<&str> = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["assistant", "tool", "user"]);
        assert_eq!(body["messages"][2]["content"][0]["text"], "Results above.");
    }

    /// A message keeps the same blocks with or without a marker. A cache entry
    /// is keyed on them, so a message marked last round must not change shape
    /// when it is plain this round.
    #[test]
    fn an_unmarked_message_renders_the_blocks_it_had_when_marked() {
        let mut messages = vec![msg("user", vec![text("context"), tail("volatile note")])];
        let marked = body_for(CLAUDE, &messages, None);
        for _ in 0..3 {
            messages.push(msg("assistant", vec![text("answer")]));
            messages.push(msg("user", vec![text("next")]));
        }
        let unmarked = body_for(CLAUDE, &messages, None);

        let strip = |content: &serde_json::Value| {
            content
                .as_array()
                .unwrap()
                .iter()
                .map(|part| part["text"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            strip(&marked["messages"][0]["content"]),
            strip(&unmarked["messages"][0]["content"])
        );
    }

    /// Images ride beside the text parts, and a marker lands on text only.
    #[test]
    fn an_image_message_keeps_its_image_after_the_marked_text() {
        let messages = vec![msg(
            "user",
            vec![
                text("describe this"),
                ContentBlock::Image {
                    source_type: "base64".to_string(),
                    media_type: "image/png".to_string(),
                    data: "AAAA".to_string(),
                },
            ],
        )];
        let body = body_for(CLAUDE, &messages, None);
        let content = &body["messages"][0]["content"];
        assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(marker_count(&body), 1);
    }

    /// Chat Completions forces a call by naming the function.
    #[test]
    fn a_forced_call_names_its_function() {
        assert_eq!(
            OpenAiProvider::forced_tool_choice_chat("request_read"),
            serde_json::json!({ "type": "function", "function": { "name": "request_read" } })
        );
    }

    /// The effort reaches the body exactly as handed in, for every model.
    ///
    /// This builder also serves OpenRouter and local servers, so it cannot tell
    /// whose vocabulary applies from the model id. It used to rewrite `max`
    /// into `xhigh` whenever the id was not `gpt-5.6`, which sent an
    /// OpenAI-proprietary tier to a local server that rejected it (400,
    /// 2026-08-12). Deciding what a model supports belongs to `llm::reasoning`
    /// and is enforced by `RoutingProvider`; a second rule here is drift.
    #[test]
    fn chat_body_sends_the_reasoning_effort_verbatim() {
        let provider = OpenAiProvider::new("k".to_string(), "gpt-4o".to_string()).unwrap();
        for model in [
            "gpt-5.6-sol",
            "gpt-5.4",
            "muse-glimmer:30b-mlx",
            "z-ai/glm-5.2",
        ] {
            for effort in crate::llm::reasoning::EFFORT_LADDER {
                let body = provider
                    .build_chat_body(model, &[], &[], None, Some(effort))
                    .body;
                assert_eq!(
                    body["reasoning_effort"], *effort,
                    "{model} rewrote {effort}"
                );
            }
        }
    }

    /// No effort means no key at all, so the server applies its own default
    /// rather than being told a value the caller never chose.
    #[test]
    fn chat_body_omits_reasoning_effort_when_none_is_given() {
        let provider = OpenAiProvider::new("k".to_string(), "gpt-4o".to_string()).unwrap();
        let body = provider
            .build_chat_body("gpt-4o", &[], &[], None, None)
            .body;
        assert!(body.get("reasoning_effort").is_none());
    }

    // --- Prefill beats (ADR 0556) ---

    fn chat_body(model: &str, messages: &[Message], system: Option<&str>) -> ChatBody {
        OpenAiProvider::new("k".to_string(), model.to_string())
            .unwrap()
            .build_chat_body(model, messages, &[], system, Some("high"))
    }

    fn chat_route(model: &str) -> TurnRoute<'_> {
        TurnRoute {
            wire_model: model,
            reasoning_effort: Some("high"),
            betas: &[],
        }
    }

    fn json(bytes: &[u8]) -> serde_json::Value {
        serde_json::from_slice(bytes).expect("a body is JSON")
    }

    /// `body` as a prefill: one output token, not streamed, no usage stream.
    fn as_prefill(body: &serde_json::Value) -> serde_json::Value {
        let mut value = body.clone();
        let object = value.as_object_mut().unwrap();
        object.remove("max_completion_tokens");
        object.remove("stream_options");
        object.insert("max_tokens".into(), CHAT_OUTPUT_CAP.into());
        object.insert("stream".into(), false.into());
        value
    }

    /// The whole cut keeps the turn's reasoning and tool choice. The stable
    /// cut ends at the marked view, never at a rolling mark after it.
    #[test]
    fn an_openrouter_pair_cuts_at_the_last_marked_view() {
        let ChatBody {
            mut body,
            stable_end,
        } = chat_body(CLAUDE, &history(), Some("system"));
        body["tool_choice"] = OpenAiProvider::forced_tool_choice_chat("request_read");
        let end = stable_end.expect("a marked view ends the cut");
        assert_eq!(end, StableEnd::Part { item: 3, part: 0 });
        let pair = prefill_pair(&body, end, chat_route(CLAUDE)).expect("a pair");

        let whole = as_prefill(&body);
        assert_eq!(json(&pair.whole.body), whole);
        assert_eq!(whole["max_tokens"], 1);
        assert!(whole.get("max_completion_tokens").is_none());
        assert_eq!(whole["reasoning_effort"], "high");
        assert_eq!(whole["tool_choice"]["function"]["name"], "request_read");

        let mut stable = whole;
        let messages = stable["messages"].as_array_mut().unwrap();
        messages.truncate(4);
        messages[3]["content"].as_array_mut().unwrap().truncate(1);
        assert_eq!(json(&pair.stable.body), stable);
        assert_eq!(stable["messages"][3]["content"][0]["text"], "memory view");
        assert!(stable["messages"][3]["content"][0]["cache_control"].is_object());
        assert_eq!(pair.stable.output_cap, Some(CHAT_OUTPUT_CAP));
    }

    /// With no view, a marked system block ends the cut, and a placeholder
    /// message follows it.
    #[test]
    fn with_no_view_the_cut_keeps_the_marked_system_block() {
        let plain = vec![
            msg("user", vec![text("first question")]),
            msg("assistant", vec![text("first answer")]),
            msg("user", vec![text("second question")]),
        ];
        let ChatBody { body, stable_end } = chat_body(CLAUDE, &plain, Some("system"));
        let end = stable_end.expect("the marked system block ends the cut");
        assert_eq!(end, StableEnd::Placeholder { keep: 1 });
        let pair = prefill_pair(&body, end, chat_route(CLAUDE)).expect("a pair");
        let stable = json(&pair.stable.body);
        let messages = stable["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert!(messages[0]["content"][0]["cache_control"].is_object());
        assert_eq!(
            messages[1],
            serde_json::json!({"role": "user", "content": "."})
        );

        assert!(chat_body(CLAUDE, &plain, None).stable_end.is_none());
    }

    /// A model outside the Anthropic namespace carries no marker, so a beat
    /// would read nothing.
    #[test]
    fn a_model_outside_the_anthropic_namespace_gets_no_cut() {
        for model in ["gpt-5.4", "z-ai/glm-5.2", "google/gemini-3.8-flash"] {
            assert!(
                chat_body(model, &history(), Some("system"))
                    .stable_end
                    .is_none(),
                "{model}"
            );
        }
    }

    const COMPLETED_TURN: &str = concat!(
        r#"data: {"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}]}"#,
        "\n\n",
        r#"data: {"choices":[],"usage":{"prompt_tokens":900,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":0,"cache_write_tokens":880}}}"#,
        "\n\n",
        "data: [DONE]\n\n",
    );

    const PREFILL_REPLY: &str = r#"{"model":"anthropic/claude-haiku-5.5","choices":[],"usage":{"prompt_tokens":900,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":880}}}"#;

    /// A turn hands back its pair, and the beat posts the stable cut to the
    /// Chat Completions endpoint and reads the reply's usage.
    #[tokio::test]
    async fn a_turn_returns_a_pair_whose_beat_reads_the_turn() {
        let (base, bodies) =
            super::super::serve_recording("/v1/chat/completions", COMPLETED_TURN, PREFILL_REPLY)
                .await;
        let provider =
            OpenAiProvider::new_with_base_url("k".into(), CLAUDE.into(), &base, Vec::new(), true)
                .unwrap();
        let turn = provider
            .chat_completions(
                &history(),
                &[],
                ModelSelection::model(CLAUDE),
                Some("system"),
                None,
            )
            .await
            .expect("the turn completes");
        let pair = turn
            .prefill
            .expect("an OpenRouter Claude turn can be prefilled");

        let beat = crate::llm::LlmProvider::prefill(
            &provider,
            &pair.stable,
            crate::llm::metered::CallToken::for_test(),
        )
        .await
        .expect("the beat answers");
        assert_eq!(beat.cache_read_tokens, Some(880));
        assert_eq!(beat.output_tokens, Some(1));

        let sent = bodies.lock().unwrap().clone();
        let [_, beat_body] = sent.as_slice() else {
            panic!("expected a turn and a beat, saw {}", sent.len());
        };
        assert_eq!(beat_body, &json(&pair.stable.body));
    }
}
