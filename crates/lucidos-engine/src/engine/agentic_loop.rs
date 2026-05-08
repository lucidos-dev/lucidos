use crate::llm::provider::LlmResponse;
use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use crate::llm::{get_default_tools, get_notification_tool, get_read_notifications_tool};
use crate::llm::{ContentBlock, Message, MessageContent};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::context::{estimate_message_chars, trim_context_if_needed};
use super::types::*;
use super::LucidosEngine;

/// Build the tool list for intent sub-loops.
/// Notification tools must be included explicitly — they're not in get_default_tools().
fn build_intent_tools() -> Vec<ToolDefinition> {
    let mut tools: Vec<_> = get_default_tools()
        .into_iter()
        .filter(|t| t.name != tn::EXECUTE_INTENT)
        .collect();
    tools.push(get_notification_tool());
    tools.push(get_read_notifications_tool());
    tools
}

/// Derive the "target" key for the consecutive-tool-call circuit breaker.
///
/// Most tools have a meaningful target argument (`path`, `url`, `query`).
/// `run_bash` is special: its argument is `command`, and bucketing by the
/// bare tool name would mean three unrelated shell calls (e.g. `git status`
/// → `git add` → `git commit`) trip the guard. Bucketing by the first
/// whitespace-delimited token of `command` keeps the original "stop the LLM
/// from spamming the exact same call" intent (same prefix still counts) while
/// letting unrelated commands run in sequence.
pub(super) fn derive_call_key(tool_name: &str, args: &serde_json::Value) -> String {
    if tool_name == tn::RUN_BASH {
        if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
            if let Some(first_token) = cmd.split_whitespace().next() {
                return first_token.to_string();
            }
        }
        return tn::RUN_BASH.to_string();
    }

    args.get("path")
        .or_else(|| args.get("url"))
        .or_else(|| args.get("query"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Check if buffered text has reached a renderable boundary.
pub(super) fn should_flush(text: &str) -> bool {
    // Paragraph break
    if text.ends_with("\n\n") {
        return true;
    }
    // Code fence close
    if text.ends_with("```\n") {
        return true;
    }
    // Heading completed (ends with newline after a heading line)
    if text.ends_with('\n') {
        if let Some(last_line) = text.lines().last() {
            if last_line.starts_with('#') {
                return true;
            }
        }
    }
    // Horizontal rule
    if text.ends_with("\n---\n") || text.ends_with("\n***\n") {
        return true;
    }
    false
}

/// Detect image descriptions that indicate the model couldn't see the image.
/// These are error responses, not actual descriptions — using them would poison
/// the LLM context with false information (e.g. "the image shows an error message").
fn is_bad_image_description(desc: &str) -> bool {
    let lower = desc.to_lowercase();
    lower.contains("i do not see any image")
        || lower.contains("i don't see any image")
        || lower.contains("no image attached")
        || lower.contains("no images attached")
        || lower.contains("please provide the image")
        || lower.contains("i cannot see")
        || lower.contains("no image was provided")
        || lower.contains("no image provided")
}

/// Hard cap on per-turn LLM tool-call iterations. The engine returns a
/// `[ENGINE-LIMIT]`-tagged response when the cap is hit — the chat agent
/// cannot otherwise observe its own tool-call count, so any "tool-call
/// cap" claim without this prefix is a hallucination. The chat system
/// prompt tells the model the same.
pub(super) const MAX_ITERATIONS: usize = 100;

/// Without this emit the frontend would never see a terminator and the
/// thread would show "running" forever after hitting the cap.
pub(super) async fn emit_iteration_cap_response_generated(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    meta: &crate::engine::thread_events::EventMeta,
    images: Vec<String>,
    effective_model: Option<String>,
    effective_effort: Option<String>,
) -> String {
    let msg = format!(
        "[ENGINE-LIMIT] Per-turn limit of {} tool calls reached. Send any message to continue from here.",
        MAX_ITERATIONS
    );
    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                text: msg.clone(),
                images,
                model: effective_model,
                reasoning_effort: effective_effort,
            },
            meta: meta.clone(),
        },
        "[AgenticLoop] ResponseGenerated (iteration cap)",
    )
    .await;
    msg
}

/// Critical invariant: the persisted event MUST get a fresh primary key,
/// NOT `prompt.event_id`. The chat fast-path (`chat::process`) emits
/// `MessageReceived` with the client-provided UUID before injecting,
/// so reusing that UUID here causes an `events_pkey` duplicate-key
/// error and the event is silently dropped under `emit_or_log`. The
/// frontend correlates pending messages via `MessageReceived.id`;
/// `UserPromptInjected` is a separate engine-side acknowledgment whose
/// link back to the request is carried by `meta.request_event_id`.
pub(super) async fn emit_user_prompt_injected_event(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    base_meta: &crate::engine::thread_events::EventMeta,
    prompt: &super::InjectedPrompt,
) {
    let mut inject_meta = base_meta.clone();
    inject_meta.event_id = None;
    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::engine::thread_events::ThreadEvent::UserPromptInjected {
                text: prompt.text.clone(),
                mode: prompt.mode,
                origin: prompt.origin.clone(),
                injected_message_id: prompt.event_id,
            },
            meta: inject_meta,
        },
        "[AgenticLoop] UserPromptInjected",
    )
    .await;
}

