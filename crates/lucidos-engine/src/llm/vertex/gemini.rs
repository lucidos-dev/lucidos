//! Gemini/Vertex request-build + response-mapping for `VertexProvider`,
//! plus the Google-Search grounding helper. The `VertexProvider` struct and
//! shared auth/config live in the parent `vertex` module.


use crate::llm::provider::{
    ContentBlock, LlmResponse, Message, MessageContent, TokenCallback, ToolCall, ToolDefinition,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use super::VertexProvider;

impl VertexProvider {
    /// Search the web using Gemini's Google Search grounding.
    /// Returns a formatted string with the grounded answer and numbered source list.
    pub async fn search_with_grounding(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let model = "gemini-2.5-flash-lite";
        let request = serde_json::json!({
            "system_instruction": {
                "parts": [{"text": "You are a web search assistant. Search the web thoroughly and return comprehensive, detailed results with sources. Focus on finding and presenting relevant information from search results rather than answering from your own knowledge."}]
            },
            "contents": [{
                "role": "user",
                "parts": [{"text": query}]
            }],
            "tools": [{"google_search": {}}]
        });

        let access_token = self.get_access_token().await?;
        let url = self.endpoint_for_model(model);

        let (status, body) = self
            .request_with_retry(model, &url, &access_token, &request)
            .await?;

        if !status.is_success() {
            return Err(format!("Gemini search API error ({}): {}", status, body).into());
        }

        let parsed: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse Gemini search response: {}", e))?;

        // Extract the grounded text answer
        let answer = parsed["candidates"][0]["content"]["parts"]
            .as_array()
            .and_then(|parts| {
                parts
                    .iter()
                    .find_map(|p| p["text"].as_str().map(|s| s.to_string()))
            })
            .unwrap_or_default();

        // Extract grounding chunks (sources with URL and title)
        let metadata = &parsed["candidates"][0]["groundingMetadata"];
        let chunks = metadata["groundingChunks"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut sources = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            if i >= max_results {
                break;
            }
            let title = chunk["web"]["title"].as_str().unwrap_or("Untitled");
            let uri = chunk["web"]["uri"].as_str().unwrap_or("");
            if !uri.is_empty() {
                sources.push(format!("{}. {}\n   {}", i + 1, title, uri));
            }
        }

        if answer.is_empty() && sources.is_empty() {
            return Ok(format!("No search results found for: {}", query));
        }

        let mut result = String::new();
        if !answer.is_empty() {
            result.push_str(&answer);
        }
        if !sources.is_empty() {
            if !result.is_empty() {
                result.push_str("\n\n");
            }
            result.push_str("Sources:\n\n");
            result.push_str(&sources.join("\n\n"));
        }

        Ok(result)
    }

    pub(super) async fn chat_gemini(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: &str,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let contents = messages_to_vertex_contents(messages)?;

        let vertex_tools = if tools.is_empty() {
            None
        } else {
            Some(vec![VertexTool {
                function_declarations: tools
                    .into_iter()
                    .map(|t| VertexFunction {
                        name: t.name,
                        description: t.description,
                        parameters: t.parameters,
                    })
                    .collect(),
            }])
        };

        let system_inst = system_prompt.map(|s| VertexSystemInstruction {
            parts: vec![VertexPart {
                text: Some(s.to_string()),
                ..Default::default()
            }],
        });

        let generation_config = if model.starts_with("gemini-3") {
            let effort = reasoning_effort.unwrap_or("high");
            if effort == "none" {
                Some(VertexGenerationConfig {
                    thinking_config: VertexThinkingConfig { thinking_budget: 0 },
                })
            } else {
                let budget = crate::llm::thinking_budget_for_effort(effort);
                Some(VertexGenerationConfig {
                    thinking_config: VertexThinkingConfig {
                        thinking_budget: budget,
                    },
                })
            }
        } else {
            None
        };

        let request = VertexRequest {
            system_instruction: system_inst,
            contents,
            tools: vertex_tools,
            generation_config,
        };

        let access_token = self.get_access_token().await?;

        let (status, body) = self
            .request_with_retry(
                model,
                &self.endpoint_for_model(model),
                &access_token,
                &request,
            )
            .await?;

        if !status.is_success() {
            log!("[Vertex] Gemini API error ({}): {}", status, body);
            return Err(format!("Gemini API error ({}): {}", status, body).into());
        }

        let parsed: VertexResponse = match serde_json::from_str(&body) {
            Ok(p) => p,
            Err(e) => {
                log!(
                    "[Vertex] Failed to parse Gemini response: {}\nBody: {}",
                    e,
                    body
                );
                return Err(format!("Failed to parse Gemini response: {}", e).into());
            }
        };

        let response = build_gemini_llm_response(parsed);

        // Gemini uses non-streaming requests, so emit the full final-answer
        // text at once. If the same response also contains function calls, the
        // text is a model/tool-turn preamble and the agentic loop suppresses it
        // before writing conversation history. Do not surface it as streamed
        // assistant prose first; that made prompt echoes visible in trigger runs.
        if should_emit_gemini_text_delta(&response) {
            if let (Some(cb), Some(text)) = (&on_token, response.content.as_ref()) {
                cb(text);
            }
        }

        Ok(response)
    }
}

fn should_emit_gemini_text_delta(response: &LlmResponse) -> bool {
    response.tool_calls.is_empty()
        && response
            .content
            .as_deref()
            .is_some_and(|text| !text.is_empty())
}

/// Translate a parsed `VertexResponse` (Gemini non-streaming reply) into
/// the cross-provider `LlmResponse`. Pulled out of the axum-adjacent
/// handler so empty-completion behaviour can be unit-tested — the previous
/// inline mapping discarded `finishReason` + `usageMetadata`, which made
/// "model returned nothing" indistinguishable from a safety block or a
/// `MAX_TOKENS` cutoff. We now preserve both verbatim.
fn build_gemini_llm_response(parsed: VertexResponse) -> LlmResponse {
    let mut content: Option<String> = None;
    let mut tool_calls = Vec::new();
    let mut stop_reason: Option<String> = None;

    if let Some(candidates) = parsed.candidates {
        if let Some(candidate) = candidates.into_iter().next() {
            stop_reason = candidate.finish_reason;
            for part in candidate.content.parts {
                // Skip thinking parts — internal reasoning, not shown to user
                if part.thought {
                    continue;
                }
                if let Some(text) = part.text {
                    content = Some(text);
                }
                if let Some(fc) = part.function_call {
                    tool_calls.push(ToolCall {
                        id: uuid::Uuid::new_v4().to_string(),
                        name: fc.name,
                        arguments: fc.args,
                        thought_signature: part.thought_signature,
                    });
                }
            }
        }
    }

    let (input_tokens, output_tokens) = parsed
        .usage_metadata
        .map(|u| {
            (
                u.prompt_token_count
                    .map(|n| crate::llm::clamp_provider_token_count(n, "Vertex")),
                u.candidates_token_count
                    .map(|n| crate::llm::clamp_provider_token_count(n, "Vertex")),
            )
        })
        .unwrap_or((None, None));

    LlmResponse {
        content,
        tool_calls,
        stop_reason,
        output_tokens,
        input_tokens,
        // Gemini exposes `cachedContentTokenCount` separately, but that's a
        // distinct caching model (named caches, not Anthropic's ephemeral
        // prompt cache). Keep both fields None to avoid conflating the two
        // until the cost UI explicitly grows a Gemini-cached lane.
        cache_creation_tokens: None,
        cache_read_tokens: None,
        thinking_chars: None,
        unknown_sse_dropped: 0,
    }
}

// ===== Gemini/Vertex request/response types =====

#[derive(Serialize)]
struct VertexRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<VertexSystemInstruction>,
    contents: Vec<VertexContent>,
    tools: Option<Vec<VertexTool>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "generationConfig")]
    generation_config: Option<VertexGenerationConfig>,
}

