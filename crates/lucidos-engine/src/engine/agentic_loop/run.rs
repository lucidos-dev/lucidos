//! The agentic-loop driver (`run_agentic_loop`): call LLM → parse → execute
//! tools → repeat. A single cohesive `loop` with labeled control flow and
//! interdependent mutable state; left as one method.

use crate::llm::provider::LlmResponse;
use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use crate::llm::{ContentBlock, Message, MessageContent};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::super::context::{
    estimate_message_chars, estimate_tokens_from_chars, replace_image_blocks,
    trim_context_if_needed,
};
use super::super::types::*;
use super::super::LucidosEngine;
use super::helpers::*;

impl LucidosEngine {
    /// The agentic loop: call LLM → parse response → execute tools → repeat.
    ///
    /// Returns ProcessResult on completion.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_agentic_loop(
        &self,
        messages: &mut Vec<Message>,
        system_prompt: &str,
        tools: &[ToolDefinition],
        request_id: Uuid,
        thread_id: Uuid,
        response_channel: Option<crate::engine::thread_events::EventChannel>,
        message_budget: usize,
        extraction_ctx: &str,
        // `(description, model)` — the model name is needed so the
        // `ImageDescribed` event records which Flash model produced the text.
        mut image_description_handle: Option<
            tokio::task::JoinHandle<Option<(String, String)>>,
        >,
        origin_id: Uuid,
        proposed_change: &mut bool,
        user_images: Option<&[crate::api::ChatImage]>,
        device_id: Option<&str>,
        model_override: Option<&str>,
        reasoning_effort: Option<&str>,
        cancel_token: &CancellationToken,
        injection_rx: &mut mpsc::UnboundedReceiver<super::super::InjectedPrompt>,
        // Set to true at every terminator emission so the post-loop guard
        // in `chat::process` can skip its `payload->>'request_event_id'`
        // existence check on the success path. The check has no functional
        // index and would walk every event in long-lived threads otherwise.
        terminator_emitted: &mut bool,
        capture_seed: ContextCaptureSeed<'_>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        // EventMeta for this request — all persisted events in this cycle share the same context
        let meta = crate::engine::thread_events::EventMeta {
            request_event_id: Some(origin_id),
            channel: response_channel,
            ..crate::engine::thread_events::EventMeta::NONE
        };

        let model_str = model_override.unwrap_or(self.llm.default_model());
        let effective_model = (!model_str.is_empty()).then(|| model_str.to_string());
        let effective_effort = reasoning_effort.map(|s| s.to_string());
        let capture_window = super::super::context::context_window_for(capture_seed.model);