/// Defensive post-loop guard: emits `ResponseAborted` if no terminator
/// landed for `request_event_id`. Catches future regressions in the
/// loop's many return paths — every existing path emits explicitly, but
/// the SQL check is the safety net. Scoped to `request_event_id` so a
/// previous exchange's terminator doesn't mask a current zombie one.
///
/// Skip the SQL when callers can prove a terminator was emitted (success
/// path) — `chat::process` does this via the `terminator_emitted` flag
/// threaded through `run_agentic_loop`. Without that fast path this
/// query runs on every chat turn against a `payload->>'request_event_id'`
/// expression that has no functional index, walking every event in the
/// thread on long-lived conversations.
pub(super) async fn ensure_terminator_emitted(
    bus: &crate::engine::event_bus::EventBus,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    request_event_id: Uuid,
    channel: Option<crate::engine::thread_events::EventChannel>,
) {
    let has_terminator: bool = match sqlx::query_scalar(
        "SELECT EXISTS(\
            SELECT 1 FROM events \
            WHERE aggregate_id = $1 \
              AND event_type = ANY($3::text[]) \
              AND payload->>'request_event_id' = $2\
        )",
    )
    .bind(thread_id.to_string())
    .bind(request_event_id.to_string())
    .bind(crate::engine::thread_events::ThreadEvent::TERMINATOR_EVENT_TYPES)
    .fetch_one(pool)
    .await
    {
        Ok(b) => b,
        Err(e) => {
            // Fail open: assume no terminator and let the defensive emit
            // run. A phantom abort is much better than silently leaving
            // the UI stuck on "running" because the DB hiccupped.
            crate::log!(
                "[AgenticLoop] terminator-existence query failed for request {} on thread {}: {}",
                request_event_id,
                thread_id,
                e
            );
            false
        }
    };

    if has_terminator {
        return;
    }

    crate::log!(
        "[AgenticLoop] WARNING: loop exited without emitting a terminator for request {} on thread {} — emitting defensive ResponseAborted",
        request_event_id,
        thread_id
    );
    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::engine::thread_events::ThreadEvent::ResponseAborted {
                text: String::new(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: crate::engine::thread_events::EventMeta {
                request_event_id: Some(request_event_id),
                channel,
                ..crate::engine::thread_events::EventMeta::NONE
            },
        },
        "[AgenticLoop] ResponseAborted (defensive — no terminator emitted)",
    )
    .await;
}

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
        mut image_description_handle: Option<tokio::task::JoinHandle<Option<String>>>,
        origin_id: Uuid,
        proposed_change: &mut bool,
        user_images: Option<&[crate::api::ChatImage]>,
        device_id: Option<&str>,
        model_override: Option<&str>,
        reasoning_effort: Option<&str>,
        cancel_token: &CancellationToken,
        injection_rx: &mut mpsc::UnboundedReceiver<super::InjectedPrompt>,
        // Set to true at every terminator emission so the post-loop guard
        // in `chat::process` can skip its `payload->>'request_event_id'`
        // existence check on the success path. The check has no functional
        // index and would walk every event in long-lived threads otherwise.
        terminator_emitted: &mut bool,
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

        let mut iterations = 0;
        let mut images: Vec<String> = Vec::new(); // Track screenshots created during this request
        let mut last_tool_call: Option<(String, String)> = None; // (tool_name, key) - key is path for read_file
        let mut consecutive_same_call = 0;
        let mut cached_list_files: Option<String> = None;
        let mut read_cache: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut modified_app_uis: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        // Agent loop
        loop {
            iterations += 1;
            if cancel_token.is_cancelled() {
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ResponseCanceled {
                                text: String::new(),
                                images: images.clone(),
                                model: effective_model.clone(),
                                reasoning_effort: effective_effort.clone(),
                            },
                            meta: meta.clone(),
                        },
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

            let trimmed = trim_context_if_needed(messages, message_budget) > 0;
            // Safety net: validate tool_use/tool_result pairing after trimming.
            // The primary fix ensures correct block ordering, but this catches any
            // edge case where pairing breaks (trimming bugs, injection ordering, etc.)
            super::context::validate_tool_use_pairing(messages);
            // Always measure AFTER trimming — pass 0 strips images even when pass 2
            // removes no messages, so pre-trim chars can be wildly inflated.
            let context_chars: usize = messages.iter().map(estimate_message_chars).sum();

            let context_tokens = context_chars / 4; // rough estimate
            let context_messages = messages.len();
            let trimmed_str = if trimmed { " (trimmed)" } else { "" };
            self.event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::Thinking {
                            text: format!(
                                "Context: {} tokens, {} messages{}",
                                context_tokens, context_messages, trimmed_str
                            ),
                            context_tokens: Some(context_tokens),
                            context_messages: Some(context_messages),
                            trimmed: if trimmed { Some(true) } else { None },
                        },
                        meta: meta.clone(),
                    },
                    "[AgenticLoop] Thinking (context summary)",
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
                    if should_flush(&text) {
                        let _ = sender.send(crate::engine::event_bus::EmittedEvent {
                            event_id: uuid::Uuid::new_v4(),
                            seq: None,
                            created: chrono::Utc::now(),
                            typed: crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::TextStreaming {
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
                                    event: crate::engine::thread_events::ThreadEvent::Retrying {
                                        reason: format!("Request failed: {}", e),
                                    },
                                    meta: crate::engine::thread_events::EventMeta::NONE,
                                },
                                "[AgenticLoop] Retrying",
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
                    self.event_bus.emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ResponseCanceled {
                                text: partial.clone(), images: images.clone(), model: effective_model.clone(), reasoning_effort: effective_effort.clone(),
                            },
                            meta: meta.clone(),
                        },
                        "[AgenticLoop] ResponseCanceled",
                    ).await;
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

            // None for providers that don't report `usage` (OpenAI, Gemini) —
            // the chars/4 estimate stands.
            if let Some(input_tokens) = response.input_tokens {
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ContextTokensMeasured {
                                input_tokens,
                            },
                            meta: meta.clone(),
                        },
                        "[AgenticLoop] ContextTokensMeasured",
                    )
                    .await;
            }

            // Final flush — send any remaining buffered text and persist remainder
            let (flush_text, remaining_to_persist) = {
                let text = raw_buffer.lock().unwrap();
                let cloned = if text.is_empty() {
                    None
                } else {
                    Some(text.clone())
                };
                let last = *last_persisted_len.lock().unwrap();
                let remaining = if text.len() > last {
                    Some(text[last..].to_string())
                } else {
                    None
                };
                (cloned, remaining)
            };
            if let Some(flush) = flush_text {
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::TextStreaming {
                                text: flush,
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[AgenticLoop] TextStreaming final flush",
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
                                event: crate::engine::thread_events::ThreadEvent::RefreshAppUI {
                                    app_id: app_id.clone(),
                                },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[AgenticLoop] RefreshAppUI (post-loop)",
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
                // stop_reason distinguishes legitimate end_turn from
                // truncation; thinking_chars distinguishes "thought hard
                // then gave up" from "said nothing without thinking".
                let stop_reason = response.stop_reason.as_deref().unwrap_or("unknown");
                let output_tokens = response
                    .output_tokens
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let thinking_chars = response
                    .thinking_chars
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let hint = match stop_reason {
                    "end_turn" => " — model decided no action was needed",
                    "max_tokens" => " — output truncated by token budget",
                    _ => "",
                };
                let error = format!(
                    "Model returned no response (stop_reason: {}, output_tokens: {}, thinking_chars: {}, model: {}){}.",
                    stop_reason,
                    output_tokens,
                    thinking_chars,
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

                // If read_file called 3+ times on SAME file, block it
                if current_tool == tn::READ_FILE && consecutive_same_call >= 3 {
                    log!(
                        "[AgentLoop] Blocking repeated read_file of '{}' (call #{})",
                        call_key,
                        consecutive_same_call
                    );
                    // After 5 blocked attempts, force-break out of the loop
                    if consecutive_same_call >= 5 {
                        log!(
                            "[AgentLoop] Force-breaking tool loop after {} repeated read_file attempts",
                            consecutive_same_call
                        );
                        let msg = format!("I read the file `{}` but wasn't able to complete the task with it. Could you give me more specific instructions?", call_key);
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
                    let stop_msg = format!("STOP: You've read '{}' multiple times. The content hasn't changed. Use the information you have and proceed with your task.", call_key);
                    push_circuit_breaker(messages, &response, &stop_msg);
                    continue;
                }

                // Generic circuit breaker: any tool called 3+ times in a row
                // Excluded: write tools (multiple edits normal), browser tools (multi-step
                // browsing workflows are normal), run_python (sequential scripts processing
                // different data are normal). All still bounded by MAX_ITERATIONS.
                let excluded = matches!(
                    current_tool.as_str(),
                    tn::EDIT_FILE
                        | tn::WRITE_FILE
                        | tn::RUN_PYTHON
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
                let tool_called_event_id = self
                    .event_bus
                    .emit_for_id(crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ToolCalled {
                            name: tool_call.name.clone(),
                            description: self.describe_tool(&tool_call.name, &tool_call.arguments),
                            args: tool_call.arguments.clone(),
                        },
                        meta: meta.clone(),
                    })
                    .await;

                // Check read cache first for read_file
                let result = if tool_call.name == tn::READ_FILE {
                    let path = tool_call
                        .arguments
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if let Some(cached) = read_cache.get(path) {
                        log!(
                            "[AgentLoop] Step {}/{}: (cached)",
                            iterations,
                            MAX_ITERATIONS
                        );
                        cached.clone()
                    } else {
                        let r = self
                            .execute_tool(
                                &tool_call.name,
                                &tool_call.arguments,
                                extraction_ctx,
                                request_id,
                                device_id,
                                cancel_token,
                                thread_id,
                            )
                            .await;
                        if !r.starts_with("Error:") {
                            read_cache.insert(path.to_string(), r.clone());
                        }
                        r
                    }
                } else if let Some(r) = self
                    .handle_special_tool(
                        &tool_call.name,
                        &tool_call.arguments,
                        thread_id,
                        user_images,
                        device_id,
                        tool_called_event_id,
                    )
                    .await
                {
                    r
                } else {
                    self.execute_tool(
                        &tool_call.name,
                        &tool_call.arguments,
                        extraction_ctx,
                        request_id,
                        device_id,
                        cancel_token,
                        thread_id,
                    )
                    .await
                };
                let is_error = result.starts_with("Error:");

                // Invalidate read cache on write/edit
                if (tool_call.name == tn::WRITE_FILE || tool_call.name == tn::EDIT_FILE)
                    && !is_error
                {
                    let path = tool_call
                        .arguments
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    read_cache.remove(path);

                    // Track app UI modifications for deferred refresh before final response
                    // Path format: apps/{app_id}/{file}
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
                let tool_result_text;
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

                // Persist + broadcast ToolResult
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ToolResult {
                                name: tool_call.name.clone(),
                                result: crate::core::sanitize_for_jsonb(&tool_result_text),
                                images: tool_result_images,
                            },
                            meta: meta.clone(),
                        },
                        "[AgenticLoop] ToolResult",
                    )
                    .await;

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

                // Send credential request as a dedicated SSE event so frontend can show inline form
                if result.starts_with(crate::engine::tools::credentials::CREDENTIAL_REQUEST_PREFIX)
                {
                    if let Some(json_start) = result.find('{') {
                        let payload = result[json_start..].to_string();
                        self.event_bus
                            .emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event:
                                        crate::engine::thread_events::ThreadEvent::CredentialRequest {
                                            payload,
                                        },
                                    meta: crate::engine::thread_events::EventMeta::NONE,
                                },
                                "[AgenticLoop] CredentialRequest",
                            )
                            .await;
                    }
                }

                // Send email confirm request as SSE event so frontend can show confirmation modal
                if result.starts_with("[EMAIL_CONFIRM]") {
                    if let Some(json_start) = result.find('{') {
                        let payload = result[json_start..].to_string();
                        self.event_bus
                            .emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event:
                                        crate::engine::thread_events::ThreadEvent::EmailConfirmRequest {
                                            payload,
                                        },
                                    meta: crate::engine::thread_events::EventMeta::NONE,
                                },
                                "[AgenticLoop] EmailConfirmRequest",
                            )
                            .await;
                    }
                }

                // Send push notification request SSE event to trigger browser permission
                if result.starts_with("[PUSH_NOTIFICATION_REQUEST]") {
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event:
                                    crate::engine::thread_events::ThreadEvent::PushNotificationRequest,
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[AgenticLoop] PushNotificationRequest",
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
                            event: crate::engine::thread_events::ThreadEvent::Retrying {
                                reason: "Retrying with different approach".to_string(),
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[AgenticLoop] Retrying (edit error)",
                    )
                    .await;
                "One or more edit_file calls failed because old_string was not found — the file content has changed since you last read it. The error message above contains the file's current content. Use THAT content (not your earlier context) to construct the correct old_string for your next edit_file call."
            } else if had_errors {
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::Retrying {
                                reason: "Retrying with different approach".to_string(),
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[AgenticLoop] Retrying (tool error)",
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
                if let Some(rest) = result.strip_prefix("[APP_CAPTURE:") {
                    if let Some(end_bracket) = rest.find("]\n") {
                        let screenshot_b64 = &rest[..end_bracket];
                        let dom_text = &rest[end_bracket + 2..];
                        result_blocks.push(ContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: dom_text.to_string(),
                        });
                        trailing_blocks.push(ContentBlock::Image {
                            source_type: "base64".to_string(),
                            media_type: "image/png".to_string(),
                            data: screenshot_b64.to_string(),
                        });
                        continue;
                    }
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
                let mut injected_prompts: Vec<super::InjectedPrompt> = Vec::new();
                while let Ok(prompt) = injection_rx.try_recv() {
                    injected_prompts.push(prompt);
                }
                for prompt in injected_prompts {
                    crate::log!(
                        "[Inject] Mid-flight {:?} prompt injected into thread {}: {}",
                        prompt.mode,
                        thread_id,
                        &prompt.text[..prompt.text.floor_char_boundary(80)]
                    );

                    emit_user_prompt_injected_event(&self.event_bus, thread_id, &meta, &prompt)
                        .await;

                    let framed = match prompt.mode {
                        super::thread_events::ActorMode::Human => format!(
                            "[USER CORRECTION — the user sent this while you were working. \
                             Prioritize this over your current plan and adjust accordingly.]\n\n{}",
                            prompt.text
                        ),
                        super::thread_events::ActorMode::Agent
                        | super::thread_events::ActorMode::Engine => format!(
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

            // After the first LLM call, strip base64 image data from message[0].
            // The LLM has already seen the images — subsequent iterations only need
            // the text description to remember what was shown.
            if iterations == 1 {
                // Resolve the Flash description (should be done by now — Flash is much
                // faster than the main model's first response)
                let image_description = if let Some(handle) = image_description_handle.take() {
                    match handle.await {
                        Ok(desc) => desc.filter(|d| !is_bad_image_description(d)),
                        Err(_) => None,
                    }
                } else {
                    None
                };

                if let MessageContent::Blocks(blocks) = &mut messages[0].content {
                    let mut stripped = 0usize;
                    for block in blocks.iter_mut() {
                        if let ContentBlock::Image { data, .. } = block {
                            stripped += data.len();
                            let desc = image_description
                                .as_deref()
                                .unwrap_or("user-attached image");
                            *block = ContentBlock::Text {
                                text: format!("[image: {}]", desc),
                            };
                        }
                    }
                    if stripped > 0 {
                        log!(
                            "[AgentLoop] Stripped {}KB of image data from context after first LLM call",
                            stripped / 1024
                        );
                    }
                }

                // Update the event in DB with the resolved description
                if let Some(ref desc) = image_description {
                    if let Err(e) = self
                        .event_store
                        .update_image_description(origin_id, desc)
                        .await
                    {
                        log!(
                            "[AgentLoop] Failed to update image description in event: {}",
                            e
                        );
                    }
                }
            }
        }
    }

    /// Handle tool calls that need special processing outside of `execute_tool`.
    /// Returns `Some(result)` if the tool was handled, `None` if it should fall through
    /// to `execute_tool()`.
    async fn handle_special_tool(
        &self,
        tool_name: &str,
        tool_args: &serde_json::Value,
        thread_id: Uuid,
        user_images: Option<&[crate::api::ChatImage]>,
        device_id: Option<&str>,
        // Event_id of the just-emitted ToolCalled event. Forwarded as
        // `spawning_event_id` for spawn-style tools (run_thread, run_claude)
        // so the new thread can be traced back to the exact tool call that
        // started it.
        tool_called_event_id: Option<Uuid>,
    ) -> Option<String> {
        if tool_name == tn::REFRESH_FILE {
            let path = tool_args["path"].as_str().unwrap_or("");
            self.event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::RefreshFile {
                            path: path.to_string(),
                        },
                        meta: crate::engine::thread_events::EventMeta::NONE,
                    },
                    "[AgenticLoop] RefreshFile",
                )
                .await;
            Some(format!("File preview refreshed for {}", path))
        } else if tool_name == tn::CAPTURE_APP || tool_name == tn::REFRESH_APP {
            let app_id = tool_args
                .get("app_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if tool_name == tn::REFRESH_APP {
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::RefreshAppUI {
                                app_id: app_id.clone(),
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[AgenticLoop] RefreshAppUI (refresh_app tool)",
                    )
                    .await;
            }

            let skip_capture = tool_args
                .get("skip_capture")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if skip_capture {
                Some("App UI refreshed.".to_string())
            } else {
                if tool_name == tn::REFRESH_APP {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }

                let request_id = Uuid::new_v4().to_string();
                let (tx, rx) = tokio::sync::oneshot::channel();
                {
                    let mut captures = self.pending_captures.lock().unwrap();
                    captures.insert(request_id.clone(), tx);
                }

                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::CaptureAppUI {
                                app_id: app_id.clone(),
                                request_id: request_id.clone(),
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[AgenticLoop] CaptureAppUI",
                    )
                    .await;

                Some(
                    match tokio::time::timeout(std::time::Duration::from_secs(10), rx).await {
                        Ok(Ok(capture)) => {
                            format!(
                                "[APP_CAPTURE:{}]\nDOM snapshot:\n{}",
                                capture.screenshot, capture.dom
                            )
                        }
                        Ok(Err(_)) => {
                            "Error: Capture failed — the frontend channel was dropped.".to_string()
                        }
                        Err(_) => {
                            let mut captures = self.pending_captures.lock().unwrap();
                            captures.remove(&request_id);
                            "Error: Capture timed out (10s) — the frontend could not open or capture the app UI.".to_string()
                        }
                    },
                )
            }
        } else if tool_name == tn::RUN_CLAUDE {
            let prompt = tool_args["prompt"].as_str().unwrap_or("");
            if prompt.is_empty() {
                Some("Error: prompt is required".to_string())
            } else {
                let repo_id = match tool_args["repo"].as_str().filter(|s| !s.is_empty()) {
                    Some(repo_param) => match crate::core::repositories::RepositoryStore::resolve(
                        &self.pool, repo_param,
                    )
                    .await
                    {
                        Ok(Some(repo)) => Some(repo.id.to_string()),
                        Ok(None) => {
                            return Some(format!("Error: Repository '{}' not found. Use manage_repositories with action 'list' to see registered repositories.", repo_param));
                        }
                        Err(e) => {
                            return Some(format!("Error: Failed to look up repository: {}", e));
                        }
                    },
                    None => None,
                };

                // Absent `images` → forward the current message's images (default).
                // Present (even empty) → caller has explicitly chosen the selection.
                let resolved_images: Option<Vec<crate::api::ChatImage>> = match tool_args
                    .get("images")
                {
                    Some(serde_json::Value::Array(arr)) => {
                        let refs: Vec<String> = arr
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                        if refs.len() != arr.len() {
                            return Some("Error: `images` entries must be strings".to_string());
                        }
                        let events = match self
                            .event_store
                            .get_thread_events(&thread_id.to_string())
                            .await
                        {
                            Ok(evts) => evts,
                            Err(e) => {
                                return Some(format!("Error: Failed to load thread events: {}", e))
                            }
                        };
                        match crate::engine::tools::image::resolve_thread_image_refs(&self.workspace_path, &events, &refs)
                        {
                            Ok(imgs) => Some(imgs),
                            Err(e) => return Some(format!("Error: {}", e)),
                        }
                    }
                    _ => None,
                };

                let caller_title = tool_args["title"].as_str();
                let images = resolved_images
                    .as_deref()
                    .or(user_images)
                    .map(<[crate::api::ChatImage]>::to_vec);
                Some(
                    match self
                        .spawn_agent_thread(crate::engine::claude_code::SpawnAgentThreadParams {
                            prompt: prompt.to_string(),
                            user_images: images,
                            device_id: device_id.map(str::to_string),
                            parent_thread_id: Some(thread_id),
                            spawning_event_id: tool_called_event_id,
                            repo_id,
                            caller_title: caller_title.map(str::to_string),
                        })
                        .await
                    {
                        Ok(cc_thread_id) => {
                            format!(
                                "Claude Code session started in a new thread (thread_id: {}). \
                             Tell the user you've started a new thread to work on this and include \
                             a link: [Open thread](thread:{})",
                                cc_thread_id, cc_thread_id
                            )
                        }
                        Err(e) => format!("Error: Failed to start Claude Code: {}", e),
                    },
                )
            }
        } else if tool_name == tn::RUN_THREAD {
            let prompt = tool_args["prompt"].as_str().unwrap_or("");
            if prompt.is_empty() {
                Some("Error: prompt is required".to_string())
            } else {
                Some(
                    match LucidosEngine::check_thread_recursion_guard(&self.pool, thread_id).await {
                        Err(guard_err) => format!("Error: {}", guard_err),
                        Ok(_) => {
                            let child_thread_id = uuid::Uuid::new_v4();
                            // Without this the spawned loop defaults to LUCIDOS_MODEL
                            // instead of the user's chat-mode preference.
                            let (chat_model, chat_effort) =
                                crate::core::PreferenceStore::user_chat_settings(&self.pool).await;
                            // Emit MessageReceived eagerly BEFORE spawning the background task.
                            // This ensures the parent's active_children_count is incremented
                            // immediately, preventing a race where the parent completes
                            // (ResponseGenerated → "review") before children register.
                            // origin=None lets `make_message_received` synthesize a
                            // structured ParentThread origin from `mode + parent_thread_id`,
                            // which downstream consumers need to attribute the spawn to
                            // the parent agent run.
                            let eager_result = self
                                .event_bus
                                .emit(crate::engine::event_bus::BusEvent::Thread {
                                    thread_id: child_thread_id,
                                    event: crate::engine::chat::make_message_received(
                                        &self.workspace_path,
                                        prompt,
                                        None,
                                        None,
                                        None,
                                        Some(thread_id),
                                        tool_called_event_id,
                                        crate::engine::thread_events::ActorMode::Agent,
                                        chat_model.as_deref(),
                                        chat_effort.as_deref(),
                                        None,
                                    ),
                                    meta: crate::engine::thread_events::EventMeta {
                                        channel: Some(
                                            crate::engine::thread_events::EventChannel::Chat,
                                        ),
                                        ..crate::engine::thread_events::EventMeta::NONE
                                    },
                                })
                                .await;
                            match eager_result {
                                Ok(Some(emit)) => {
                                    let origin_id = emit.event_id;
                                    let caller_title = tool_args["title"].as_str();
                                    match self.spawn_thread(
                                        prompt,
                                        Some(thread_id),
                                        tool_called_event_id,
                                        child_thread_id,
                                        Some(origin_id),
                                        caller_title,
                                        chat_model,
                                        chat_effort,
                                    ) {
                                    Ok(cid) => format!(
                                        "Thread started (thread_id: {}). When the child thread finishes, \
                                         this conversation will automatically resume with its results — \
                                         you don't need to check on it. Continue with other work or tell \
                                         the user you've delegated this subtask: [Open thread](thread:{})",
                                        cid, cid
                                    ),
                                    Err(e) => format!("Error starting thread: {}", e),
                                }
                                }
                                Ok(None) => {
                                    "Error: MessageReceived emit returned no result".to_string()
                                }
                                Err(e) => format!("Error starting thread: {}", e),
                            }
                        }
                    },
                )
            }
        } else if let Some((server_id, mcp_tool_name)) =
            crate::mcp::McpManager::parse_mcp_tool_name(tool_name)
        {
            let (auto_approve, server_name) = {
                let statuses = self.mcp_manager.list_servers().await.unwrap_or_default();
                let server = statuses.iter().find(|s| s.id == server_id);
                (
                    server.map(|s| s.auto_approve).unwrap_or(false),
                    server
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| server_id.clone()),
                )
            };

            let approved = if auto_approve {
                true
            } else {
                let consent_request_id = uuid::Uuid::new_v4().to_string();
                let (tx, rx) = tokio::sync::oneshot::channel();
                {
                    let mut pending = self.pending_mcp_consent.lock().unwrap();
                    pending.insert(consent_request_id.clone(), tx);
                }

                let args_summary = serde_json::to_string_pretty(tool_args)
                    .unwrap_or_else(|_| tool_args.to_string());
                let args_summary = if args_summary.len() > 500 {
                    format!(
                        "{}...",
                        &args_summary[..args_summary.floor_char_boundary(500)]
                    )
                } else {
                    args_summary
                };

                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::McpConsentRequest {
                                data: serde_json::json!({
                                    "request_id": consent_request_id,
                                    "server_name": server_name,
                                    "tool_name": mcp_tool_name,
                                    "arguments_summary": args_summary,
                                })
                                .to_string(),
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[AgenticLoop] McpConsentRequest",
                    )
                    .await;

                match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
                    Ok(Ok(allowed)) => allowed,
                    Ok(Err(_)) => false,
                    Err(_) => {
                        let mut pending = self.pending_mcp_consent.lock().unwrap();
                        pending.remove(&consent_request_id);
                        false
                    }
                }
            };

            Some(if approved {
                match self
                    .mcp_manager
                    .call_tool(&server_id, &mcp_tool_name, tool_args.clone())
                    .await
                {
                    Ok((result, _, _)) => result,
                    Err(e) => format!("Error: MCP tool call failed: {}", e),
                }
            } else {
                format!(
                    "Error: User denied MCP tool call '{}' on '{}'",
                    mcp_tool_name, server_name
                )
            })
        } else {
            None
        }
    }

    /// Run an isolated agentic loop for intent execution.
    /// This is a simplified version of run_agentic_loop with its own system prompt,
    /// no conversation history, and all tools except execute_intent (no recursion).
    /// Returns only the final response text.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_intent_loop(
        &self,
        system_prompt: &str,
        task: &str,
        request_id: Uuid,
        extraction_ctx: &str,
        device_id: Option<&str>,
        cancel_token: &CancellationToken,
        thread_id: Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let tools = build_intent_tools();

        let mut messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text(format!("Task: {}", task)),
        }];

        const MAX_INTENT_ITERATIONS: usize = 100;
        let mut iterations = 0;
        let mut last_tool_call: Option<(String, String)> = None;
        let mut consecutive_same_call = 0;

        loop {
            iterations += 1;

            if cancel_token.is_cancelled() {
                return Ok("Intent execution canceled.".to_string());
            }

            if iterations > MAX_INTENT_ITERATIONS {
                return Ok("Intent execution reached iteration limit.".to_string());
            }

            // Trim context if needed
            let message_budget = 400_000; // ~100k tokens budget for intent sub-loops
            trim_context_if_needed(&mut messages, message_budget);

            // Call LLM with no streaming (sub-loop doesn't stream text to frontend)
            let response = self
                .llm
                .chat(
                    messages.clone(),
                    tools.clone(),
                    None, // no model override — use default
                    Some(system_prompt),
                    None, // no token streaming callback
                    None, // no reasoning effort override
                )
                .await?;

            // No tool calls → final answer
            if response.tool_calls.is_empty() {
                return Ok(response.content.unwrap_or_else(|| "Done.".to_string()));
            }

            // Build assistant message with tool_use blocks
            let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
            if let Some(ref text) = response.content {
                assistant_blocks.push(ContentBlock::Text { text: text.clone() });
            }
            for tc in &response.tool_calls {
                assistant_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.arguments.clone(),
                });
            }
            messages.push(Message {
                role: "assistant".to_string(),
                content: MessageContent::Blocks(assistant_blocks),
            });

            // Circuit breaker for consecutive duplicate calls
            if response.tool_calls.len() == 1 {
                let tc = &response.tool_calls[0];
                let call_key = derive_call_key(&tc.name, &tc.arguments);
                let current_call = (tc.name.clone(), call_key);
                if Some(&current_call) == last_tool_call.as_ref() {
                    consecutive_same_call += 1;
                } else {
                    consecutive_same_call = 1;
                    last_tool_call = Some(current_call);
                }

                if consecutive_same_call >= 4 {
                    // Force break — add tool results and stop
                    let result_blocks: Vec<ContentBlock> = response
                        .tool_calls
                        .iter()
                        .map(|tc| ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: "STOP: Repeated tool call. Give your final answer now."
                                .to_string(),
                        })
                        .collect();
                    messages.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Blocks(result_blocks),
                    });
                    continue;
                }
            } else {
                consecutive_same_call = 0;
                last_tool_call = None;
            }

            // Execute each tool call
            let mut result_blocks: Vec<ContentBlock> = Vec::new();
            for tc in &response.tool_calls {
                // Emit ToolCalled via bus. Capture the event_id so spawn-style tools
                // can record it as the spawning_event_id of the new thread.
                let tool_called_event_id = self
                    .event_bus
                    .emit_for_id(crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ToolCalled {
                            name: tc.name.clone(),
                            description: self.describe_tool(&tc.name, &tc.arguments),
                            args: tc.arguments.clone(),
                        },
                        meta: crate::engine::thread_events::EventMeta::NONE,
                    })
                    .await;

                let result = if let Some(r) = self
                    .handle_special_tool(
                        &tc.name,
                        &tc.arguments,
                        thread_id,
                        None,
                        device_id,
                        tool_called_event_id,
                    )
                    .await
                {
                    r
                } else {
                    self.execute_tool(
                        &tc.name,
                        &tc.arguments,
                        extraction_ctx,
                        request_id,
                        device_id,
                        cancel_token,
                        thread_id,
                    )
                    .await
                };

                // Emit ToolResult via bus
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ToolResult {
                                name: tc.name.clone(),
                                result: crate::core::sanitize_for_jsonb(&result),
                                images: vec![],
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[IntentLoop] ToolResult",
                    )
                    .await;

                result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: tc.id.clone(),
                    content: result,
                });
            }

            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Blocks(result_blocks),
            });
        }
    }
}