#[derive(Serialize)]
struct VertexGenerationConfig {
    #[serde(rename = "thinkingConfig")]
    thinking_config: VertexThinkingConfig,
}

#[derive(Serialize)]
struct VertexThinkingConfig {
    #[serde(rename = "thinkingBudget")]
    thinking_budget: u32,
}

#[derive(Serialize)]
struct VertexSystemInstruction {
    parts: Vec<VertexPart>,
}

#[derive(Serialize)]
struct VertexContent {
    role: String,
    parts: Vec<VertexPart>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct VertexPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inline_data: Option<VertexInlineData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_call: Option<VertexFunctionCallPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_response: Option<VertexFunctionResponsePart>,
    /// Gemini 3 attaches an opaque `thoughtSignature` to the first
    /// `functionCall` part of every turn and rejects the next request with
    /// HTTP 400 INVALID_ARGUMENT unless the same signature is echoed back on
    /// the same `functionCall` part. Sits at the part level (peer of
    /// `functionCall`), not inside it.
    #[serde(skip_serializing_if = "Option::is_none")]
    thought_signature: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VertexInlineData {
    mime_type: String,
    data: String,
}

#[derive(Serialize)]
struct VertexFunctionCallPart {
    name: String,
    args: serde_json::Value,
}

#[derive(Serialize)]
struct VertexFunctionResponsePart {
    name: String,
    response: serde_json::Value,
}

/// Convert engine `Message`s to Gemini `VertexContent`s.
///
/// Walks the list left-to-right while building a `tool_use_id → name` map from
/// `ContentBlock::ToolUse` blocks so that later `ContentBlock::ToolResult`
/// blocks can emit a `functionResponse` part — Gemini indexes responses by
/// function name, not by tool-use id, so the name has to be resolved here.
///
/// Dropping `ToolUse`/`ToolResult` from the parts (the old behavior) left any
/// assistant message that contained ONLY a tool call with `parts: []`, which
/// Gemini rejects with `INVALID_ARGUMENT: must include at least one parts
/// field`.
fn messages_to_vertex_contents(
    messages: Vec<Message>,
) -> Result<Vec<VertexContent>, Box<dyn std::error::Error + Send + Sync>> {
    let mut tool_use_names: HashMap<String, String> = HashMap::new();
    let mut contents = Vec::with_capacity(messages.len());
    for m in messages {
        let role = if m.role == "user" {
            "user".to_string()
        } else {
            "model".to_string()
        };
        let parts = message_content_to_parts(m.content, &mut tool_use_names)?;
        contents.push(VertexContent { role, parts });
    }
    Ok(contents)
}

fn message_content_to_parts(
    content: MessageContent,
    tool_use_names: &mut HashMap<String, String>,
) -> Result<Vec<VertexPart>, Box<dyn std::error::Error + Send + Sync>> {
    match content {
        MessageContent::Text(s) => Ok(vec![VertexPart {
            text: Some(s),
            ..Default::default()
        }]),
        MessageContent::Blocks(blocks) => blocks
            .into_iter()
            .map(|block| match block {
                ContentBlock::Text { text } => Ok(VertexPart {
                    text: Some(text),
                    ..Default::default()
                }),
                ContentBlock::Image {
                    media_type, data, ..
                } => Ok(VertexPart {
                    inline_data: Some(VertexInlineData {
                        mime_type: media_type,
                        data,
                    }),
                    ..Default::default()
                }),
                ContentBlock::ToolUse {
                    id,
                    name,
                    input,
                    thought_signature,
                } => {
                    tool_use_names.insert(id, name.clone());
                    Ok(VertexPart {
                        function_call: Some(VertexFunctionCallPart {
                            name,
                            args: input,
                        }),
                        thought_signature,
                        ..Default::default()
                    })
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                } => {
                    let name = tool_use_names.get(&tool_use_id).cloned().ok_or_else(|| {
                        format!(
                            "ToolResult references unknown tool_use_id {} — Gemini functionResponse needs a function name and the preceding assistant message had no matching ToolUse",
                            tool_use_id
                        )
                    })?;
                    Ok(VertexPart {
                        function_response: Some(VertexFunctionResponsePart {
                            name,
                            response: serde_json::json!({ "content": content }),
                        }),
                        ..Default::default()
                    })
                }
            })
            .collect(),
    }
}

#[derive(Serialize)]
struct VertexTool {
    function_declarations: Vec<VertexFunction>,
}

#[derive(Serialize)]
struct VertexFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Deserialize)]
struct VertexResponse {
    candidates: Option<Vec<VertexCandidate>>,
    /// Top-level token-usage block. Present on every non-error Gemini
    /// response — we map `promptTokenCount` → `input_tokens` and
    /// `candidatesTokenCount` → `output_tokens` so cost analytics has the
    /// same shape as Anthropic/OpenAI provider responses.
    #[serde(rename = "usageMetadata", default)]
    usage_metadata: Option<VertexUsageMetadata>,
}

