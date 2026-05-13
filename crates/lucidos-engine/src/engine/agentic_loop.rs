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

/// Race a tool execution future against the per-thread cancel token. On
/// cancel, returns `Err("Error: canceled by user")` so the agent loop's
/// `tool_use → tool_result` pairing invariant survives — every emitted
/// `ToolCalled` gets a matching `ToolResult` (with `success: false`)
/// even when the work is aborted mid-await. The outer loop's pre-iter
/// `is_cancelled()` then emits `ResponseCanceled` on the next iteration.
///
/// `biased` poll order means cancel ALWAYS wins when the token is already
/// cancelled — without it, an instantly-ready tool result could race with
/// a pending cancel and leak one extra iteration past the user's Stop.
///
/// For subprocess-spawning tools (`run_python`, `run_bash`, MCP), this is
/// only half the fix: the inner future being dropped here must also tear
/// down the OS child, which requires `kill_on_drop(true)` on the
/// `tokio::process::Command`. Without that, dropping the future leaks the
/// child process even though the agent loop unblocks.
pub(super) async fn run_tool_with_cancel<F>(
    fut: F,
    cancel_token: &CancellationToken,
) -> super::tools::ToolOutcome
where
    F: std::future::Future<Output = super::tools::ToolOutcome>,
{
    tokio::select! {
        biased;
        _ = cancel_token.cancelled() => Err("Error: canceled by user".to_string()),
        r = fut => r,
    }
}

/// Build the tool list for intent sub-loops.
/// Notification tools must be included explicitly — they're not in get_default_tools().
pub(super) fn build_intent_tools() -> Vec<ToolDefinition> {
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

/// One sentinel-prefixed tool result, parsed: which transient `ThreadEvent`
/// to broadcast to the frontend, and (when not `None`) what to substitute
/// into the LLM-visible tool result so the model cannot read the raw JSON
/// and act on it as if the call returned synchronously. `redacted_text:
/// None` means "emit the event but leave the tool result alone" — the
/// EmailConfirm tool already has a description that explains the modal
/// flow correctly, so its raw payload is harmless to the model.
pub(super) struct SentinelMatch {
    pub label: &'static str,
    pub event: super::thread_events::ThreadEvent,
    pub redacted_text: Option<String>,
}

/// Inspect a tool result for any front-end confirm-flow sentinel and, if one
/// matches, return the transient event to emit + the LLM-facing replacement
/// text (or `None` to leave the LLM's view of the result unchanged).
pub(super) fn match_sentinel(text: &str) -> Option<SentinelMatch> {
    use super::thread_events::ThreadEvent;
    use super::tools::credentials::CREDENTIAL_REQUEST_PREFIX;
    use super::tools::plugins::{
        PLUGIN_INSTALL_REQUEST_PREFIX, PLUGIN_UNINSTALL_REQUEST_PREFIX,
    };

    type SentinelEntry = (
        &'static str,
        &'static str,
        fn(String) -> ThreadEvent,
        Option<&'static str>,
    );
    let entries: &[SentinelEntry] = &[
        (
            CREDENTIAL_REQUEST_PREFIX,
            "[AgenticLoop] CredentialRequest",
            |payload| ThreadEvent::CredentialRequest { payload },
            Some("Credential request modal shown to the user. Wait for them to enter the credential or cancel — do not chat-ask for the same value; the modal resolves the request."),
        ),
        (
            PLUGIN_INSTALL_REQUEST_PREFIX,
            "[AgenticLoop] PluginInstallRequest",
            |payload| ThreadEvent::PluginInstallRequest { payload },
            Some("Install panel shown to the user. Wait for them to click Confirm or Cancel — do not chat-ask about overwrites and do not claim the install succeeded; the panel resolves it and the next user message will tell you the outcome."),
        ),
        (
            PLUGIN_UNINSTALL_REQUEST_PREFIX,
            "[AgenticLoop] PluginUninstallRequest",
            |payload| ThreadEvent::PluginUninstallRequest { payload },
            Some("Uninstall panel shown to the user. Wait for them to click Confirm or Cancel — do not chat-ask which files to delete and do not claim files were removed; the panel resolves it and the next user message will tell you the outcome."),
        ),
        (
            "[EMAIL_CONFIRM]",
            "[AgenticLoop] EmailConfirmRequest",
            |payload| ThreadEvent::EmailConfirmRequest { payload },
            None,
        ),
    ];

    for &(prefix, label, ctor, redacted) in entries {
        if !text.starts_with(prefix) {
            continue;
        }
        let after = &text[prefix.len()..];
        let rel_start = after.find('{')?;
        let payload = after[rel_start..].to_string();
        return Some(SentinelMatch {
            label,
            event: ctor(payload),
            redacted_text: redacted.map(str::to_string),
        });
    }
    None
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
    if crate::engine::thread_events::has_terminator_for(pool, thread_id, request_event_id).await {
        return;
    }

    crate::log!(
        "[AgenticLoop] WARNING: loop exited without emitting a terminator for request {} on thread {} — emitting defensive ResponseAborted",
        request_event_id,
        thread_id
    );
    crate::engine::thread_events::emit_response_aborted(
        bus,
        thread_id,
        crate::engine::thread_events::AbortCause::ProcessKilled,
        String::new(),
        vec![],
        None,
        None,
        crate::engine::thread_events::EventMeta {
            request_event_id: Some(request_event_id),
            channel,
            ..crate::engine::thread_events::EventMeta::NONE
        },
        "[AgenticLoop] ResponseAborted (defensive — no terminator emitted)",
    )
    .await;
}

/// Static parts of a ContextCaptured event built once by `chat::process`
/// before the loop starts: section list (system + memory + history + …),
/// the tool list, and the model id. The loop appends a dynamic
/// `Conversation` section per iteration sized from the current `messages`.
///
/// `capture_body` mirrors `PreferenceStore::capture_context` — read once at
/// build time and threaded through so the loop can fill the dynamic
/// `Conversation` section's body when the user has the preference on.
/// Without this, the body stayed `None` even with capture on, and the
/// modal misleadingly showed "Body not captured (capture_context off)".
pub(crate) struct ContextCaptureSeed<'a> {
    pub sections: &'a [crate::engine::ContextSection],
    pub tools: &'a [String],
    pub model: &'a str,
    pub capture_body: bool,
}