#[cfg(test)]
mod should_flush_tests {
    use super::{is_bad_image_description, should_flush};

    // --- Paragraph breaks ---

    #[test]
    fn flushes_on_double_newline() {
        assert!(should_flush("Hello world\n\n"));
    }

    #[test]
    fn no_flush_on_single_newline() {
        assert!(!should_flush("Hello world\n"));
    }

    #[test]
    fn no_flush_mid_paragraph() {
        assert!(!should_flush("Hello world"));
    }

    #[test]
    fn flushes_on_multiple_paragraphs() {
        assert!(should_flush("First paragraph.\n\nSecond paragraph.\n\n"));
    }

    // --- Code fence close ---

    #[test]
    fn flushes_on_code_fence_close() {
        assert!(should_flush("```rust\nfn main() {}\n```\n"));
    }

    #[test]
    fn no_flush_on_code_fence_open() {
        assert!(!should_flush("```rust\n"));
    }

    #[test]
    fn no_flush_on_code_fence_without_trailing_newline() {
        assert!(!should_flush("```rust\ncode\n```"));
    }

    // --- Heading after newline ---

    #[test]
    fn flushes_on_heading_followed_by_newline() {
        assert!(should_flush("Some text\n## Heading\n"));
    }

    #[test]
    fn flushes_on_h1_followed_by_newline() {
        assert!(should_flush("Intro\n# Title\n"));
    }