#[derive(Deserialize)]
struct VertexCandidate {
    #[serde(default)]
    content: VertexResponseContent,
    /// `"STOP"` for a normal completion; `"MAX_TOKENS"`, `"SAFETY"`,
    /// `"RECITATION"`, `"OTHER"`, `"BLOCKLIST"`, ... for early termination.
    /// Preserved verbatim so empty-completion debugging can distinguish
    /// "model said nothing" from a content-policy block.
    #[serde(rename = "finishReason", default)]
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct VertexResponseContent {
    #[serde(default)]
    parts: Vec<VertexResponsePart>,
}

#[derive(Deserialize, Default)]
struct VertexUsageMetadata {
    #[serde(rename = "promptTokenCount", default)]
    prompt_token_count: Option<u64>,
    #[serde(rename = "candidatesTokenCount", default)]
    candidates_token_count: Option<u64>,
}

#[derive(Deserialize)]
struct VertexResponsePart {
    text: Option<String>,
    /// When true, this part contains internal reasoning (Gemini thinking mode).
    #[serde(default)]
    thought: bool,
    #[serde(rename = "functionCall")]
    function_call: Option<VertexFunctionCall>,
    /// Opaque encrypted reasoning signature Gemini 3 attaches to the first
    /// `functionCall` part of every turn. Must be echoed back on the next
    /// request or the API rejects with HTTP 400 INVALID_ARGUMENT
    /// "Function call is missing a thought_signature".
    #[serde(default, rename = "thoughtSignature")]
    thought_signature: Option<String>,
}

#[derive(Deserialize)]
struct VertexFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{ContentBlock, Message, MessageContent};