        let mut iterations = 0;
        // Capture before the loop pushes assistant/tool messages and shifts the index.
        // Maintained across iterations: trim pass 2 may remove older messages, which
        // shifts the captured index down by the number removed (handled below).
        let mut user_message_idx = messages.len().saturating_sub(1);
        let mut images: Vec<String> = Vec::new(); // Track screenshots created during this request
        let mut last_tool_call: Option<(String, String)> = None; // (tool_name, key) - key derived by derive_call_key
        let mut consecutive_same_call = 0;
        let mut cached_list_files: Option<String> = None;
        let mut modified_app_uis: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        // Agent loop
        loop {
            iterations += 1;
            if cancel_token.is_cancelled() {
                crate::engine::thread_events::emit_response_canceled(
                    &self.event_bus,
                    &self.pool,
                    thread_id,
                    crate::engine::thread_events::CancelCause::UserStop,
                    String::new(),
                    images.clone(),
                    effective_model.clone(),
                    effective_effort.clone(),
                    meta_with_cancel_actor(self, thread_id, &meta),
                    "[AgenticLoop] ResponseCanceled (cancel pre-iter)",
                )
                .await;
                *terminator_emitted = true;
                return Ok(ProcessResult {
                    response: String::new(),
                    steps: vec![],
                    images,
                    request_id,
                    thread_id,
                    proposed_change: *proposed_change,
                    auto_apply: false,
                    orphaned_injections: vec![],
                });
            }

            if iterations > MAX_ITERATIONS {
                let msg = emit_iteration_cap_response_generated(
                    &self.event_bus,
                    thread_id,
                    &meta,
                    images.clone(),
                    effective_model.clone(),
                    effective_effort.clone(),
                )
                .await;
                *terminator_emitted = true;
                return Ok(ProcessResult {
                    response: msg,
                    steps: vec![],
                    images,
                    request_id,
                    thread_id,
                    proposed_change: *proposed_change,
                    auto_apply: false,
                    orphaned_injections: vec![],
                });
            }

            // Pin the current turn's user message so pass 2 cannot drop it.
            // The recent-tail rule alone fails once enough tool iterations
            // shift the message out of the last PRESERVE_RECENT_MESSAGES
            // slots; losing it strips the request line and the iter-1
            // image-description placeholder from every subsequent call,
            // making the model report "I lost track of what you asked".
            let removed_count =
                trim_context_if_needed(messages, message_budget, Some(user_message_idx));
            let trimmed = removed_count > 0;
            // Pass 2 of trim removes oldest messages from index 1; the protected
            // index guard ensures every removal sits strictly below
            // user_message_idx, so it shifts down by removed_count.
            user_message_idx = user_message_idx.saturating_sub(removed_count);
            // Safety net: validate tool_use/tool_result pairing after trimming.
            // The primary fix ensures correct block ordering, but this catches any
            // edge case where pairing breaks (trimming bugs, injection ordering, etc.)
            crate::llm::validate::validate_tool_use_pairing(messages);
            // Always measure AFTER trimming — pass 0 strips images even when pass 2
            // removes no messages, so pre-trim chars can be wildly inflated.
            let context_chars: usize = messages.iter().map(estimate_message_chars).sum();

            // chars-to-tokens at the same 1.5 chars/token rate the trim budget
            // assumes (see `agent_context_char_budget`). The previous chars/4
            // estimate matched English prose but undercounted JSON-heavy tool
            // results by ~2.4× — the May 25 workspace-learning trigger reported
            // 649 K tokens to the UI then sent 1.54 M to the API. Matching the
            // budget's ratio gives an honest signal: if the displayed estimate
            // approaches the model's context window, the request really IS that
            // close to the cap.
            let context_tokens = estimate_tokens_from_chars(context_chars);
            let context_messages = messages.len();
            let trimmed_str = if trimmed { " (trimmed)" } else { "" };
            self.event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ThoughtStreamed {
                            text: format!(
                                "Context: {} tokens, {} messages{}",
                                context_tokens, context_messages, trimmed_str
                            ),
                        },
                        meta: meta.clone(),
                    },
                    "[AgenticLoop] ThoughtStreamed (context summary)",
                )
                .await;

            // Create token streaming callback — buffers text, flushes as HTML at paragraph boundaries
            let raw_buffer = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

            // Incremental text persistence: sync callback sends deltas to async persist task
            let (persist_tx, mut persist_rx) = mpsc::unbounded_channel::<String>();
            let last_persisted_len = std::sync::Arc::new(std::sync::Mutex::new(0usize));
            {
                let bus = self.event_bus.clone();
                let persist_meta = meta.clone();
                tokio::spawn(async move {
                    while let Some(delta) = persist_rx.recv().await {
                        bus.emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::TextStreamed {
                                    text: delta,
                                },
                                meta: persist_meta.clone(),
                            },
                            "[AgenticLoop] TextStreamed delta",
                        )
                        .await;
                    }
                });
            }

            let token_cb: Option<crate::llm::TokenCallback> = {
                let sender = self.event_bus.sender();
                let buf = raw_buffer.clone();
                let persist = persist_tx.clone();
                let persisted_len = last_persisted_len.clone();
                Some(Box::new(move |delta: &str| {
                    let mut text = buf.lock().unwrap();
                    text.push_str(delta);
                    // Mid-stream defense against Opus 4.7 emitting
                    // `<ask_user_question>...</ask_user_question>` as text
                    // instead of a structured tool call. Once the opening
                    // fragment appears in the buffer, stop flushing deltas
                    // so the user doesn't see the raw tag in their live
                    // view. The post-response repair path in the no-
                    // tool-calls branch below strips the tag, synthesises
                    // a real tool call, and routes the question through
                    // `walk_question_batch`. See `inline_question_repair`.
                    if crate::engine::inline_question_repair::buffer_contains_inline_tag(&text) {
                        return;
                    }
                    if should_flush(&text) {
                        let _ = sender.send(crate::engine::event_bus::EmittedEvent {
                            event_id: uuid::Uuid::new_v4(),
                            seq: None,
                            created: chrono::Utc::now(),
                            typed: crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::CumulativeTextUpdated {
                                    text: text.clone(),
                                },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            aggregate: None,
                        });
                        // Persist new text since last persistence point
                        let mut last = persisted_len.lock().unwrap();
                        if text.len() > *last {
                            let new_text = text[*last..].to_string();
                            let _ = persist.send(new_text);
                            *last = text.len();
                        }
                    }
                }) as crate::llm::TokenCallback)
            };

            let call_tools = tools.to_vec();

            // Race LLM call against cancel token so stop button works immediately
            let llm_future = self.llm.chat(
                messages.clone(),
                call_tools,
                model_override,
                Some(system_prompt),
                token_cb,
                reasoning_effort,
            );
            let cancel_future = cancel_token.cancelled();

            let response = tokio::select! {
                result = llm_future => {
                    match result {
                        Ok(r) => r,
                        Err(e) => {
                            self.event_bus.emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::LlmCallRetried {
                                        reason: format!("Request failed: {}", e),
                                    },
                                    meta: crate::engine::thread_events::EventMeta::NONE,
                                },
                                "[AgenticLoop] LlmCallRetried",
                            ).await;
                            self.event_bus.emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                        error: e.to_string(),
                                    },
                                    meta: meta.clone(),
                                },
                                "[AgenticLoop] ResponseFailed",
                            ).await;
                            *terminator_emitted = true;
                            return Err(e);
                        }
                    }
                }
                _ = cancel_future => {
                    let partial = raw_buffer.lock().unwrap().clone();
                    // Persist any remaining un-persisted text
                    {
                        let last = *last_persisted_len.lock().unwrap();
                        if partial.len() > last {
                            let remaining = &partial[last..];
                            self.event_bus.emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::TextStreamed {
                                        text: remaining.to_string(),
                                    },
                                    meta: meta.clone(),
                                },
                                "[AgenticLoop] TextStreamed flush on cancel",
                            ).await;
                        }
                    }
                    drop(persist_tx);
                    crate::engine::thread_events::emit_response_canceled(
                        &self.event_bus,
                        &self.pool,
                        thread_id,
                        crate::engine::thread_events::CancelCause::UserStop,
                        partial.clone(),
                        images.clone(),
                        effective_model.clone(),
                        effective_effort.clone(),
                        meta_with_cancel_actor(self, thread_id, &meta),
                        "[AgenticLoop] ResponseCanceled",
                    )
                    .await;
                    *terminator_emitted = true;
                    return Ok(ProcessResult {
                        response: partial,
                        steps: vec![],
                        images,
                        request_id,
                        thread_id,
                        proposed_change: *proposed_change,
                        auto_apply: false,
                        orphaned_injections: vec![],
                    });
                }
            };

            // `usage` only when the provider reported it (Anthropic);
            // OpenAI/Gemini stay None and the conservative chars*2/3 estimate
            // stands (see `estimate_tokens_from_chars`).
            //
            // Sections describe the prompt-time breakdown (system + memory
            // + history + … + user message). All non-system sections are
            // already concatenated into messages[0].content, so summing
            // them and `context_chars` would double-count. The Conversation
            // section instead carries the *delta* — bytes added by the
            // tool loop on iter 2+ — so the section list sums to the live
            // total: system_prompt + context_chars.
            let static_total: usize =
                capture_seed.sections.iter().map(|s| s.char_count).sum();
            let system_chars: usize = capture_seed
                .sections
                .iter()
                .find(|s| s.name == "System Instructions")
                .map(|s| s.char_count)
                .unwrap_or(0);
            let bundled_total = static_total - system_chars;
            let conversation_extra = context_chars.saturating_sub(bundled_total);
            // When `capture_body` is on, fill the Conversation body so the
            // modal shows what was actually sent (assistant text + tool I/O
            // accumulated by the loop). Capped at SECTION_PERSIST_MAX (8 KB
            // — same cap chat::process applies to other section bodies) to
            // keep large tool outputs from bloating every events row.
            const CONVERSATION_PERSIST_MAX: usize = 8_000;
            let conversation_body = capture_seed
                .capture_body
                .then(|| {
                    let serialized = serialize_messages_for_capture(messages);
                    if serialized.len() > CONVERSATION_PERSIST_MAX {
                        super::super::context::truncate_head_tail(
                            &serialized,
                            CONVERSATION_PERSIST_MAX,
                        )
                    } else {
                        serialized
                    }
                });
            let conversation = crate::engine::ContextSection {
                name: "Conversation".to_string(),
                content: conversation_body,
                char_count: conversation_extra,
                role: crate::engine::ContextRole::User,
                group: None,
            };
            let estimated_total_tokens: usize =
                estimate_tokens_from_chars(system_chars + context_chars);
            let iter_sections: Vec<_> = capture_seed
                .sections
                .iter()
                .cloned()
                .chain(std::iter::once(conversation))
                .collect();
            let usage = response.input_tokens.map(|input_tokens| {
                crate::engine::ApiUsage {
                    input_tokens,
                    output_tokens: response.output_tokens.unwrap_or(0),
                    cache_read_tokens: response.cache_read_tokens.unwrap_or(0),
                    cache_creation_tokens: response.cache_creation_tokens.unwrap_or(0),
                }
            });
            self.event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ContextCaptured {
                            producer: crate::engine::ContextProducer::MainLlm,
                            model: capture_seed.model.to_string(),
                            context_window: capture_window,
                            sections: iter_sections,
                            tools: capture_seed.tools.to_vec(),
                            estimated_total_tokens,
                            usage,
                            trimmed,
                        },
                        meta: meta.clone(),
                    },
                    "[AgenticLoop] ContextCaptured",
                )
                .await;

            // Post-response repair: Opus 4.7 sometimes emits
            // `<ask_user_question>...</ask_user_question>` as inline text
            // instead of a structured tool call. Detect that case BEFORE
            // the final flush so the cleaned text (tag removed) is what
            // streams to the frontend AND persists. The synthesised tool
            // call is appended further down so the existing tool-execution
            // branch takes over from `if response.tool_calls.is_empty()`.
            // See `inline_question_repair` for the detection contract and
            // why the prompt-side rule alone is insufficient.
            let inline_repair = response
                .content
                .as_deref()
                .and_then(crate::engine::inline_question_repair::detect_inline_ask_user_question);
            let mut response = response;
            if let Some(ref d) = inline_repair {
                response.content = if d.cleaned_text.is_empty() {
                    None
                } else {
                    Some(d.cleaned_text.clone())
                };
                let synth_id = format!("synth-aq-{}", uuid::Uuid::new_v4());
                response.tool_calls.push(crate::llm::ToolCall {
                    id: synth_id.clone(),
                    name: tn::ASK_USER_QUESTION.to_string(),
                    arguments: serde_json::json!({ "questions": d.questions_json }),
                    thought_signature: None,
                });
                crate::log!(
                    "[InlineQuestionRepair] thread={} synthesised tool call from inline <ask_user_question> tag (synth_id={})",
                    thread_id,
                    synth_id,
                );
            }

            // Final flush — send any remaining buffered text and persist
            // remainder. When inline repair fired, use the cleaned text
            // (tag-stripped) so the frontend's live view and persisted
            // TextStreamed events both reflect the repaired body.
            let (flush_text, remaining_to_persist) = {
                let raw = raw_buffer.lock().unwrap();
                let effective: &str = inline_repair
                    .as_ref()
                    .map(|d| d.cleaned_text.as_str())
                    .unwrap_or(raw.as_str());
                let cloned = if effective.is_empty() {
                    None
                } else {
                    Some(effective.to_string())
                };
                let last = *last_persisted_len.lock().unwrap();
                // `get(last..)` returns None when `last` exceeds length
                // (cleaned text shorter than what was already streamed —
                // happens when suppression caught the tag mid-flight) or
                // lands off a char boundary. Either way, no further delta
                // to persist.
                let remaining = effective
                    .get(last..)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                (cloned, remaining)
            };
            if let Some(flush) = flush_text {
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::CumulativeTextUpdated {
                                text: flush,
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[AgenticLoop] CumulativeTextUpdated final flush",
                    )
                    .await;
            }
            if let Some(remaining) = remaining_to_persist {
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::TextStreamed {
                                text: remaining,
                            },
                            meta: meta.clone(),
                        },
                        "[AgenticLoop] TextStreamed final persist",
                    )
                    .await;
            }
            drop(persist_tx);

            // No more tool calls - we have the final answer
            if response.tool_calls.is_empty() {
                // Refresh any app UIs that were modified during the tool loop
                for app_id in &modified_app_uis {
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::AppUiRefreshRequested {
                                    app_id: app_id.clone(),
                                },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[AgenticLoop] AppUiRefreshRequested (post-loop)",
                        )
                        .await;
                }
                let cleaned = response
                    .content
                    .as_deref()
                    .map(|c| self.clean_response(c))
                    .filter(|c| !c.is_empty());

                if let Some(clean_response) = cleaned {
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event:
                                    crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                                        text: clean_response.clone(),
                                        images: images.clone(),
                                        model: effective_model.clone(),
                                        reasoning_effort: effective_effort.clone(),
                                    },
                                meta: meta.clone(),
                            },
                            "[AgenticLoop] ResponseGenerated",
                        )
                        .await;

                    *terminator_emitted = true;
                    return Ok(ProcessResult {
                        response: clean_response,
                        steps: vec![],
                        images,
                        request_id,
                        thread_id,
                        proposed_change: *proposed_change,
                        auto_apply: false,
                        orphaned_injections: vec![],
                    });
                }

                // Empty completion (no content, no tool calls). Always a
                // failure from the user's perspective — surface via
                // ResponseFailed with full diagnostic in the error string.
                // See `empty_completion_hint` for the branch logic
                // distinguishing intentional silence, parser miss, and
                // truncation.
                let stop_reason = response.stop_reason.as_deref().unwrap_or("unknown");
                let output_tokens_n = response.output_tokens.unwrap_or(0);
                let thinking_chars_n = response.thinking_chars.unwrap_or(0);
                let unknown_sse_dropped = response.unknown_sse_dropped;
                let output_tokens = response
                    .output_tokens
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let thinking_chars = response
                    .thinking_chars
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let hint = empty_completion_hint(
                    stop_reason,
                    output_tokens_n,
                    thinking_chars_n,
                    unknown_sse_dropped,
                );
                let dropped_suffix = if unknown_sse_dropped > 0 {
                    format!(", unknown_sse_dropped: {}", unknown_sse_dropped)
                } else {
                    String::new()
                };
                let error = format!(
                    "Model returned no response (stop_reason: {}, output_tokens: {}, thinking_chars: {}{}, model: {}){}.",
                    stop_reason,
                    output_tokens,
                    thinking_chars,
                    dropped_suffix,
                    effective_model.as_deref().unwrap_or("unknown"),
                    hint,
                );
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                error,
                            },
                            meta: meta.clone(),
                        },
                        "[AgenticLoop] ResponseFailed (empty completion)",
                    )
                    .await;

                *terminator_emitted = true;
                return Ok(ProcessResult {
                    response: String::new(),
                    steps: vec![],
                    images,
                    request_id,
                    thread_id,
                    proposed_change: *proposed_change,
                    auto_apply: false,
                    orphaned_injections: vec![],
                });
            }

            // Execute each tool call
            let mut tool_outputs = Vec::new();
            let mut had_errors = false;

            // Helper: push a circuit-breaker response into the messages array.
            // Must add the assistant's tool_use message AND matching tool_result blocks
            // to maintain proper alternation and tool_use/tool_result pairing required
            // by the Claude API. Without this, we'd get "tool_use ids were found without
            // tool_result blocks" errors.
            let push_circuit_breaker =
                |messages: &mut Vec<Message>, response: &LlmResponse, result_text: &str| {
                    // 1. Assistant message with tool_use blocks (from the LLM's response)
                    let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
                    if let Some(content_text) = &response.content {
                        let t: String = content_text.clone();
                        assistant_blocks.push(ContentBlock::Text { text: t });
                    }
                    for tc in &response.tool_calls {
                        assistant_blocks.push(ContentBlock::ToolUse {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            input: tc.arguments.clone(),
                            thought_signature: tc.thought_signature.clone(),
                        });
                    }
                    messages.push(Message {
                        role: "assistant".to_string(),
                        content: MessageContent::Blocks(assistant_blocks),
                    });

                    // 2. User message with tool_result blocks (one per tool_use) + instruction
                    let mut result_blocks: Vec<ContentBlock> = response
                        .tool_calls
                        .iter()
                        .map(|tc| ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: result_text.to_string(),
                        })
                        .collect();
                    result_blocks.push(ContentBlock::Text {
                        text: "Use the results you already have and give your final answer now."
                            .to_string(),
                    });
                    messages.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Blocks(result_blocks),
                    });
                };

            // Check for consecutive duplicate tool calls (e.g., list_files loop)
            if response.tool_calls.len() == 1 {
                let current_tool = &response.tool_calls[0].name;
                let current_args = &response.tool_calls[0].arguments;

                // Track by tool name + target for dedup detection
                let call_key = derive_call_key(current_tool, current_args);

                let current_call = (current_tool.clone(), call_key.clone());
                if Some(&current_call) == last_tool_call.as_ref() {
                    consecutive_same_call += 1;
                } else {
                    consecutive_same_call = 1;
                    last_tool_call = Some(current_call);
                }

                // If list_files called 2+ times, return cached result with strong stop message
                if current_tool == tn::LIST_FILES && consecutive_same_call >= 2 {
                    if let Some(ref cached) = cached_list_files {
                        log!(
                            "[AgentLoop] Returning cached list_files result (call #{})",
                            consecutive_same_call
                        );
                        if consecutive_same_call >= 4 {
                            log!(
                                "[AgentLoop] Force-breaking tool loop after {} repeated list_files attempts",
                                consecutive_same_call
                            );
                            let msg = "I listed the available files but wasn't able to complete the task. Could you give me more specific instructions?";
                            self.event_bus.emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                                        text: msg.to_string(), images: images.clone(), model: effective_model.clone(), reasoning_effort: effective_effort.clone(),
                                    },
                                    meta: meta.clone(),
                                },
                                "[AgenticLoop] ResponseGenerated (force-break)",
                            ).await;
                            *terminator_emitted = true;
                            return Ok(ProcessResult {
                                response: msg.to_string(),
                                steps: vec![],
                                images,
                                request_id,
                                thread_id,
                                proposed_change: *proposed_change,
                                auto_apply: false,
                                orphaned_injections: vec![],
                            });
                        }
                        let cached_result = format!(
                            "[list_files result - CACHED, DO NOT CALL AGAIN]\n{}\n\nSTOP: You have the file list. DO NOT call list_files again. Proceed with your task NOW.",
                            cached
                        );
                        push_circuit_breaker(messages, &response, &cached_result);
                        continue;
                    }
                }

                // If read_file called 3+ times with the SAME args (path + window),
                // block it. Different windows of the same file bucket separately
                // via `derive_call_key`, so legitimate paging doesn't trip this.
                if current_tool == tn::READ_FILE && consecutive_same_call >= 3 {
                    // Pull the bare path for human-readable messages — `call_key`
                    // includes the window suffix and is meant for bucketing only.
                    let path_for_msg = current_args
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    log!(
                        "[AgentLoop] Blocking repeated read_file of '{}' (call #{})",
                        path_for_msg,
                        consecutive_same_call
                    );
                    // After 5 blocked attempts, force-break out of the loop
                    if consecutive_same_call >= 5 {
                        log!(
                            "[AgentLoop] Force-breaking tool loop after {} repeated read_file attempts",
                            consecutive_same_call
                        );
                        let msg = format!("I read the file `{}` but wasn't able to complete the task with it. Could you give me more specific instructions?", path_for_msg);
                        self.event_bus
                            .emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event:
                                        crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                                            text: msg.clone(),
                                            images: images.clone(),
                                            model: effective_model.clone(),
                                            reasoning_effort: effective_effort.clone(),
                                        },
                                    meta: meta.clone(),
                                },
                                "[AgenticLoop] ResponseGenerated (read_file force-break)",
                            )
                            .await;
                        *terminator_emitted = true;
                        return Ok(ProcessResult {
                            response: msg,
                            steps: vec![],
                            images,
                            request_id,
                            thread_id,
                            proposed_change: *proposed_change,
                            auto_apply: false,
                            orphaned_injections: vec![],
                        });
                    }
                    let stop_msg = format!("STOP: You've read '{}' with the same window multiple times. The content hasn't changed. Use the information you have and proceed with your task.", path_for_msg);
                    push_circuit_breaker(messages, &response, &stop_msg);
                    continue;
                }

                // Generic circuit breaker: any tool called 3+ times in a row
                // Excluded: write tools (multiple edits normal), browser tools (multi-step
                // browsing workflows are normal). All still bounded by MAX_ITERATIONS.
                //
                // `run_python` / `run_python_background` were previously excluded under the
                // rationale "sequential scripts processing different data are normal" — true,
                // but the bucket key for those tools is the first non-blank non-comment
                // non-import line of `code` (see `derive_call_key` + `python_call_key`),
                // so different scripts get different buckets and don't trip. The guard
                // catches VERBATIM retries (same first actionable line three times in a
                // row) — e.g. `time.sleep(60)` thrice, or two failing import attempts
                // copy-pasted with a small tweak past char 80. It does NOT catch
                // escalating-argument retries (`time.sleep(60)` then `time.sleep(120)`)
                // — those each bucket separately. The `bash_output(wait_secs)` server-
                // side block is the structural fix for sleep-poll; this guard is the
                // last-line defense for the verbatim-retry case.
                let excluded = matches!(
                    current_tool.as_str(),
                    tn::EDIT_FILE
                        | tn::WRITE_FILE
                        | tn::WEB_SEARCH
                        | tn::BROWSER_OPEN
                        | tn::BROWSER_EXTRACT
                        | tn::BROWSER_CLICK
                        | tn::BROWSER_SCREENSHOT
                        | tn::BROWSER_CLOSE
                        | tn::BROWSER_FORGET_LOGIN
                        | tn::BROWSER_CLEAR_DATA
                );
                if consecutive_same_call >= 3 && !excluded {
                    let target = if call_key.is_empty() {
                        current_tool.to_string()
                    } else {
                        call_key.clone()
                    };

                    if consecutive_same_call >= 5 {
                        // Hard break after 5 — the LLM ignored warnings
                        log!(
                            "[AgentLoop] Force-breaking loop: {} called {} times on '{}'",
                            current_tool,
                            consecutive_same_call,
                            target
                        );
                        let msg = format!("I tried to process `{}` but it failed repeatedly. The error may need to be resolved before retrying.", target);
                        self.event_bus
                            .emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event:
                                        crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                                            text: msg.clone(),
                                            images: images.clone(),
                                            model: effective_model.clone(),
                                            reasoning_effort: effective_effort.clone(),
                                        },
                                    meta: meta.clone(),
                                },
                                "[AgenticLoop] ResponseGenerated (generic force-break)",
                            )
                            .await;
                        *terminator_emitted = true;
                        return Ok(ProcessResult {
                            response: msg,
                            steps: vec![],
                            images,
                            request_id,
                            thread_id,
                            proposed_change: *proposed_change,
                            auto_apply: false,
                            orphaned_injections: vec![],
                        });
                    }

                    // Soft break at 3-4 — warn the LLM and let it continue
                    log!(
                        "[AgentLoop] Warning LLM: {} called {} times on '{}'",
                        current_tool,
                        consecutive_same_call,
                        target
                    );
                    let stop_msg = format!(
                        "STOP: You've called {} on '{}' {} times. Do NOT call it again. Use the results you already have and give your final answer now.",
                        current_tool, target, consecutive_same_call
                    );
                    push_circuit_breaker(messages, &response, &stop_msg);
                    continue;
                }
            } else {
                consecutive_same_call = 0;
                last_tool_call = None;
            }

            for tool_call in &response.tool_calls {
                let tool_desc = self.describe_tool(&tool_call.name, &tool_call.arguments);
                log!(
                    "[AgentLoop] Step {}/{}: {}",
                    iterations,
                    MAX_ITERATIONS,
                    tool_desc
                );

                // Persist + broadcast ToolCalled. Capture the event_id so spawn-style
                // tools (run_thread, run_claude) can record which tool call triggered
                // the spawn — this becomes the new thread's `spawning_event_id`.
                // Mask any postgres password the LLM hardcoded into a `bash`
                // command (or other tool) before persisting; see
                // `core::redact_postgres_secrets_in_json`.
                let mut redacted_args = tool_call.arguments.clone();
                crate::core::redact_postgres_secrets_in_json(&mut redacted_args);
                let tool_called_event_id = self
                    .event_bus
                    .emit_for_id(crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ToolCalled {
                            name: tool_call.name.clone(),
                            description: self.describe_tool(&tool_call.name, &tool_call.arguments),
                            args: redacted_args,
                        },
                        meta: meta.clone(),
                    })
                    .await;

                let outcome: crate::engine::tools::ToolOutcome = if let Some(r) = self
                    .handle_special_tool(
                        &tool_call.name,
                        &tool_call.arguments,
                        thread_id,
                        user_images,
                        device_id,
                        tool_called_event_id,
                        &tool_call.id,
                    )
                    .await
                {
                    // handle_special_tool still returns `String`; lift via
                    // the legacy `Error:` prefix until its sites are
                    // migrated to typed `Err`.
                    crate::engine::tools::lift_legacy_string(r)
                } else {
                    // run_python / run_bash / http_request / mcp__* and friends
                    // dispatch through here. These are the tools that can park
                    // for minutes on a hung subprocess or a no-timeout reqwest
                    // client, so the cancel-aware wrapper is mandatory: dropping
                    // the inner future on cancel SIGKILLs the OS child (via
                    // `kill_on_drop(true)` on the Command) and lets the outer
                    // loop iterate to its pre-iter `is_cancelled()` check, which
                    // emits ResponseCanceled. Without the wrapper, a
                    // `urllib.request.urlopen()` with no timeout ignored cancel
                    // forever and the thread stayed `running` until the engine
                    // restarted.
                    run_tool_with_cancel(
                        self.execute_tool(
                            &tool_call.name,
                            &tool_call.arguments,
                            extraction_ctx,
                            request_id,
                            device_id,
                            cancel_token,
                            thread_id,
                        ),
                        cancel_token,
                    )
                    .await
                };
                let (result, is_error) = match outcome {
                    Ok(text) => (text, false),
                    Err(text) => (text, true),
                };

                // Track app UI modifications for deferred refresh before final response
                // Path format: apps/{app_id}/{file}
                if (tool_call.name == tn::WRITE_FILE || tool_call.name == tn::EDIT_FILE)
                    && !is_error
                {
                    let path = tool_call
                        .arguments
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if let Some(rest) = path.strip_prefix("apps/") {
                        if let Some(slash) = rest.find('/') {
                            let app_id = &rest[..slash];
                            modified_app_uis.insert(app_id.to_string());
                        }
                    }
                }

                // Clear from deferred refresh if explicitly refreshed via tool
                if tool_call.name == tn::REFRESH_APP {
                    if let Some(app_id) = tool_call.arguments.get("app_id").and_then(|v| v.as_str())
                    {
                        modified_app_uis.remove(app_id);
                    }
                }

                // Cache list_files result to prevent loops
                if tool_call.name == tn::LIST_FILES && !is_error {
                    cached_list_files = Some(result.clone());
                }

                // Track screenshots for HTML embedding
                if tool_call.name == tn::BROWSER_SCREENSHOT && !is_error {
                    // Extract path from "Screenshot saved to artifacts/{path} ({} bytes)"
                    if let Some(start) = result.find("artifacts/") {
                        if let Some(end) = result[start..].find(" (") {
                            let path = &result[start + 10..start + end]; // skip "artifacts/"
                            images.push(path.to_string());
                        }
                    }
                }

                // Extract generated image base64 from tool result (if present)
                let mut tool_result_images: Vec<String> = vec![];
                let mut tool_result_text;
                if let Some(rest) = result.strip_prefix("[GENERATED_IMAGE:") {
                    if let Some(end_bracket) = rest.find("]\n") {
                        let image_b64 = &rest[..end_bracket];
                        tool_result_images.push(image_b64.to_string());
                        tool_result_text = rest[end_bracket + 2..].to_string();
                    } else {
                        tool_result_text = result.clone();
                    }
                } else if let Some(stub) =
                    crate::engine::tools::files::strip_image_content_marker(&result)
                {
                    // [IMAGE_CONTENT:type]\n<base64> from read_file: the agentic loop already lifted
                    // the bytes into a proper image content block above, so persisting the base64
                    // again would just bloat the events table (and the wire payload — frontend never
                    // reads ToolResult.result for read_file). Strip to a small stub.
                    tool_result_text = stub;
                } else {
                    tool_result_text = result.clone();
                }

                // Front-end confirm-flow sentinels (credentials, plugin install,
                // plugin uninstall, email confirm): emit the transient
                // ThreadEvent that drives the panel/modal, and — for sentinels
                // whose raw JSON would mislead the LLM (install/uninstall let
                // it parse `overwrites` and chat-ask, see git history) —
                // replace tool_result_text so the model only sees a one-line
                // wait notice. EmailConfirm passes through unredacted because
                // its tool description already explains the modal flow.
                let sentinel_event = match_sentinel(&tool_result_text).map(|m| {
                    if let Some(redacted) = m.redacted_text {
                        tool_result_text = redacted;
                    }
                    (m.label, m.event)
                });

                // Persist + broadcast ToolResult
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ToolResult {
                                name: tool_call.name.clone(),
                                result: crate::core::sanitize_for_jsonb(&tool_result_text),
                                images: tool_result_images,
                                success: !is_error,
                                // Always stamp the originating ToolCalled's
                                // event id so `groupIntoExchanges` (frontend)
                                // routes this result to the call's exchange
                                // via `chatToolCallOwners`, not via the
                                // post-`UserQuestionAsked` request_id
                                // redirect. Without explicit pairing, an
                                // `ask_user_question` call's ToolResult
                                // followed the redirect into the question
                                // divider and the original MR exchange's
                                // "Executing ask_user_question..." spinner
                                // never resolved. Chronological name pairing
                                // for in-process resume blocks still works
                                // regardless of this field — see
                                // `core::store::messages::collect_tool_pairs_chronological`.
                                tool_called_event_id,
                            },
                            meta: meta.clone(),
                        },
                        "[AgenticLoop] ToolResult",
                    )
                    .await;

                if let Some((label, event)) = sentinel_event {
                    use crate::engine::event_bus::BusEvent;
                    use crate::engine::thread_events::EventMeta;
                    self.event_bus
                        .emit_or_log(
                            BusEvent::Thread {
                                thread_id,
                                event,
                                meta: EventMeta::NONE,
                            },
                            label,
                        )
                        .await;
                }

                if is_error {
                    had_errors = true;
                    log!(
                        "[AgentLoop] Step {}/{}: Error, will retry: {}",
                        iterations,
                        MAX_ITERATIONS,
                        result
                    );
                } else {
                    log!(
                        "[AgentLoop] Step {}/{}: Success",
                        iterations,
                        MAX_ITERATIONS
                    );
                }

                // Send push notification request SSE event to trigger browser permission
                if result.starts_with("[PUSH_NOTIFICATION_REQUEST]") {
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event:
                                    crate::engine::thread_events::ThreadEvent::PushNotificationRequested,
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[AgenticLoop] PushNotificationRequested",
                        )
                        .await;
                }

                tool_outputs.push((tool_call.id.clone(), tool_result_text));
            }

            // 1. Add the assistant's tool_use response as a message
            let mut assistant_blocks = Vec::new();
            if let Some(text) = &response.content {
                assistant_blocks.push(ContentBlock::Text { text: text.clone() });
            }
            for tc in &response.tool_calls {
                assistant_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.arguments.clone(),
                    thought_signature: tc.thought_signature.clone(),
                });
            }
            messages.push(Message {
                role: "assistant".to_string(),
                content: MessageContent::Blocks(assistant_blocks),
            });

            // 2. Add tool_result blocks as a user message
            let had_edit_errors = had_errors
                && response
                    .tool_calls
                    .iter()
                    .any(|tc| tc.name == tn::EDIT_FILE);
            let instruction = if had_edit_errors {
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::LlmCallRetried {
                                reason: "Retrying with different approach".to_string(),
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[AgenticLoop] LlmCallRetried (edit error)",
                    )
                    .await;
                "One or more edit_file calls failed because old_string was not found — the file content has changed since you last read it. The error message above contains the file's current content. Use THAT content (not your earlier context) to construct the correct old_string for your next edit_file call."
            } else if had_errors {
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::LlmCallRetried {
                                reason: "Retrying with different approach".to_string(),
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[AgenticLoop] LlmCallRetried (tool error)",
                    )
                    .await;
                "Error occurred. Review the error messages above and try a different approach."
            } else {
                "Results above. Do NOT repeat analysis you already gave — the user already read it. Proceed directly to your next action or final answer."
            };

            // Build tool result blocks for the user message.
            // CRITICAL: All ToolResult blocks must come before any Image/Text blocks.
            // The Claude API validates that tool_result blocks immediately follow the
            // assistant's tool_use blocks. Interleaving Image or Text blocks between
            // ToolResult blocks causes the API to miss subsequent ToolResults, producing
            // "tool_use ids were found without tool_result blocks" 400 errors.
            let mut result_blocks: Vec<ContentBlock> = Vec::new();
            let mut trailing_blocks: Vec<ContentBlock> = Vec::new();
            for (tool_use_id, result) in &tool_outputs {
                if let Some((screenshot_b64, dom_text)) = parse_app_capture_marker(result) {
                    result_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: dom_text.to_string(),
                    });
                    trailing_blocks.push(ContentBlock::Image {
                        source_type: "base64".to_string(),
                        media_type: sniff_image_media_type(screenshot_b64).to_string(),
                        data: screenshot_b64.to_string(),
                    });
                    continue;
                }
                if let Some((media_type, image_b64)) =
                    crate::engine::tools::files::parse_image_content_marker(result)
                {
                    result_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: "[Image file displayed to you below]".to_string(),
                    });
                    trailing_blocks.push(ContentBlock::Image {
                        source_type: "base64".to_string(),
                        media_type: media_type.to_string(),
                        data: image_b64.to_string(),
                    });
                    continue;
                }
                result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: result.clone(),
                });
            }
            // Append images and instruction text AFTER all ToolResult blocks
            result_blocks.append(&mut trailing_blocks);
            result_blocks.push(ContentBlock::Text {
                text: instruction.to_string(),
            });

            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Blocks(result_blocks),
            });

            // Check for injected prompts (mid-flight user corrections or system events).
            // Drain all pending injections and add them as user messages before the next LLM call.
            {
                let mut injected_prompts: Vec<super::super::InjectedPrompt> = Vec::new();
                while let Ok(prompt) = injection_rx.try_recv() {
                    injected_prompts.push(prompt);
                }
                for prompt in injected_prompts {
                    match &prompt.kind {
                        super::super::InjectedPromptKind::WakeFromChild => {
                            crate::log!(
                                "[Inject] Wake-from-child (spawning_event {:?}) into active parent {}",
                                prompt.spawning_event_id,
                                thread_id
                            );
                            // Caller (`process_message_with_steps`) already
                            // formatted the block via
                            // `format_child_thread_completed_block`; the same
                            // formatter `build_session_messages` uses on
                            // reload, so the inflight wake matches what a
                            // post-restart resume would project.
                            messages.push(Message {
                                role: "user".to_string(),
                                content: MessageContent::Text(prompt.text.clone()),
                            });
                            continue;
                        }
                        super::super::InjectedPromptKind::UserText => {}
                    }

                    crate::log!(
                        "[Inject] Mid-flight {:?} prompt injected into thread {}: {}",
                        prompt.mode,
                        thread_id,
                        &prompt.text[..prompt.text.floor_char_boundary(80)]
                    );

                    emit_user_prompt_injected_event(&self.event_bus, thread_id, &meta, &prompt)
                        .await;

                    let framed = match prompt.mode {
                        super::super::thread_events::ActorMode::Human => format!(
                            "[USER CORRECTION — the user sent this while you were working. \
                             Prioritize this over your current plan and adjust accordingly.]\n\n{}",
                            prompt.text
                        ),
                        super::super::thread_events::ActorMode::Agent
                        | super::super::thread_events::ActorMode::Engine => format!(
                            "[SYSTEM UPDATE — new information arrived while you were working. \
                             Incorporate this into your current response.]\n\n{}",
                            prompt.text
                        ),
                    };
                    let content = if let Some(imgs) = &prompt.images {
                        if !imgs.is_empty() {
                            let mut blocks = vec![ContentBlock::Text { text: framed }];
                            for img in imgs {
                                blocks.push(ContentBlock::Image {
                                    source_type: "base64".to_string(),
                                    media_type: img.mime_type.clone(),
                                    data: img.base64.clone(),
                                });
                            }
                            MessageContent::Blocks(blocks)
                        } else {
                            MessageContent::Text(framed)
                        }
                    } else {
                        MessageContent::Text(framed)
                    };
                    messages.push(Message {
                        role: "user".to_string(),
                        content,
                    });
                }
            }

            // After the first LLM call, replace base64 image data in the original
            // user message with the Flash-generated text description. The LLM has
            // already seen the actual bytes — subsequent iterations only need the
            // description to remember what was shown.
            if iterations == 1 {
                // Resolve the Flash description (should be done by now — Flash is much
                // faster than the main model's first response). The handle yields
                // `(description, model)` so we can stamp the producing model on
                // the emitted `ImageDescribed` event.
                let described = if let Some(handle) = image_description_handle.take() {
                    match handle.await {
                        Ok(opt) => opt.filter(|(d, _)| !is_bad_image_description(d)),
                        Err(_) => None,
                    }
                } else {
                    None
                };
                let image_description: Option<&str> = described.as_ref().map(|(d, _)| d.as_str());

                if let Some(msg) = messages.get_mut(user_message_idx) {
                    if let MessageContent::Blocks(blocks) = &mut msg.content {
                        let desc = image_description
                            .unwrap_or("user-attached image")
                            .to_string();
                        let stripped = replace_image_blocks(blocks, || format!("[image: {}]", desc));
                        if stripped > 0 {
                            log!(
                                "[AgentLoop] Stripped {}KB of image data from context after first LLM call",
                                stripped / 1024
                            );
                        }
                    }
                }

                // Emit one `ImageDescribed` event per attached hash so the
                // description is preserved as a derived past-tense fact —
                // who (model) saw what (hash) and when (event timestamp).
                // The hashes come from the persisted MessageReceived row so
                // failed-decode images dropped at emit time are NOT
                // re-included here. Idempotent on retries: this branch only
                // runs on iteration 1.
                if let Some((desc, model)) = described {
                    let hashes = match self.event_store.get_event_by_id(origin_id).await {
                        Ok(Some(row)) => row
                            .payload
                            .get("user_image_hashes")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|h| h.as_str().map(str::to_string))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default(),
                        Ok(None) => {
                            log!(
                                "[AgentLoop] ImageDescribed: origin event {} not found, skipping emit",
                                origin_id
                            );
                            Vec::new()
                        }
                        Err(e) => {
                            log!(
                                "[AgentLoop] ImageDescribed: failed to load origin event {}: {}",
                                origin_id, e
                            );
                            Vec::new()
                        }
                    };
                    for hash in hashes {
                        self.event_bus
                            .emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::ImageDescribed {
                                        source_event_id: origin_id,
                                        hash,
                                        description: desc.clone(),
                                        model: model.clone(),
                                    },
                                    // Engine-internal enrichment; inherit the
                                    // turn's channel but drop request_event_id
                                    // (this isn't a response to the user's
                                    // request, it's a derived fact about the
                                    // request itself).
                                    meta: crate::engine::thread_events::EventMeta {
                                        channel: meta.channel,
                                        ..crate::engine::thread_events::EventMeta::NONE
                                    },
                                },
                                "[AgentLoop] ImageDescribed",
                            )
                            .await;
                    }
                }
            }
        }
    }

}