    #[test]
    fn no_flush_on_heading_without_trailing_newline() {
        assert!(!should_flush("Some text\n## Heading"));
    }

    #[test]
    fn flushes_on_first_line_heading() {
        // A complete heading line should always flush
        assert!(should_flush("# Title\n"));
    }

    // --- Horizontal rules ---

    #[test]
    fn flushes_on_dash_horizontal_rule() {
        assert!(should_flush("Some text\n---\n"));
    }

    #[test]
    fn flushes_on_asterisk_horizontal_rule() {
        assert!(should_flush("Some text\n***\n"));
    }

    #[test]
    fn no_flush_on_partial_horizontal_rule() {
        assert!(!should_flush("Some text\n---"));
    }

    // --- Edge cases ---

    #[test]
    fn no_flush_on_empty_string() {
        assert!(!should_flush(""));
    }

    #[test]
    fn no_flush_on_whitespace_only() {
        assert!(!should_flush("   "));
    }

    #[test]
    fn no_flush_on_single_char() {
        assert!(!should_flush("a"));
    }

    #[test]
    fn flushes_long_text_ending_with_paragraph_break() {
        let mut text = "A".repeat(5000);
        text.push_str("\n\n");
        assert!(should_flush(&text));
    }

    #[test]
    fn no_flush_on_long_text_without_boundary() {
        let text = "A".repeat(5000);
        assert!(!should_flush(&text));
    }

