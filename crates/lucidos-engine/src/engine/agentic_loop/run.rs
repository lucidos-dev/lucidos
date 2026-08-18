//! The agentic-loop driver (`run_agentic_loop`): call LLM → parse → execute
//! tools → repeat. A single cohesive `loop` with labeled control flow and
//! interdependent mutable state; left as one method.

use crate::engine::inline_question_repair::InlineQuestionLeak;
use crate::llm::provider::LlmResponse;
use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use crate::llm::{ContentBlock, Message, MessageContent};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::super::context::{
    estimate_message_chars, estimate_tokens_from_chars, trim_context_if_needed,
};
use super::super::types::*;
use super::super::LucidosEngine;
use super::helpers::*;

impl LucidosEngine {
    /// After the first LLM call, emit the derived `ImageDescribed` fact(s) for
    /// the turn's attached images — see [`emit_image_descriptions`] for what
    /// lands and when it no-ops.
    ///
    /// What this wrapper adds is the turn context: the actual image bytes are
    /// deliberately KEPT in the user message for the whole turn (preserved by
    /// `trim_context_if_needed`'s image pins) so the model can still see the
    /// image after intervening tool calls. The description is a record of what
    /// was shown, never a substitute for the bytes. Callers run this only at
    /// `rounds == 1`, and `take()` empties the handle, so a second call is
    /// a no-op.
    async fn emit_image_descriptions_after_first_llm_call(
        &self,
        image_description_handle: &mut Option<tokio::task::JoinHandle<Option<(String, String)>>>,
        origin_id: Uuid,
        thread_id: Uuid,
        meta: &crate::engine::thread_events::EventMeta,
    ) {
        emit_image_descriptions(
            &self.event_store,
            &self.event_bus,
            thread_id,
            origin_id,
            meta.channel,
            image_description_handle.take(),
        )
        .await;
    }

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
        mut image_description_handle: Option<tokio::task::JoinHandle<Option<(String, String)>>>,
        origin_id: Uuid,
        proposed_change: &mut bool,
        user_images: Option<&[crate::api::ChatImage]>,
        device_id: Option<&str>,
        model_override: Option<&str>,
        reasoning_effort: Option<&str>,
        cancel_token: &CancellationToken,
        injection_rx: &mut mpsc::UnboundedReceiver<super::super::InjectedPrompt>,
        // This turn's registration generation, so a drain reported after a
        // force-evict can't decrement a newer turn's unread count. Travels
        // with `injection_rx` — the two describe the same registration.
        injection_generation: u64,
        // Set to true once this turn's ENDING is accounted for, so the
        // post-loop guard in `chat::process` can skip its
        // `payload->>'request_event_id'` existence check. The check has no
        // functional index and would walk every event in long-lived threads
        // otherwise.
        //
        // "Settled" rather than "emitted" because there are two ways to
        // account for an ending: emit a terminator (every branch below but
        // one), or park on an `await_event` wait, which deliberately emits
        // NONE. A parked thread has not finished, so a synthesized
        // `ResponseAborted` would report it as interrupted and drag it out of
        // `waiting_for_event`.
        terminator_settled: &mut bool,
        capture_seed: ContextCaptureSeed<'_>,
        // The firing trigger's declared side-effect grant (ADR 0002, Phase 5).
        // Empty for chat turns. Consulted by the command guard only on the
        // `Trigger` channel to gate `IrreversibleDanger` commands.
        trigger_side_effect_grant: &[crate::engine::command_guard::SideEffectCategory],
        // This turn's resolved per-turn tool-call cap (the `max_tool_calls`
        // preference, defaulting to `DEFAULT_MAX_TOOL_CALLS`). Passed in rather
        // than read here so the caller reads it ONCE and hands the same number
        // to the system-prompt builder: the prompt states this cap to the model,
        // and a prompt that names a different number than the loop enforces is
        // exactly the kind of fabricated engine internal the prompt warns
        // against. Passing it also freezes the cap for the turn, so a mid-turn
        // Settings change cannot move it under a running loop.
        max_tool_calls: usize,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        // EventMeta for this request — all persisted events in this cycle share the same context
        let meta = crate::engine::thread_events::EventMeta {
            request_event_id: Some(origin_id),
            channel: response_channel,
            ..crate::engine::thread_events::EventMeta::NONE
        };

        // Pin ONE provider Arc for this whole response — a mid-response runtime
        // swap (credential added/removed) must not change the in-flight provider.
        let provider = self.current_provider();
        let model_str = model_override.unwrap_or(provider.default_model());
        let effective_model = (!model_str.is_empty()).then(|| model_str.to_string());
        let effective_effort = reasoning_effort.map(|s| s.to_string());
        let capture_window = self.context_window_for(capture_seed.model);

        // Command guard (ADR 0002): off unless the workspace turned on the
        // `command_guard` preference. Read once per response — the toggles can't
        // change mid-response — and consulted before each bash/python dispatch
        // below. The judge sub-toggle + model are only read when the guard is on
        // (no DB cost on the common off path). `command_guard_ctx` carries the
        // per-response judge-verdict cache so a re-emitted identical command
        // doesn't re-pay the LLM call.
        let command_guard_enabled = crate::core::PreferenceStore::command_guard(&self.pool).await?;
        let (command_guard_judge_enabled, command_judge_model) = if command_guard_enabled {
            (
                crate::core::PreferenceStore::command_guard_judge(&self.pool).await?,
                crate::core::PreferenceStore::command_judge_model(&self.pool).await,
            )
        } else {
            (false, String::new())
        };
        let mut command_guard_judge_cache: std::collections::HashMap<
            String,
            crate::engine::command_guard::JudgedClassification,
        > = std::collections::HashMap::new();