/// Render the in-flight `messages` array as a compact text body for the
/// `Conversation` section's persisted content. Each message is prefixed with
/// its role; tool calls/results are summarized inline so the dump stays
/// readable rather than dumping raw JSON. Caller truncates with
/// `truncate_head_tail` when over the persistence cap.
fn serialize_messages_for_capture(messages: &[Message]) -> String {
    let mut out = String::new();
    for msg in messages {
        out.push_str(&format!("[{}]\n", msg.role));
        match &msg.content {
            MessageContent::Text(s) => out.push_str(s),
            MessageContent::Blocks(blocks) => {
                for block in blocks {
                    match block {
                        ContentBlock::Text { text } => out.push_str(text),
                        ContentBlock::ToolUse { name, input, id } => {
                            out.push_str(&format!(
                                "\n[tool_use {} id={} input={}]",
                                name, id, input
                            ));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            out.push_str(&format!(
                                "\n[tool_result id={} content={}]",
                                tool_use_id, content
                            ));
                        }
                        ContentBlock::Image { .. } => out.push_str("\n[image]"),
                    }
                }
            }
        }
        out.push_str("\n\n");
    }
    out
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
        let capture_window = super::context::context_window_for(capture_seed.model);

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
                crate::engine::thread_events::emit_response_canceled(
                    &self.event_bus,
                    &self.pool,
                    thread_id,
                    crate::engine::thread_events::CancelCause::UserStop,
                    String::new(),
                    images.clone(),
                    effective_model.clone(),
                    effective_effort.clone(),
                    meta.clone(),
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
                    crate::engine::thread_events::emit_response_canceled(
                        &self.event_bus,
                        &self.pool,
                        thread_id,
                        crate::engine::thread_events::CancelCause::UserStop,
                        partial.clone(),
                        images.clone(),
                        effective_model.clone(),
                        effective_effort.clone(),
                        meta.clone(),
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
            // OpenAI/Gemini stay None and the chars/4 estimate stands.
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
                        super::context::truncate_head_tail(
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
            };
            let estimated_total_tokens: usize = (system_chars + context_chars) / 4;
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

                // Check read cache first for read_file
                let outcome: crate::engine::tools::ToolOutcome = if tool_call.name == tn::READ_FILE
                {
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
                        Ok(cached.clone())
                    } else {
                        let r = run_tool_with_cancel(
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
                        .await;
                        if let Ok(ref text) = r {
                            read_cache.insert(path.to_string(), text.clone());
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
                    match &prompt.kind {
                        super::InjectedPromptKind::WakeFromChild {
                            child_thread_id: child_id,
                            child_completed_event_id,
                        } => {
                            crate::log!(
                                "[Inject] Wake-from-child {} (event {}) into active parent {}",
                                child_id,
                                child_completed_event_id,
                                thread_id
                            );
                            let row = match self
                                .event_store
                                .get_event_by_id(*child_completed_event_id)
                                .await
                            {
                                Ok(Some(r)) => r,
                                Ok(None) => {
                                    crate::log!(
                                        "[Inject] Wake-from-child event {} missing in DB; skipping projection",
                                        child_completed_event_id
                                    );
                                    continue;
                                }
                                Err(e) => {
                                    crate::log!(
                                        "[Inject] Failed to load wake-from-child event {}: {}; skipping projection",
                                        child_completed_event_id,
                                        e
                                    );
                                    continue;
                                }
                            };
                            let block =
                                crate::core::store::format_child_thread_completed_block(&row);
                            messages.push(Message {
                                role: "user".to_string(),
                                content: MessageContent::Text(block),
                            });
                            continue;
                        }
                        super::InjectedPromptKind::UserText => {}
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

}

#[path = "agentic_loop_special_tool.rs"]
mod special_tool;

#[cfg(test)]
#[path = "agentic_loop_unit_tests.rs"]
mod unit_tests;

#[cfg(test)]
#[path = "agentic_loop_tests.rs"]
mod agentic_loop_db_tests;