    // --- Combinations ---

    #[test]
    fn flushes_code_block_then_paragraph() {
        assert!(should_flush("```\ncode\n```\n\nNext paragraph\n\n"));
    }

    #[test]
    fn flushes_heading_in_middle_of_text() {
        assert!(should_flush("First part\n## Section\n"));
    }

    // --- List items (should NOT flush) ---

    #[test]
    fn no_flush_on_list_item() {
        assert!(!should_flush("- item 1\n"));
    }

    #[test]
    fn no_flush_on_numbered_list() {
        assert!(!should_flush("1. item\n"));
    }

    // --- is_bad_image_description ---

    #[test]
    fn rejects_gemini_no_image_response() {
        assert!(is_bad_image_description(
            "Please provide the images you would like me to describe. I do not see any images attached to your message."
        ));
    }

    #[test]
    fn rejects_contraction_variant() {
        assert!(is_bad_image_description(
            "I don't see any images in the message."
        ));
    }

    #[test]
    fn rejects_no_image_provided() {
        assert!(is_bad_image_description(
            "No image was provided for analysis."
        ));
    }

    #[test]
    fn accepts_valid_description() {
        assert!(!is_bad_image_description(
            "A screenshot of a calendar invitation showing a meeting titled 'Standup' on March 17, 2026 at 09:00-09:15."
        ));
    }