        // Rounds run so far this turn. A *round* is one LLM API call plus the
        // execution of every tool call its response carried (see `docs/glossary.md`
        // § Round); this is the unit the loop iterates in.
        let mut rounds = 0;
        // Tool calls made so far this turn, against `max_tool_calls`. Distinct
        // from `rounds`: one response can carry several tool calls, so the two
        // diverge and only this one matches what the setting, the prompt and
        // the terminator all call a "tool call".
        let mut tool_calls_made = 0usize;
        // Capture before the loop pushes assistant/tool messages and shifts the index.
        // Maintained across rounds: trim pass 2 may remove older messages, which
        // shifts the captured index down by the number removed (handled below).
        let mut user_message_idx = messages.len().saturating_sub(1);
        // Messages whose image bytes must survive trim pass 0 for the whole turn.
        // Split by provenance because only one side needs a bound:
        //
        // `user_image_idxs` — the turn's user message plus any mid-turn injection
        // that carried images. The user's own content; bounded by how many
        // messages they send in one turn, so it needs no cap.
        //
        // `explicit_image_idxs` — tool results holding an image the model asked
        // to see (`view_image` / `read_file`). Capped, because the model can
        // issue as many of these in one turn as its cap allows, and pinned images are
        // exempt from pass 0 by construction; a "describe every photo in this
        // folder" turn would otherwise accumulate hundreds of un-strippable
        // images and blow the context window.
        //
        // Without these pins an image went blind on the model's very next tool
        // call — see the trim doc comment.
        let mut user_image_idxs: Vec<usize> = vec![user_message_idx];
        let mut explicit_image_idxs: Vec<usize> = Vec::new();
        let mut images: Vec<String> = Vec::new(); // Track screenshots created during this request
        let mut last_tool_call: Option<(String, String)> = None; // (tool_name, key) - key derived by derive_call_key
        let mut consecutive_same_call = 0;
        // Consecutive-*failure* streak for the generic circuit breaker (distinct
        // from `consecutive_same_call`, which counts ALL repeats and still drives
        // the content-deterministic read_file / list_files breakers). This grows
        // only when the model repeats the same `(tool, key)` AND the previous
        // identical call failed — a successful repeat or a different call resets
        // it. `last_call_was_error` carries the prior single-call result forward.
        let mut consecutive_failing_call = 0usize;
        let mut last_call_was_error = false;
        // Re-ask guard: set when an iteration's `ask_user_question` call errored
        // (e.g. the model dropped the required `question` field), read by the
        // next iteration's no-tool-calls branch so a rejected ask can't silently
        // degrade into a prose typed-reply menu. `forced` bounds it per response.
        let mut question_ask_failed_last_iter = false;
        let mut question_reask_forced = 0usize;
        // Wake check: how many times this turn has been sent back for leaving
        // todo work open with nothing that would re-open the thread. Bounded by
        // `MAX_TODO_WAKE_NUDGE`, so a model that answers and still walks away
        // finalizes normally.
        let mut todo_wake_nudges_forced = 0usize;
        let mut cached_list_files: Option<String> = None;
        let mut modified_app_uis: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        // Agent loop
        loop {
            rounds += 1;
            if cancel_token.is_cancelled() {
                crate::engine::thread_events::emit_response_canceled(
                    &self.event_bus,
                    &self.pool,
                    thread_id,
                    cancel_cause_for_turn(self, thread_id),
                    String::new(),
                    images.clone(),
                    effective_model.clone(),
                    effective_effort.clone(),
                    meta_with_cancel_actor(self, thread_id, &meta),
                    "[AgenticLoop] ResponseCanceled (cancel pre-iter)",
                )
                .await;
                *terminator_settled = true;
                return Ok(terminal_result(
                    String::new(),
                    images,
                    request_id,
                    thread_id,
                    *proposed_change,
                ));
            }

            // Two backstops, because they catch different runaways.
            //
            // The user's cap: at most `max_tool_calls` calls, with the response
            // that crosses the line completing first (every `tool_use` needs a
            // matching `tool_result` or the provider rejects the next request,
            // so a response's calls cannot be abandoned half-executed).
            //
            // The round backstop: several paths `continue` WITHOUT executing a
            // tool call, so they never advance the count. Each is bounded on its
            // own, but this keeps "the turn always ends" unconditional rather
            // than a property of every path staying bounded forever. See
            // `NON_TOOL_ROUND_SLACK`.
            let cap_message = if tool_calls_made >= max_tool_calls {
                Some(tool_call_cap_message(max_tool_calls))
            } else if rounds > max_tool_calls.saturating_add(NON_TOOL_ROUND_SLACK) {
                Some(round_backstop_message(rounds))
            } else {
                None
            };
            if let Some(cap_message) = cap_message {
                let msg = emit_iteration_cap_response_generated(
                    &self.event_bus,
                    thread_id,
                    &meta,
                    images.clone(),
                    effective_model.clone(),
                    effective_effort.clone(),
                    cap_message,
                )
                .await;
                *terminator_settled = true;
                return Ok(terminal_result(
                    msg,
                    images,
                    request_id,
                    thread_id,
                    *proposed_change,
                ));
            }

            // Pin the current turn's user message so pass 2 cannot drop it
            // (`Some(user_message_idx)`), and keep every tracked image-bearing
            // message's bytes for the whole turn (`keep_image_idxs`). The
            // recent-tail rule alone fails once enough tool rounds shift a
            // message out of the last PRESERVE_RECENT_MESSAGES slots; losing it
            // strips the request line from every subsequent call (model reports
            // "I lost track of what you asked"), and stripping its image blinds
            // the model to what it was looking at.
            let mut keep_image_idxs = user_image_idxs.clone();
            keep_image_idxs.extend_from_slice(&explicit_image_idxs);
            let trim_outcome = trim_context_if_needed(
                messages,
                message_budget,
                Some(user_message_idx),
                &keep_image_idxs,
            );
            let removed_count = trim_outcome.messages_removed;
            // `trimmed` reports ANY content loss, not just pass-2 eviction —
            // passes 1/1.5 replace tool-result bodies with a truncation note,
            // which the UI previously showed as an untrimmed turn.
            let trimmed = trim_outcome.any();
            // Pass 2 of trim removes oldest messages from index 1; the protected
            // index guard ensures every removal sits strictly below
            // user_message_idx, so it shifts down by removed_count.
            user_message_idx = user_message_idx.saturating_sub(removed_count);
            // Every pin sits at or above pass 2's eviction floor (which is the
            // minimum over the pins), so none of them was removed — the uniform
            // shift stays exact and no pin can end up aliasing a different
            // message.
            for idx in user_image_idxs
                .iter_mut()
                .chain(explicit_image_idxs.iter_mut())
            {
                *idx = idx.saturating_sub(removed_count);
            }
            // Safety net: validate tool_use/tool_result pairing after trimming.
            // The primary fix ensures correct block ordering, but this catches any
            // edge case where pairing breaks (trimming bugs, injection ordering, etc.)
            crate::llm::validate::validate_tool_use_pairing(messages);
            // Always measure AFTER trimming — pass 0 strips images even when pass 2
            // removes no messages, so pre-trim chars can be wildly inflated.
            let context_chars: usize = messages.iter().map(estimate_message_chars).sum();

            // chars-to-tokens at the measured 2.5 chars/token display rate, not
            // the budget's conservative 1.5 (see both doc comments in
            // `context.rs`). This line is read by a human next to the model's
            // context window, so it wants accuracy; the budget wants a margin.
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
                    // Mid-stream defense against the model emitting a tool call
                    // as inline XML text instead of a structured tool_use block.
                    // Two leak shapes: `<ask_user_question>...</ask_user_question>`
                    // and the generic `<invoke name="...">...</invoke>`. Once the
                    // opening fragment appears in the buffer, stop flushing deltas
                    // so the user doesn't see the raw tag in their live view. The
                    // post-response repair paths below strip the tag and synthesise
                    // a real tool call. See `inline_question_repair` /
                    // `inline_tool_call_repair`.
                    if crate::engine::inline_question_repair::buffer_contains_inline_tag(&text)
                        || crate::engine::inline_tool_call_repair::buffer_contains_inline_tool_call(
                            &text,
                        )
                    {
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

            // Race LLM call against cancel token so stop button works immediately.
            // Scoped for the prompt-cache probe, which reads the correlation off
            // the task rather than through the provider trait (see
            // `llm::cache_probe`); inert unless `LUCIDOS_CACHE_PROBE` is set.
            let llm_future = crate::llm::cache_probe::scope(
                crate::llm::cache_probe::ProbeCall {
                    thread_id,
                    turn_id: origin_id,
                    round: rounds,
                },
                provider.chat(
                    messages.clone(),
                    call_tools,
                    model_override,
                    Some(system_prompt),
                    token_cb,
                    reasoning_effort,
                ),
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
                            *terminator_settled = true;
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
                        cancel_cause_for_turn(self, thread_id),
                        partial.clone(),
                        images.clone(),
                        effective_model.clone(),
                        effective_effort.clone(),
                        meta_with_cancel_actor(self, thread_id, &meta),
                        "[AgenticLoop] ResponseCanceled",
                    )
                    .await;
                    *terminator_settled = true;
                    return Ok(terminal_result(
                        partial,
                        images,
                        request_id,
                        thread_id,
                        *proposed_change,
                    ));
                }
            };