    #[test]
    fn message_content_to_parts_text_only() {
        let content = MessageContent::Text("hello".to_string());
        let parts = message_content_to_parts(content, &mut HashMap::new()).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].text.as_deref(), Some("hello"));
        assert!(parts[0].inline_data.is_none());
    }

    #[test]
    fn message_content_to_parts_with_image() {
        let content = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "describe this".to_string(),
            },
            ContentBlock::Image {
                source_type: "base64".to_string(),
                media_type: "image/jpeg".to_string(),
                data: "abc123".to_string(),
            },
        ]);
        let parts = message_content_to_parts(content, &mut HashMap::new()).unwrap();
        assert_eq!(parts.len(), 2);
        // First part is text
        assert_eq!(parts[0].text.as_deref(), Some("describe this"));
        assert!(parts[0].inline_data.is_none());
        // Second part is image
        assert!(parts[1].text.is_none());
        let inline = parts[1].inline_data.as_ref().unwrap();
        assert_eq!(inline.mime_type, "image/jpeg");
        assert_eq!(inline.data, "abc123");
    }

    #[test]
    fn message_content_to_parts_serializes_correctly() {
        let content = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "look at this".to_string(),
            },
            ContentBlock::Image {
                source_type: "base64".to_string(),
                media_type: "image/png".to_string(),
                data: "AAAA".to_string(),
            },
        ]);
        let parts = message_content_to_parts(content, &mut HashMap::new()).unwrap();
        let json = serde_json::to_value(&parts).unwrap();
        let arr = json.as_array().unwrap();

        // Text part: only "text" field, no "inlineData"
        assert_eq!(arr[0]["text"], "look at this");
        assert!(arr[0].get("inlineData").is_none());

        // Image part: only "inlineData" field, no "text"
        assert!(arr[1].get("text").is_none());
        assert_eq!(arr[1]["inlineData"]["mimeType"], "image/png");
        assert_eq!(arr[1]["inlineData"]["data"], "AAAA");
    }

    #[test]
    fn tool_use_only_assistant_message_emits_function_call_part() {
        // Repro: an assistant message whose only block is a `ToolUse` used to
        // produce `parts: []` for that content, which Gemini rejects with
        // INVALID_ARGUMENT "Unable to submit request because it must include
        // at least one parts field". The conversation below is the exact
        // shape the agentic loop builds after the very first tool call.
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: MessageContent::Text("audit workspace".to_string()),
            },
            Message {
                role: "assistant".to_string(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "load_knowhow".to_string(),
                    input: serde_json::json!({"id": "system-knowhow/workspace-audit"}),
                    thought_signature: None,
                }]),
            },
            Message {
                role: "user".to_string(),
                content: MessageContent::Blocks(vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "t1".to_string(),
                        content: "knowhow body".to_string(),
                    },
                    ContentBlock::Text {
                        text: "Results above...".to_string(),
                    },
                ]),
            },
        ];

        let contents = messages_to_vertex_contents(messages).unwrap();

        assert_eq!(contents.len(), 3);
        for c in &contents {
            assert!(
                !c.parts.is_empty(),
                "Gemini rejects content with empty parts (role={})",
                c.role
            );
        }

        assert_eq!(contents[1].role, "model");
        assert_eq!(contents[1].parts.len(), 1);
        let fc = contents[1].parts[0]
            .function_call
            .as_ref()
            .expect("assistant ToolUse should map to a functionCall part");
        assert_eq!(fc.name, "load_knowhow");
        assert_eq!(fc.args["id"], "system-knowhow/workspace-audit");

        assert_eq!(contents[2].role, "user");
        assert_eq!(contents[2].parts.len(), 2);
        let fr = contents[2].parts[0]
            .function_response
            .as_ref()
            .expect("user ToolResult should map to a functionResponse part");
        assert_eq!(fr.name, "load_knowhow");
        assert_eq!(fr.response["content"], "knowhow body");
        assert_eq!(contents[2].parts[1].text.as_deref(), Some("Results above..."));
    }

    #[test]
    fn function_call_part_serializes_to_camel_case_wire_format() {
        let messages = vec![Message {
            role: "assistant".to_string(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "grep_files".to_string(),
                input: serde_json::json!({"pattern": "foo"}),
                thought_signature: None,
            }]),
        }];
        let contents = messages_to_vertex_contents(messages).unwrap();
        let json = serde_json::to_value(&contents).unwrap();
        let part = &json[0]["parts"][0];
        assert_eq!(part["functionCall"]["name"], "grep_files");
        assert_eq!(part["functionCall"]["args"]["pattern"], "foo");
        assert!(part.get("text").is_none());
        assert!(part.get("inlineData").is_none());
        assert!(part.get("functionResponse").is_none());
    }

    #[test]
    fn function_call_part_round_trips_thought_signature_to_wire() {
        // Gemini 3 attaches `thoughtSignature` to the first `functionCall`
        // part of every turn and hard-rejects the next request with HTTP 400
        // INVALID_ARGUMENT "Function call is missing a thought_signature"
        // unless the same signature is echoed back. The ContentBlock::ToolUse
        // we built from the prior turn's response must carry the signature
        // through to the VertexPart on the next request.
        let messages = vec![Message {
            role: "assistant".to_string(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "glob_files".to_string(),
                input: serde_json::json!({"pattern": "**/*.rs"}),
                thought_signature: Some("Ax9z-encrypted-sig".to_string()),
            }]),
        }];
        let contents = messages_to_vertex_contents(messages).unwrap();
        let json = serde_json::to_value(&contents).unwrap();
        let part = &json[0]["parts"][0];
        assert_eq!(part["functionCall"]["name"], "glob_files");
        assert_eq!(
            part["thoughtSignature"], "Ax9z-encrypted-sig",
            "Vertex AI rejects the request with HTTP 400 if the signature \
             isn't echoed on the functionCall part"
        );
    }

    #[test]
    fn function_call_part_omits_thought_signature_when_none() {
        // Older models (Gemini 2.5, Claude through this provider) don't emit
        // a signature. Emitting `"thoughtSignature": null` would be wasteful;
        // omit the field entirely.
        let messages = vec![Message {
            role: "assistant".to_string(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "glob_files".to_string(),
                input: serde_json::json!({"pattern": "**/*.rs"}),
                thought_signature: None,
            }]),
        }];
        let contents = messages_to_vertex_contents(messages).unwrap();
        let json = serde_json::to_value(&contents).unwrap();
        let part = &json[0]["parts"][0];
        assert!(
            part.get("thoughtSignature").is_none(),
            "must omit field when no signature: {}",
            part
        );
    }

    #[test]
    fn vertex_response_part_reads_thought_signature_from_wire() {
        // Wire shape from Gemini 3's generateContent response. The signature
        // sits on the same part as the functionCall and must be captured into
        // ToolCall so the next request can echo it back.
        let body = r#"{
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "glob_files",
                            "args": {"pattern": "**/*.rs"}
                        },
                        "thoughtSignature": "Ax9z-encrypted-sig"
                    }]
                }
            }]
        }"#;
        let parsed: VertexResponse = serde_json::from_str(body).unwrap();
        let part = &parsed.candidates.unwrap()[0].content.parts[0];
        assert_eq!(
            part.thought_signature.as_deref(),
            Some("Ax9z-encrypted-sig"),
            "Gemini 3's signature on the functionCall part must be captured"
        );
    }

    /// Gemini's empty-completion case is the regression target — a
    /// candidate with no parts (or only thought parts), `finishReason`
    /// `"SAFETY"`/`"MAX_TOKENS"`/etc., and a populated `usageMetadata`.
    /// The previous mapping discarded both fields and we couldn't
    /// distinguish "model said nothing" from a content-policy block or a
    /// token-budget cutoff. The pure helper must preserve both.
    #[test]
    fn build_gemini_llm_response_preserves_empty_completion_metadata() {
        let body = r#"{
            "candidates": [{
                "content": {"parts": []},
                "finishReason": "SAFETY"
            }],
            "usageMetadata": {
                "promptTokenCount": 1234,
                "candidatesTokenCount": 0,
                "totalTokenCount": 1234
            }
        }"#;
        let parsed: VertexResponse = serde_json::from_str(body).unwrap();
        let resp = build_gemini_llm_response(parsed);

        assert!(resp.content.is_none(), "empty completion has no text");
        assert!(resp.tool_calls.is_empty());
        assert_eq!(
            resp.stop_reason.as_deref(),
            Some("SAFETY"),
            "finishReason MUST survive — distinguishes silence from a content-policy block"
        );
        assert_eq!(resp.input_tokens, Some(1234));
        assert_eq!(resp.output_tokens, Some(0));
    }

    /// Normal `STOP` completion path — both finishReason and usage flow
    /// through the same helper so cross-provider analytics has parity with
    /// the Claude-via-Vertex path that captures these via `message_start`
    /// + `message_delta`.
    #[test]
    fn build_gemini_llm_response_captures_stop_and_usage_on_success() {
        let body = r#"{
            "candidates": [{
                "content": {"parts": [{"text": "ok"}]},
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 42,
                "candidatesTokenCount": 7,
                "totalTokenCount": 49
            }
        }"#;
        let parsed: VertexResponse = serde_json::from_str(body).unwrap();
        let resp = build_gemini_llm_response(parsed);

        assert_eq!(resp.content.as_deref(), Some("ok"));
        assert_eq!(resp.stop_reason.as_deref(), Some("STOP"));
        assert_eq!(resp.input_tokens, Some(42));
        assert_eq!(resp.output_tokens, Some(7));
        assert!(
            should_emit_gemini_text_delta(&resp),
            "text-only Gemini responses should still stream their final answer"
        );
    }

    #[test]
    fn gemini_text_delta_is_suppressed_when_function_call_is_present() {
        let body = r#"{
            "candidates": [{
                "content": {"parts": [
                    {"text": "No introductory transition, no \"Okay, I see\". Just act."},
                    {"functionCall": {
                        "name": "read_file",
                        "args": {"path": "notes.md"}
                    }}
                ]},
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 42,
                "candidatesTokenCount": 11,
                "totalTokenCount": 53
            }
        }"#;
        let parsed: VertexResponse = serde_json::from_str(body).unwrap();
        let resp = build_gemini_llm_response(parsed);

        assert_eq!(
            resp.content.as_deref(),
            Some("No introductory transition, no \"Okay, I see\". Just act.")
        );
        assert_eq!(resp.tool_calls.len(), 1);
        assert!(
            !should_emit_gemini_text_delta(&resp),
            "Gemini tool-call preambles must not be surfaced as streamed assistant prose"
        );
    }

    /// MAX_TOKENS truncation: model spent the whole budget without ending
    /// naturally. Output text may or may not be present; finishReason MUST
    /// surface so the empty-completion diagnostic can flag truncation.
    #[test]
    fn build_gemini_llm_response_surfaces_max_tokens_truncation() {
        let body = r#"{
            "candidates": [{
                "content": {"parts": []},
                "finishReason": "MAX_TOKENS"
            }],
            "usageMetadata": {
                "promptTokenCount": 60000,
                "candidatesTokenCount": 32768,
                "totalTokenCount": 92768
            }
        }"#;
        let parsed: VertexResponse = serde_json::from_str(body).unwrap();
        let resp = build_gemini_llm_response(parsed);

        assert_eq!(resp.stop_reason.as_deref(), Some("MAX_TOKENS"));
        assert_eq!(resp.output_tokens, Some(32768));
    }
}