    #[test]
    fn accepts_ocr_description() {
        assert!(!is_bad_image_description(
            "The image shows a document with the text: 'Møte med Alex, 14. mars kl 10:00-11:00'"
        ));
    }
}

#[cfg(test)]
mod intent_loop_tools_tests {
    use crate::llm::tool_names as tn;

    /// Intent sub-loops must include notification tools so intents can send
    /// notifications. Regression test for: send_notification silently fails
    /// when called from execute_intent because the tool wasn't in the tool list.
    #[test]
    fn intent_loop_tools_include_send_notification() {
        let tools = super::build_intent_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&tn::SEND_NOTIFICATION),
            "Intent loop tools must include send_notification, got: {:?}",
            names
        );
    }

    #[test]
    fn intent_loop_tools_include_read_notifications() {
        let tools = super::build_intent_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&tn::READ_NOTIFICATIONS),
            "Intent loop tools must include read_notifications, got: {:?}",
            names
        );
    }

    #[test]
    fn intent_loop_tools_exclude_execute_intent() {
        let tools = super::build_intent_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            !names.contains(&tn::EXECUTE_INTENT),
            "Intent loop tools must NOT include execute_intent (no recursion)"
        );
    }
}

#[cfg(test)]
mod derive_call_key_tests {
    use super::derive_call_key;
    use crate::llm::tool_names as tn;
    use serde_json::json;