            // `usage` only when the provider reported it (Anthropic);
            // OpenAI/Gemini stay None and the chars*2/5 estimate stands (see
            // `estimate_tokens_from_chars`). Those two tokenizers measured more
            // efficient than the Claude-calibrated ratio, so the fallback errs
            // toward over-reporting there, which is the safe direction.
            //
            // Sections describe the prompt-time breakdown (system + memory
            // + history + … + user message). All non-system sections are
            // already concatenated into messages[0].content, so summing
            // them and `context_chars` would double-count. The Conversation
            // section instead carries the *delta* — bytes added by the
            // tool loop on iter 2+ — so the section list sums to the live
            // total: system_prompt + context_chars.
            let static_total: usize = capture_seed.sections.iter().map(|s| s.char_count).sum();
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
            let conversation_body = capture_seed.capture_body.then(|| {
                let serialized = serialize_messages_for_capture(messages);
                if serialized.len() > CONVERSATION_PERSIST_MAX {
                    super::super::context::truncate_head_tail(&serialized, CONVERSATION_PERSIST_MAX)
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
            // Tool schemas are part of every request and the trim budget already
            // subtracts them, so the reported total must include them too —
            // otherwise the Context Viewer under-reports what was actually sent.
            let estimated_total_tokens: usize = estimate_tokens_from_chars(
                system_chars + capture_seed.tool_defs_chars + context_chars,
            );
            // Surface the schemas as their own row so the section tree still
            // adds up to `estimated_total_tokens`. Counting them in the total
            // without showing them would leave a user auditing a near-full
            // context unable to see where a large fixed chunk went. No body —
            // the schemas are generated, and dumping ~70 of them would dwarf
            // the capture.
            let tool_definitions = crate::engine::ContextSection {
                name: format!("Tool Definitions ({})", capture_seed.tools.len()),
                content: None,
                char_count: capture_seed.tool_defs_chars,
                role: crate::engine::ContextRole::System,
                group: None,
            };
            let iter_sections: Vec<_> = capture_seed
                .sections
                .iter()
                .cloned()
                .chain(std::iter::once(tool_definitions))
                .chain(std::iter::once(conversation))
                .collect();
            let usage = response
                .input_tokens
                .map(|input_tokens| crate::engine::ApiUsage {
                    input_tokens,
                    output_tokens: response.output_tokens.unwrap_or(0),
                    cache_read_tokens: response.cache_read_tokens.unwrap_or(0),
                    cache_creation_tokens: response.cache_creation_tokens.unwrap_or(0),
                });
            // Calibration breadcrumb for the chars/token ratio baked into
            // `estimate_tokens_from_chars`. This line is what retuned it from
            // 1.5 to the measured 2.5: 12,069 captures gave p01 2.28, p50 2.60,
            // p99 2.74, tight enough to act on where a single eyeballed capture
            // had not been (two OpenRouter routes in one thread once reported
            // the same prompt as 2.76 and 1.79). Keep logging it. The ratio is
            // still one Claude-calibrated constant, and a per-family split
            // needs this same query run against a fatter GPT/Gemini sample.
            // `estimated` deliberately includes the tool schemas, matching what
            // the budget subtracts, so the comparison is like-for-like.
            if let Some(u) = usage {
                let counted_chars = system_chars + capture_seed.tool_defs_chars + context_chars;
                log!(
                    "[Context] calibration model={} estimated={} actual_input={} \
                     cache_read={} cache_creation={} chars={} implied_chars_per_token={:.2}",
                    capture_seed.model,
                    estimated_total_tokens,
                    u.input_tokens,
                    u.cache_read_tokens,
                    u.cache_creation_tokens,
                    counted_chars,
                    if u.input_tokens > 0 {
                        counted_chars as f64 / u.input_tokens as f64
                    } else {
                        0.0
                    }
                );
            }
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

            // Post-response repair: the model sometimes emits
            // `<ask_user_question>...</ask_user_question>` as inline text
            // instead of a structured tool call, and sometimes emits the bare
            // payload with no tag at all. Detect that case BEFORE the final
            // flush so the cleaned text is what streams to the frontend AND
            // persists. A dispatchable payload becomes a synthesised tool call,
            // appended here so the existing tool-execution branch takes over
            // from `if response.tool_calls.is_empty()`. A degenerate one keeps
            // its prose and forces a bounded re-ask further down. See
            // `inline_question_repair` for the detection contract and why the
            // prompt-side rule alone is insufficient.
            let inline_repair = response
                .content
                .as_deref()
                .and_then(crate::engine::inline_question_repair::detect_inline_ask_user_question);
            let mut response = response;
            if let Some(ref leak) = inline_repair {
                let cleaned_text = match leak {
                    InlineQuestionLeak::Dispatch { cleaned_text, .. } => cleaned_text,
                    InlineQuestionLeak::Degenerate { cleaned_text } => cleaned_text,
                };
                response.content = (!cleaned_text.is_empty()).then(|| cleaned_text.clone());
            }
            match inline_repair {
                Some(InlineQuestionLeak::Dispatch {
                    ref questions_json, ..
                }) => {
                    let synth_id = format!("synth-aq-{}", uuid::Uuid::new_v4());
                    response.tool_calls.push(crate::llm::ToolCall {
                        id: synth_id.clone(),
                        name: tn::ASK_USER_QUESTION.to_string(),
                        arguments: serde_json::json!({ "questions": questions_json }),
                        thought_signature: None,
                    });
                    crate::log!(
                        "[InlineQuestionRepair] thread={} synthesised tool call from leaked ask_user_question payload (synth_id={})",
                        thread_id,
                        synth_id,
                    );
                }
                Some(InlineQuestionLeak::Degenerate { .. }) => {
                    crate::log!(
                        "[InlineQuestionRepair] thread={} stripped a leaked <ask_user_question> tag with no dispatchable payload, forcing a re-ask",
                        thread_id,
                    );
                }
                None => {}
            }
            // Whether THIS iteration stripped a degenerate tag. Read by the
            // re-ask guard in the termination branch below.
            let question_leaked_as_text =
                matches!(inline_repair, Some(InlineQuestionLeak::Degenerate { .. }));
            // Post-response repair (generic tool call): the model sometimes
            // emits a tool call as inline `<invoke name="...">...</invoke>` XML
            // text instead of a structured tool_use block — the same leak class
            // as the question repair above, a different tag (observed: a
            // `bash_output` poll written as text mid-release, terminating the
            // turn with raw XML as the response). Only when the model made NO
            // real tool call (a genuine leak), reconstruct the call and push it
            // so the existing tool-execution branch resumes the turn instead of
            // persisting the XML. The detector requires a registered tool name +
            // a non-code-fenced block, so prose/examples don't mis-fire. The
            // synthesised call flows through the same command guard + circuit
            // breakers as a real call. See `inline_tool_call_repair`.
            let tool_call_repair = if response.tool_calls.is_empty() {
                let known: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
                response.content.as_deref().and_then(|c| {
                    crate::engine::inline_tool_call_repair::detect_inline_tool_call(c, &known)
                })
            } else {
                None
            };
            if let Some(ref d) = tool_call_repair {
                response.content = if d.cleaned_text.is_empty() {
                    None
                } else {
                    Some(d.cleaned_text.clone())
                };
                let synth_id = format!("synth-tc-{}", uuid::Uuid::new_v4());
                response.tool_calls.push(crate::llm::ToolCall {
                    id: synth_id.clone(),
                    name: d.name.clone(),
                    arguments: d.arguments.clone(),
                    thought_signature: None,
                });
                crate::log!(
                    "[InlineToolCallRepair] thread={} synthesised '{}' tool call from inline <invoke> tag (synth_id={})",
                    thread_id,
                    d.name,
                    synth_id,
                );
            }
            // Post-response repair (argument text): the model sometimes
            // HTML-entity-escapes the text it puts in a tool argument, so a
            // trigger group the user asked to call `Machine & Tooling Health`
            // is created, persisted and re-served as
            // `Machine &amp; Tooling Health`. Runs here, after both synthesis
            // repairs and before anything reads the arguments, so the tool
            // handler, the `ToolCalled` args, its derived description, the
            // domain event the handler emits and the tool result that echoes
            // back to the model all carry the literal text. Only plain-text
            // label arguments are touched; markup arguments keep their
            // escaping. See `tool_arg_entity_repair` for why this is the
            // model's doing and not ours.
            for tool_call in response.tool_calls.iter_mut() {
                if crate::engine::tool_arg_entity_repair::repair_tool_arg_entities(
                    &tool_call.name,
                    &mut tool_call.arguments,
                ) {
                    crate::log!(
                        "[ToolArgEntityRepair] thread={} decoded HTML entities in '{}' arguments",
                        thread_id,
                        tool_call.name,
                    );
                }
            }
            // Final flush — send any remaining buffered text and persist
            // remainder. This includes the assistant's preamble on a tool-call
            // turn ("Let me organize the cards…" before write_file): the loop
            // streams it so the agent explains along the way, not just at the
            // end. When inline repair fired, use the cleaned text (tag-stripped)
            // so the frontend's live view and persisted TextStreamed events both
            // reflect the repaired body.
            let (flush_text, remaining_to_persist) = {
                let raw = raw_buffer.lock().unwrap();
                // When EITHER repair fired, the raw buffer still holds the
                // leaked tag (the streaming callback appends before suppressing),
                // so force the flush/persist to the cleaned text — the final
                // `response.content` both repairs leave behind. Otherwise fall
                // back to the raw stream / content as before.
                let cleaned_override = (inline_repair.is_some() || tool_call_repair.is_some())
                    .then(|| response.content.as_deref().unwrap_or(""));
                let effective: &str = effective_flush_text(
                    cleaned_override,
                    raw.as_str(),
                    response.content.as_deref(),
                );
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
                            event:
                                crate::engine::thread_events::ThreadEvent::CumulativeTextUpdated {
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
                // Force a re-ask: the previous iteration's `ask_user_question`
                // call errored (typically the model dropped the required
                // `question` field and put the text in the optional `header`
                // chip), and the model has now returned a prose answer instead
                // of re-calling the tool — collapsing the clickable question
                // card into a typed-reply menu the user can't tap. The schema
                // marks `question` required and the runtime check already
                // rejected the bad call, but provider tool-calling treats
                // `required` as advisory, so the model can both omit it AND then
                // abandon the question. Push a forcing instruction and loop so
                // it re-asks properly. Bounded by `MAX_QUESTION_REASK` so a
                // persistently-failing question path still finalizes.
                // Gate on actual prose: this guard targets the "answered in
                // prose instead of re-asking" degradation, which always has
                // content. An empty completion after a failed ask is a
                // different case — let it fall through to the empty-completion
                // classifier below. Requiring `Some(answer)` also keeps
                // alternation valid: the assistant message is always pushed
                // before the forcing user message, never a lone user-after-user.
                //
                // Two causes reach here, sharing one budget. A rejected call is
                // the case above. The other is a tag the model typed as text
                // whose body carried no dispatchable payload: no call was made
                // at all, so the user would read the question as prose and get
                // no card. Claude Code already redirects its own plaintext
                // questions back to the tool, from the Stop hook in
                // `cc_stop_reminder.rs`. This is the chat-side equivalent. It
                // keys on the leaked tag rather than on a trailing `?`, which
                // is the stronger signal the engine has and the hook does not.
                let reask = question_reask_cause(
                    question_ask_failed_last_iter,
                    question_leaked_as_text,
                    question_reask_forced,
                )
                .and_then(|cause| {
                    response
                        .content
                        .as_deref()
                        .map(|c| self.clean_response(c))
                        .filter(|c| !c.is_empty())
                        .map(|answer| (cause, answer))
                });
                if let Some((cause, answer)) = reask {
                    question_reask_forced += 1;
                    // Mirror the injection-continue path's message handling.
                    // Preserve the drafted prose as assistant context so the
                    // model sees what it just said before re-asking.
                    messages.push(Message {
                        role: "assistant".to_string(),
                        content: MessageContent::Text(answer),
                    });
                    messages.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Text(cause.instruction().to_string()),
                    });
                    let reason = match cause {
                        QuestionReaskCause::CallRejected => {
                            "ask_user_question had no question text, forcing re-ask"
                        }
                        QuestionReaskCause::LeakedAsText => {
                            "ask_user_question was typed as text, forcing re-ask"
                        }
                    };
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::LlmCallRetried {
                                    reason: reason.to_string(),
                                },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[AgenticLoop] LlmCallRetried (force question re-ask)",
                        )
                        .await;
                    continue;
                }

                // A user may have sent follow-ups while this LLM call was in
                // flight. If the model returned a plain final answer (no tool
                // loop), this is the last chance to ingest them before the
                // response terminates and they become orphaned follow-up turns.
                // Preserve the draft assistant text in history, append the
                // coalesced injection as one user message, and continue so the
                // agent answers the queued updates in this same turn.
                // Whether the drafted answer is already in `messages` as
                // assistant context. Read by the wake check below, which must
                // not push it a second time: an injection group that drops
                // every prompt as empty leaves `appended` false, so this block
                // pushes the draft and then falls through instead of looping.
                let mut draft_in_history = false;
                let mut injected_prompts = Vec::new();
                while let Ok(prompt) = injection_rx.try_recv() {
                    injected_prompts.push(prompt);
                }
                self.note_injections_drained(
                    thread_id,
                    injection_generation,
                    injected_prompts.len(),
                );
                let injected_prompts =
                    filter_removed_queued_prompts(&self.pool, thread_id, injected_prompts).await;
                if !injected_prompts.is_empty() {
                    if let Some(answer) = response
                        .content
                        .as_deref()
                        .map(|c| self.clean_response(c))
                        .filter(|c| !c.is_empty())
                    {
                        messages.push(Message {
                            role: "assistant".to_string(),
                            content: MessageContent::Text(answer),
                        });
                        draft_in_history = true;
                    }
                    let appended = append_injected_prompts_to_messages(
                        &self.event_bus,
                        thread_id,
                        &meta,
                        messages,
                        injected_prompts,
                    )
                    .await;
                    user_image_idxs.extend(appended.image_message_idxs);
                    if appended.appended {
                        user_message_idx = messages.len().saturating_sub(1);
                        if rounds == 1 {
                            self.emit_image_descriptions_after_first_llm_call(
                                &mut image_description_handle,
                                origin_id,
                                thread_id,
                                &meta,
                            )
                            .await;
                        }
                        continue;
                    }
                }

                // The WAKE CHECK. A turn about to end with open todo work and
                // nothing to re-open the thread. Its closing paragraph very
                // often promises to keep watching, with nothing behind it.
                // Three prose guards already say so and have failed twice, so
                // the engine asks once here instead. See ADR 0071 and
                // `docs/plans/2026-08-13-a-turn-cannot-end-claiming-to-watch.md`.
                //
                // Position is load-bearing in three directions. AFTER the
                // injection drain: a follow-up sent mid-turn re-opens the
                // thread by itself, and nudging first would deny that. AFTER
                // the re-ask guard, because a rejected question is a broken
                // interaction that outranks a stale list. BEFORE the app
                // refresh below, which is end-of-turn work a nudged turn has
                // not reached.
                //
                // Two turns are skipped rather than nudged. One the user
                // Stopped mid-call. The loop only checks the token at the top
                // of a round, so a `continue` would spend the drafted answer to
                // reach it. And one with no drafted prose: it makes no claim to
                // correct, and the classifier below diagnoses it.
                let wake_nudge = if cancel_token.is_cancelled() {
                    None
                } else {
                    let (open, covered) = self.wake_check_facts(thread_id).await;
                    should_nudge_unwatched_turn(open, covered, todo_wake_nudges_forced)
                        .then(|| open.unwrap_or(0))
                        // Lazily, so an ordinary turn does not pay `clean_response`
                        // twice: this runs only once a nudge is otherwise due.
                        .and_then(|open| {
                            response
                                .content
                                .as_deref()
                                .map(|c| self.clean_response(c))
                                .filter(|c| !c.is_empty())
                                .map(|draft| (open, draft))
                        })
                };
                if let Some((open, draft)) = wake_nudge {
                    todo_wake_nudges_forced += 1;
                    // Same message handling as the re-ask path. The drafted
                    // prose goes in as assistant context first, so the model
                    // sees the claim in question, and so the alternation holds.
                    // Unless the drain above already put it there.
                    if !draft_in_history {
                        messages.push(Message {
                            role: "assistant".to_string(),
                            content: MessageContent::Text(draft),
                        });
                    }
                    messages.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Text(todo_wake_nudge_instruction(open)),
                    });
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::LlmCallRetried {
                                    reason: format!(
                                        "{open} todo item(s) open and nothing would re-open this \
                                         thread, asking before the turn ends"
                                    ),
                                },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[AgenticLoop] LlmCallRetried (wake check)",
                        )
                        .await;
                    continue;
                }

                // Refresh anything touched during the tool loop, once per app at
                // end of turn (coalesced — not per write). Two distinct signals:
                //   - AppUiRefreshRequested → reload the open app iframe.
                //   - AppUpdated → refresh the disk-backed apps LIST (name/icon/
                //     description may have changed via a manifest edit). The list
                //     re-scans disk, so this also surfaces an app freshly created
                //     this turn via raw write_file. Guard on app_exists so an app
                //     deleted during the turn doesn't emit a spurious AppUpdated.
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
                    if self.app_manager.app_exists(app_id) {
                        self.event_bus
                            .emit_or_log(
                                crate::engine::event_bus::BusEvent::System(
                                    crate::engine::event_bus::SystemEvent::AppUpdated {
                                        app_id: app_id.clone(),
                                        name: self.app_manager.app_name(app_id),
                                        actor: None,
                                    },
                                ),
                                "[AgenticLoop] AppUpdated (post-loop)",
                            )
                            .await;
                    }
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

                    *terminator_settled = true;
                    return Ok(terminal_result(
                        clean_response,
                        images,
                        request_id,
                        thread_id,
                        *proposed_change,
                    ));
                }

                // Empty completion (no content, no tool calls). Whether this is
                // a failure depends on *why* it was empty, classified uniformly
                // across providers and thread types — see
                // `classify_empty_completion`. A genuine failure (truncation,
                // safety block, dropped output, unrecognised stop) surfaces as
                // ResponseFailed with a full diagnostic; a clean model-decided
                // stop is benign intentional silence and completes the turn
                // normally as an empty ResponseGenerated (the UI renders a
                // neutral "empty response" note instead of a red error).
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
                let class = classify_empty_completion(
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
                let diagnostic = format!(
                    "stop_reason: {}, output_tokens: {}, thinking_chars: {}{}, model: {}{}",
                    stop_reason,
                    output_tokens,
                    thinking_chars,
                    dropped_suffix,
                    effective_model.as_deref().unwrap_or("unknown"),
                    class.hint,
                );

                if class.is_error {
                    let error = format!("Model returned no response ({}).", diagnostic);
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
                } else {
                    // Benign: the model ended its turn cleanly and produced no
                    // text. Complete normally (Idle, no red dot) via an empty
                    // ResponseGenerated; log the diagnostic so an operator can
                    // still see why the turn was empty.
                    crate::log!(
                        "[AgenticLoop] thread={} empty completion treated as benign ({})",
                        thread_id,
                        diagnostic
                    );
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event:
                                    crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                                        text: String::new(),
                                        images: images.clone(),
                                        model: effective_model.clone(),
                                        reasoning_effort: effective_effort.clone(),
                                    },
                                meta: meta.clone(),
                            },
                            "[AgenticLoop] ResponseGenerated (empty completion)",
                        )
                        .await;
                }

                *terminator_settled = true;
                return Ok(terminal_result(
                    String::new(),
                    images,
                    request_id,
                    thread_id,
                    *proposed_change,
                ));
            }

            // Execute each tool call
            let mut tool_outputs = Vec::new();
            let mut had_errors = false;
            // Set if an `ask_user_question` call in THIS iteration errored, then
            // copied into `question_ask_failed_last_iter` after the loop so the
            // NEXT iteration's no-tool-calls branch can force a re-ask.
            let mut this_iter_question_ask_failed = false;
            // Set when the command guard blocks a trigger's command because the
            // side-effect isn't in the trigger's grant (ADR 0002, Phase 5). The
            // blocked command is still recorded as a failed ToolResult (so the
            // transcript is consistent); after this batch of tool calls the loop
            // emits a terminal `ResponseFailed` and returns `Err` so the
            // scheduler fails the trigger. Reset per outer-loop iteration.
            let mut trigger_fail_reason: Option<String> = None;

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
                let is_repeat = Some(&current_call) == last_tool_call.as_ref();
                if is_repeat {
                    consecutive_same_call += 1;
                } else {
                    consecutive_same_call = 1;
                    last_tool_call = Some(current_call);
                }
                // Failure streak for the generic breaker: only grows on a repeat
                // whose previous identical call FAILED; success or a different
                // call resets it (see `next_failure_streak`).
                consecutive_failing_call =
                    next_failure_streak(is_repeat, last_call_was_error, consecutive_failing_call);

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
                            *terminator_settled = true;
                            return Ok(terminal_result(
                                msg.to_string(),
                                images,
                                request_id,
                                thread_id,
                                *proposed_change,
                            ));
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
                        *terminator_settled = true;
                        return Ok(terminal_result(
                            msg,
                            images,
                            request_id,
                            thread_id,
                            *proposed_change,
                        ));
                    }
                    let stop_msg = format!("STOP: You've read '{}' with the same window multiple times. The content hasn't changed. Use the information you have and proceed with your task.", path_for_msg);
                    push_circuit_breaker(messages, &response, &stop_msg);
                    continue;
                }

                // Generic circuit breaker: any tool whose SAME (tool, call_key)
                // FAILED 3+ times in a row. It keys on consecutive *failure*,
                // not consecutive call — successful repetition is by definition
                // not a stuck loop, so productive runs (three distinct `psql`
                // queries bucketed under `psql`, sequential downloads whose
                // scripts share a first line) never trip. There is no per-tool
                // exclusion list: failing repetition of ANY tool (a `run_bash`
                // that errors, an `edit_file` whose `old_string` never matches,
                // a `web_search`/`browser_*` that keeps erroring) is a real
                // stuck loop. read_file / list_files are handled by their own
                // content-deterministic branches above (they block identical
                // *successful* re-reads, which this failure gate intentionally
                // does not). All still bounded by the tool-call cap, and the
                // `bash_output(wait_secs)` server-side block remains the
                // structural fix for the sleep-poll case.
                let breaker_action = generic_breaker_action(consecutive_failing_call);
                if breaker_action != BreakerAction::None {
                    // Human-readable target for the log / STOP / force-break
                    // message — computed once, only when the breaker fires.
                    let target = if call_key.is_empty() {
                        current_tool.to_string()
                    } else {
                        call_key.clone()
                    };
                    if breaker_action == BreakerAction::Break {
                        // Hard break after 5 consecutive failures — the LLM
                        // ignored the warnings.
                        log!(
                            "[AgentLoop] Force-breaking loop: {} failed {} times in a row on '{}'",
                            current_tool,
                            consecutive_failing_call,
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
                        *terminator_settled = true;
                        return Ok(terminal_result(
                            msg,
                            images,
                            request_id,
                            thread_id,
                            *proposed_change,
                        ));
                    }
                    // Soft break at 3-4 consecutive failures — warn the LLM and
                    // let it continue. The proposed (repeat) call is NOT executed,
                    // so `last_call_was_error` stays as-is and the failure streak
                    // survives a continued retry.
                    log!(
                        "[AgentLoop] Warning LLM: {} failed {} times in a row on '{}'",
                        current_tool,
                        consecutive_failing_call,
                        target
                    );
                    let stop_msg = format!(
                        "STOP: Your call to {} on '{}' has failed {} times in a row. Do NOT call it again the same way. Use the results you already have and give your final answer now, or resolve the underlying error first.",
                        current_tool, target, consecutive_failing_call
                    );
                    push_circuit_breaker(messages, &response, &stop_msg);
                    continue;
                }
            } else {
                consecutive_same_call = 0;
                last_tool_call = None;
                // A multi-tool-call turn breaks any single-call repeat streak —
                // clear the failure tracking so the next single call starts fresh.
                consecutive_failing_call = 0;
                last_call_was_error = false;
            }

            for tool_call in response.tool_calls.iter() {
                // Count the CALL, not the round. One response can carry several
                // tool calls (the system prompt asks for exactly that when
                // writing N files), so counting rounds would let a cap of
                // 500 pass well over 500 calls while every user-facing string
                // says "tool calls".
                tool_calls_made += 1;
                // Mask any postgres password the LLM hardcoded into a `bash`
                // command (or other tool) BEFORE it reaches the log line, the
                // persisted `description`, or the persisted `args` — the
                // description renders in the steps UI just like the args, so
                // both must be built from the redacted copy; see
                // `core::redact_postgres_secrets_in_json`.
                let mut redacted_args = tool_call.arguments.clone();
                crate::core::redact_postgres_secrets_in_json(&mut redacted_args);
                let tool_desc = self.describe_tool(&tool_call.name, &redacted_args);
                log!(
                    "[AgentLoop] Step {}/{}: {}",
                    tool_calls_made,
                    max_tool_calls,
                    tool_desc
                );

                // Persist + broadcast ToolCalled. Capture the event_id so spawn-style
                // tools (run_thread, run_coding_agent) can record which tool call
                // triggered the spawn — this becomes the new thread's
                // `spawning_event_id`.
                let tool_called_event_id = self
                    .event_bus
                    .emit_for_id(crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ToolCalled {
                            name: tool_call.name.clone(),
                            description: tool_desc,
                            args: redacted_args,
                        },
                        meta: meta.clone(),
                    })
                    .await;

                // Command guard (ADR 0002): classify bash/python commands before
                // dispatch. `Catastrophic` is hard-blocked; `IrreversibleDanger`
                // on a chat channel pauses and asks the user (this call blocks
                // in-process until the user resolves the card, the turn is
                // canceled, or a restart sweep resolves it). The refusal is
                // paired with this tool_use as a failed ToolResult below — the
                // same message-alternation guarantee the circuit breaker relies
                // on — so the model sees why it was refused and routes around it;
                // other tool calls in this response still run normally. No-op
                // unless the `command_guard` preference is on.
                let mut command_guard_ctx = crate::engine::command_permission::CommandGuardCtx {
                    enabled: command_guard_enabled,
                    judge_enabled: command_guard_judge_enabled,
                    judge_model: &command_judge_model,
                    judge_cache: &mut command_guard_judge_cache,
                    trigger_grant: trigger_side_effect_grant,
                };
                // `await_event`: registers a subscription and returns, like any
                // other tool (ADR 0047). It is handled here rather than in
                // `handle_special_tool` only because it needs the thread id and
                // the raw `tool_use` id to record the wait against.
                //
                // It used to END the turn, leaving this `ToolCalled` unpaired so
                // a delivered event could fill it in later. That shape is gone:
                // an unpaired `tool_use` is a provider 400 the moment anything
                // else runs on the thread, and every mechanism that existed to
                // keep one alive (detach-on-interruption, the attachment probe,
                // two anchor kinds, a restart guard) existed only to pay for
                // it. A delivery is now an ordinary new turn, so a subscribed
                // thread is simply idle.
                if tool_call.name == tn::AWAIT_EVENT {
                    let (result, success) = match self
                        .register_event_wait(thread_id, &tool_call.id, &tool_call.arguments)
                        .await
                    {
                        crate::engine::event_wait::AwaitEventOutcome::Registered(msg) => {
                            (msg, true)
                        }
                        // The model reads the refusal and can act on it in this
                        // same turn, which is the entire advantage `await_event`
                        // has over a trigger's silent footgun.
                        crate::engine::event_wait::AwaitEventOutcome::Refused(msg) => {
                            last_call_was_error = true;
                            had_errors = true;
                            (msg, false)
                        }
                    };
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::ToolResult {
                                    name: tool_call.name.clone(),
                                    result: result.clone(),
                                    images: vec![],
                                    success,
                                    tool_called_event_id,
                                },
                                meta: meta.clone(),
                            },
                            "[AgenticLoop] ToolResult (await_event)",
                        )
                        .await;
                    tool_outputs.push((tool_call.id.clone(), result));
                    continue;
                }

                // The other two verbs on this thread's own subscriptions. Here
                // for the same reason `await_event` is: both are scoped to the
                // calling thread, and the thread id is what `handle_special_tool`
                // does not have. Neither takes a thread argument, so an agent
                // cannot reach another thread's subscriptions through them.
                if tool_call.name == tn::LIST_EVENT_WAITS || tool_call.name == tn::CANCEL_EVENT_WAIT
                {
                    let (result, success) = if tool_call.name == tn::LIST_EVENT_WAITS {
                        (self.list_event_waits_text(thread_id).await, true)
                    } else {
                        let wait_id = tool_call
                            .arguments
                            .get("wait_id")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty());
                        let on = tool_call.arguments.get("on").and_then(|v| v.as_str());
                        let all = tool_call
                            .arguments
                            .get("all")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        // A malformed id is refused rather than read as "no id
                        // given", which with `all` unset would have reported the
                        // generic "pass one of the two" and hidden the typo.
                        match wait_id.map(uuid::Uuid::parse_str) {
                            Some(Err(e)) => (
                                format!(
                                    "Error: `wait_id` is not a valid id ({e}). Call \
                                     list_event_waits for the ids of what this thread is \
                                     actually watching."
                                ),
                                false,
                            ),
                            parsed => {
                                let id = parsed.and_then(Result::ok);
                                match self
                                    .cancel_event_waits_for_agent(thread_id, id, on, all)
                                    .await
                                {
                                    crate::engine::event_wait::CancelEventWaitOutcome::Stopped(
                                        msg,
                                    ) => (msg, true),
                                    crate::engine::event_wait::CancelEventWaitOutcome::Refused(
                                        msg,
                                    ) => (msg, false),
                                }
                            }
                        }
                    };
                    if !success {
                        last_call_was_error = true;
                        had_errors = true;
                    }
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::ToolResult {
                                    name: tool_call.name.clone(),
                                    result: result.clone(),
                                    images: vec![],
                                    success,
                                    tool_called_event_id,
                                },
                                meta: meta.clone(),
                            },
                            "[AgenticLoop] ToolResult (event-wait agent surface)",
                        )
                        .await;
                    tool_outputs.push((tool_call.id.clone(), result));
                    continue;
                }

                // Set when the guard took a checkpoint before this call (ADR
                // 0002, Phase 4). The bracket is closed right after the outcome
                // lands, below: only then is it known what the command changed.
                let mut pending_checkpoint: Option<
                    crate::engine::command_guard::PendingCheckpoint,
                > = None;
                let outcome: crate::engine::tools::ToolOutcome = match self
                    .command_guard_decision(
                        &mut command_guard_ctx,
                        &tool_call.name,
                        &tool_call.arguments,
                        thread_id,
                        &tool_call.id,
                        &meta,
                        cancel_token,
                    )
                    .await
                {
                    crate::engine::command_guard::GuardDecision::Refuse(refusal) => Err(refusal),
                    // Trigger blocked by an ungranted side-effect: record the
                    // block as a failed ToolResult (below), then break out and
                    // fail the whole trigger run after this batch.
                    crate::engine::command_guard::GuardDecision::FailTrigger(reason) => {
                        trigger_fail_reason = Some(reason.clone());
                        Err(reason)
                    }
                    decision @ (crate::engine::command_guard::GuardDecision::Proceed
                    | crate::engine::command_guard::GuardDecision::ProceedCheckpointed(_)) => {
                        if let crate::engine::command_guard::GuardDecision::ProceedCheckpointed(
                            pending,
                        ) = decision
                        {
                            pending_checkpoint = Some(pending);
                        }
                        if let Some(r) = self
                            .handle_special_tool(
                                &tool_call.name,
                                &tool_call.arguments,
                                thread_id,
                                user_images,
                                device_id,
                                tool_called_event_id,
                                &tool_call.id,
                                &meta,
                                cancel_token,
                            )
                            .await
                        {
                            // handle_special_tool still returns `String`; lift via
                            // the legacy `Error:` prefix until its sites are
                            // migrated to typed `Err`.
                            crate::engine::tools::lift_legacy_string(r)
                        } else {
                            // run_python / run_bash / http_request / mcp__* and
                            // friends dispatch through here. These are the tools
                            // that can park for minutes on a hung subprocess or a
                            // no-timeout reqwest client, so the cancel-aware
                            // wrapper is mandatory: dropping the inner future on
                            // cancel SIGKILLs the OS child (via `kill_on_drop(true)`
                            // on the Command) and lets the outer loop iterate to
                            // its pre-iter `is_cancelled()` check, which emits
                            // ResponseCanceled. Without the wrapper, a
                            // `urllib.request.urlopen()` with no timeout ignored
                            // cancel forever and the thread stayed `running` until
                            // the engine restarted.
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
                        }
                    }
                };
                let (result, is_error) = match outcome {
                    Ok(text) => (text, false),
                    Err(text) => (text, true),
                };

                // Close the checkpoint bracket. Deliberately outside the
                // success test: a command that failed or was cancelled midway
                // is exactly the one whose partial destruction the user wants
                // back, and the post image is what says how far it got.
                if let Some(pending) = pending_checkpoint.take() {
                    self.finalize_command_checkpoint(pending, thread_id, &meta)
                        .await;
                }

                // Carry this single call's outcome to the next iteration so the
                // generic breaker's failure streak only grows on repeated
                // *failures*. Only meaningful when the turn had one tool call —
                // the multi-call `else` branch above already clears the streak.
                if response.tool_calls.len() == 1 {
                    last_call_was_error = is_error;
                }

                // A failed ask_user_question (empty `question`, empty batch, or
                // non-array `questions`) must force a re-ask next iteration
                // rather than letting the model degrade to prose — record it.
                if tool_call.name == tn::ASK_USER_QUESTION && is_error {
                    this_iter_question_ask_failed = true;
                }

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

                // Split the raw result into what the model sees and what gets
                // persisted. They diverge for the image sentinels: the base64
                // has to survive to `build_tool_result_blocks` below, which is
                // what lifts it into a vision block, while the event stores a
                // stub instead of megabytes the frontend never reads. Reading
                // both off one value blinded the model and bloated the events
                // table at the same time.
                let mut split = split_tool_result(&result);

                // Front-end confirm-flow sentinels (credentials, plugin install,
                // plugin uninstall, email confirm): emit the transient
                // ThreadEvent that drives the panel/modal, and — for sentinels
                // whose raw JSON would mislead the LLM (install/uninstall let
                // it parse `overwrites` and chat-ask, see git history) —
                // replace tool_result_text so the model only sees a one-line
                // wait notice. EmailConfirm passes through unredacted because
                // its tool description already explains the modal flow.
                let sentinel_event = match_sentinel(split.event_text()).map(|m| {
                    if let Some(redacted) = m.redacted_text {
                        split.redact(redacted);
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
                                result: crate::core::sanitize_for_jsonb(split.event_text()),
                                images: std::mem::take(&mut split.images),
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
                        tool_calls_made,
                        max_tool_calls,
                        result
                    );
                } else {
                    log!(
                        "[AgentLoop] Step {}/{}: Success",
                        tool_calls_made,
                        max_tool_calls
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

                tool_outputs.push((tool_call.id.clone(), split.llm_text));

                // A trigger hit an ungranted side-effect: its block is now
                // recorded as a failed ToolResult — stop running the rest of
                // this batch and fail the trigger below.
                if trigger_fail_reason.is_some() {
                    break;
                }
            }

            // Carry this iteration's failed-ask signal into the next iteration
            // so its no-tool-calls branch can force a re-ask. Reassigned every
            // tool-call iteration, so it self-clears once an ask succeeds (or no
            // ask was attempted).
            question_ask_failed_last_iter = this_iter_question_ask_failed;

            // A trigger's command was blocked by an ungranted side-effect (ADR
            // 0002, Phase 5). Emit a terminal `ResponseFailed` and return `Err`
            // so the scheduler's failure-notification path surfaces it. The
            // blocked command's failed ToolResult was already persisted above,
            // so the transcript stays consistent; we don't append the assistant
            // message or call the LLM again — the run ends here.
            if let Some(reason) = trigger_fail_reason {
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                error: reason.clone(),
                            },
                            meta: meta.clone(),
                        },
                        "[AgenticLoop] ResponseFailed (trigger side-effect not granted)",
                    )
                    .await;
                *terminator_settled = true;
                return Err(reason.into());
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

            let result_blocks = build_tool_result_blocks(&tool_outputs, instruction);

            // Pin before the move: an image the model explicitly asked to see
            // must stay in vision for the rest of the turn, not just until its
            // next tool call.
            let pin_images = holds_explicitly_requested_image(&result_blocks);
            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Blocks(result_blocks),
            });
            if pin_images {
                push_explicit_image_pin(&mut explicit_image_idxs, messages.len() - 1);
            }

            // Check for injected prompts (mid-flight user corrections or system events).
            // Drain all pending injections and add them as user messages before the next LLM call.
            {
                let mut injected_prompts: Vec<super::super::InjectedPrompt> = Vec::new();
                while let Ok(prompt) = injection_rx.try_recv() {
                    injected_prompts.push(prompt);
                }
                self.note_injections_drained(
                    thread_id,
                    injection_generation,
                    injected_prompts.len(),
                );
                let injected_prompts =
                    filter_removed_queued_prompts(&self.pool, thread_id, injected_prompts).await;
                let appended = append_injected_prompts_to_messages(
                    &self.event_bus,
                    thread_id,
                    &meta,
                    messages,
                    injected_prompts,
                )
                .await;
                user_image_idxs.extend(appended.image_message_idxs);
                if appended.appended {
                    user_message_idx = messages.len().saturating_sub(1);
                }
            }

            // After the first LLM call, emit the derived `ImageDescribed` fact(s)
            // for any attached images. The actual image bytes STAY in the user
            // message (preserved by trim's image pins) so the model can
            // still see the image after intervening tool calls — the description
            // is only recorded as a past-tense fact, never swapped in for the bytes.
            if rounds == 1 {
                self.emit_image_descriptions_after_first_llm_call(
                    &mut image_description_handle,
                    origin_id,
                    thread_id,
                    &meta,
                )
                .await;
            }
        }
    }
}