    #[test]
    fn run_bash_buckets_by_first_token() {
        let key = derive_call_key(tn::RUN_BASH, &json!({ "command": "git status" }));
        assert_eq!(key, "git");
    }

    #[test]
    fn run_bash_trims_leading_whitespace() {
        let key = derive_call_key(tn::RUN_BASH, &json!({ "command": "  git  add ." }));
        assert_eq!(key, "git");
    }

    #[test]
    fn run_bash_empty_command_falls_back_to_tool_name() {
        let key = derive_call_key(tn::RUN_BASH, &json!({ "command": "" }));
        assert_eq!(key, tn::RUN_BASH);
    }

    #[test]
    fn run_bash_whitespace_only_command_falls_back_to_tool_name() {
        let key = derive_call_key(tn::RUN_BASH, &json!({ "command": "   " }));
        assert_eq!(key, tn::RUN_BASH);
    }

    #[test]
    fn run_bash_missing_command_falls_back_to_tool_name() {
        let key = derive_call_key(tn::RUN_BASH, &json!({}));
        assert_eq!(key, tn::RUN_BASH);
    }

    #[test]
    fn run_bash_distinct_commands_bucket_separately() {
        let git = derive_call_key(tn::RUN_BASH, &json!({ "command": "git status" }));
        let cargo = derive_call_key(tn::RUN_BASH, &json!({ "command": "cargo test" }));
        let ls = derive_call_key(tn::RUN_BASH, &json!({ "command": "ls -la" }));
        assert_eq!(git, "git");
        assert_eq!(cargo, "cargo");
        assert_eq!(ls, "ls");
        assert_ne!(git, cargo);
        assert_ne!(cargo, ls);
    }

    #[test]
    fn run_bash_same_prefix_buckets_together() {
        let a = derive_call_key(tn::RUN_BASH, &json!({ "command": "git status" }));
        let b = derive_call_key(tn::RUN_BASH, &json!({ "command": "git add ." }));
        let c = derive_call_key(tn::RUN_BASH, &json!({ "command": "git commit -m x" }));
        assert_eq!(a, "git");
        assert_eq!(b, "git");
        assert_eq!(c, "git");
    }

    #[test]
    fn read_file_keys_by_path_unchanged() {
        let key = derive_call_key(tn::READ_FILE, &json!({ "path": "src/main.rs" }));
        assert_eq!(key, "src/main.rs");
    }

    #[test]
    fn web_search_keys_by_query_unchanged() {
        let key = derive_call_key(tn::WEB_SEARCH, &json!({ "query": "rust async" }));
        assert_eq!(key, "rust async");
    }

    #[test]
    fn non_run_bash_with_command_arg_does_not_bucket_by_command() {
        // Sanity: only run_bash is special-cased. A different tool that happens
        // to carry a `command` arg falls through to the path/url/query lookup.
        let key = derive_call_key(tn::READ_FILE, &json!({ "command": "git status" }));
        assert_eq!(key, "");
    }

    #[test]
    fn non_run_bash_without_known_arg_returns_empty() {
        let key = derive_call_key(tn::LIST_FILES, &json!({}));
        assert_eq!(key, "");
    }
}

#[cfg(test)]
#[path = "agentic_loop_tests.rs"]
mod agentic_loop_db_tests;
