use crate::engine::agentic_loop::helpers::FinalAnswerDecisionExt;
use crate::engine::agentic_loop::helpers::HelpersExt;
use crate::engine::agentic_loop::special_tool::SpecialToolDispatchExt;
use crate::engine::chat::process::ChatProcessExt;
use crate::engine::failure_terminator::ensure_failure_terminator_emitted;
use crate::engine::inline_question_repair::InlineQuestionLeak;
use crate::engine::tools::todo::TodoExt;
use crate::engine::tools::ToolDispatchExt;
use crate::engine::turn_end::read_decision::TurnEndDecisionExt;
use crate::llm::provider::LlmResponse;
use crate::llm::tool_names as tn;
use crate::llm::{ContentBlock, Message, MessageContent};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::super::chat::process::working_understanding as wu;
use super::super::context::{
    estimate_message_chars, estimate_tokens_from_chars, trim_context_if_needed, RecoveryClause,
    TrimGuards,
};
use super::super::types::*;
use super::super::LucidosEngine;
use super::helpers::*;

#[easy_ext::ext(AgenticLoopRunExt)]
pub(crate) impl LucidosEngine {
    /// INVARIANT: After the first LLM call, emit the derived `ImageDescribed` fact(s) for
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

    async fn refresh_apps_modified_this_turn(
        &self,
        thread_id: Uuid,
        modified_app_uis: &std::collections::HashSet<String>,
    ) {
        for app_id in modified_app_uis {
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
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_agentic_loop(
        &self,
        messages: &mut Vec<Message>,
        system_prompt: &str,
        mut tools: TurnTools,
        request_id: Uuid,
        thread_id: Uuid,
        response_channel: Option<crate::engine::thread_events::EventChannel>,
        human_can_answer: bool,
        mut message_budget: usize,
        extraction_ctx: &str,
        mut image_description_handle: Option<tokio::task::JoinHandle<Option<(String, String)>>>,
        origin_id: Uuid,
        proposed_change: &mut bool,
        user_images: Option<&[crate::engine::types::ChatImage]>,
        device_id: Option<&str>,
        selection: crate::llm::ModelSelection<'_>,
        cancel_token: &CancellationToken,
        injection_rx: &mut mpsc::UnboundedReceiver<super::super::InjectedPrompt>,
        // INVARIANT: This turn's registration generation, so a drain reported after a
        // force-evict can't decrement a newer turn's unread count. Travels
        // with `injection_rx` — the two describe the same registration.
        injection_generation: u64,
        // INVARIANT: Set to true once this turn's ENDING is accounted for, so the
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
        // INVARIANT: This turn's resolved per-turn tool-call cap (the `max_tool_calls`
        // preference, defaulting to its catalog default). Passed in rather
        // than read here so the caller reads it ONCE and hands the same number
        // to the system-prompt builder: the prompt states this cap to the model,
        // and a prompt that names a different number than the loop enforces is
        // exactly the kind of fabricated engine internal the prompt warns
        // against. Passing it also freezes the cap for the turn, so a mid-turn
        // Settings change cannot move it under a running loop.
        max_tool_calls: usize,
        curated: crate::engine::chat::process::context_mode::CuratedTurn,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        let meta = crate::engine::thread_events::EventMeta {
            request_event_id: Some(origin_id),
            channel: response_channel,
            ..crate::engine::thread_events::EventMeta::NONE
        };

        // INVARIANT: Pin ONE provider Arc for this whole response — a mid-response runtime
        // swap (credential added/removed) must not change the in-flight provider.
        let provider = self.current_provider();
        let model_str = selection.model.unwrap_or(provider.default_model());
        let effective_model = (!model_str.is_empty()).then(|| model_str.to_string());
        let effective_effort = selection.reasoning_effort.map(|s| s.to_string());
        let caller_route = crate::core::ResolvedModelSelection {
            model: effective_model.clone(),
            reasoning_effort: effective_effort.clone(),
            provider: selection.provider.map(|kind| kind.as_str().to_string()),
        };
        let capture_window = self.context_window_for(capture_seed.model, selection.provider);

        let command_guard_enabled = crate::core::prefs::COMMAND_GUARD
            .try_read(&self.pool)
            .await?;
        let command_guard_judge_enabled = command_guard_enabled
            && crate::core::prefs::COMMAND_GUARD_JUDGE
                .try_read(&self.pool)
                .await?;
        // INVARIANT: An on judge with no model refuses the turn before any tool can run.
        // It never judges on the chat model or the static lists in silence.
        if let Some(error) = crate::engine::aux_purpose::guarded_turn_refusal(
            &self.pool,
            &self.model_registry,
            provider.as_ref(),
            command_guard_judge_enabled,
        )
        .await
        {
            self.event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                            error: error.clone(),
                            spend_cap: None,
                            agent_exit: None,
                            provider_error: None,
                        },
                        meta: meta.clone(),
                    },
                    "[AgenticLoop] ResponseFailed (no command judge model)",
                )
                .await;
            *terminator_settled = true;
            return Err(error.into());
        }
        let mut command_guard_judge_cache: std::collections::HashMap<
            String,
            crate::engine::command_guard::JudgedClassification,
        > = std::collections::HashMap::new();

        let mut rounds = 0;
        let mut tool_calls_made = 0usize;
        // INVARIANT: Capture before the loop pushes assistant/tool messages and shifts the index.
        // Maintained across rounds: the trimmer's removal pass may drop older
        // messages, which shifts the captured index down by the number removed
        // (handled below).
        let mut user_message_idx = messages.len().saturating_sub(1);
        // INVARIANT: Messages whose image bytes must survive trim pass 0 for the whole turn.
        // Split by provenance because only one side needs a bound:
        //
        // `user_image_idxs` — the turn's user message plus any mid-turn injection
        // or question answer that carried images. The user's own content;
        // bounded by how many messages they send in one turn, so it needs no cap.
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
        let mut images: Vec<String> = Vec::new();
        let mut last_tool_call: Option<(String, String)> = None;
        let mut consecutive_same_call = 0;
        let mut consecutive_failing_call = 0usize;
        let mut last_call_was_error = false;
        let mut question_ask_failed_last_iter = false;
        let mut question_reask_forced = 0usize;
        let mut question_prose_nudges_forced = 0usize;
        let mut prose_nudge_draft: Option<String> = None;
        let mut todo_wake_nudges_forced = 0usize;
        let mut read_decided = false;
        let mut cached_list_files: Option<String> = None;
        let mut modified_app_uis: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut capture_sections: Vec<crate::engine::ContextSection> =
            capture_seed.sections.to_vec();
        let system_chars: usize = capture_seed
            .sections
            .iter()
            .find(|s| s.name == "System Instructions")
            .map(|s| s.budget_delta_chars)
            .unwrap_or(0);
        let mut held_open: std::collections::HashSet<String> = std::collections::HashSet::new();
        // INVARIANT: A failed action is never cut by the trimmer. Manus found that
        // leaving mistakes in context is what stops a model repeating them.
        let mut failed: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut panel_first_seen: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let schedule = curated.schedule;
        let mode_on = curated.mode.is_on();
        let mut live_document = curated.document.clone();
        let mut live_todo = curated.todo.clone();
        let live_todo_notes = curated.todo_notes.clone();
        let mut notices = wu::RoundNotices::default();
        let budget_with_no_tools = message_budget + tools.defs_chars();

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

            // INVARIANT: Two backstops, because they catch different runaways.
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
                self.gate_chat_turn_end(
                    thread_id,
                    origin_id,
                    read_decided,
                    &cap_message,
                    crate::engine::turn_end::TurnFinish::CutShort,
                )
                .await;
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

            let mut mcp_correction: Option<String> = None;

            if tools.mcp_generation() != self.mcp_manager.tool_surface_generation() {
                let before = tools.defs().len();
                let before_servers = tools.mcp_server_ids().clone();
                let before_notice = tools.mcp_dropped_notice().map(str::to_owned);
                let changed = tools.refresh_mcp(
                    self.mcp_manager
                        .tool_surface(thread_id, tools.mcp_char_ceiling())
                        .await,
                );
                mcp_correction = [
                    mcp_surface_correction(&before_servers, tools.mcp_server_ids()),
                    mcp_dropped_correction(before_notice.as_deref(), tools.mcp_dropped_notice()),
                ]
                .into_iter()
                .flatten()
                .reduce(|servers, dropped| format!("{servers}\n{dropped}"));
                message_budget = budget_with_no_tools.saturating_sub(tools.defs_chars());
                if changed {
                    log_legacy!(
                        "[AgenticLoop] thread={} round={} MCP tools changed mid-turn: {} to {} tools in the array",
                        thread_id,
                        rounds,
                        before,
                        tools.defs().len()
                    );
                }
            }

            // INVARIANT: Pin the current turn's user message so removal cannot drop it
            // (`Some(user_message_idx)`), and keep every tracked image-bearing
            // message's bytes for the whole turn (`keep_image_idxs`). The
            // recent-tail rule alone fails once enough tool rounds shift a
            // message out of the last PRESERVE_RECENT_MESSAGES slots; losing it
            // strips the request line from every subsequent call (model reports
            // "I lost track of what you asked"), and stripping its image blinds
            // the model to what it was looking at.
            // What no STUBBING pass may cut: every failed action, and nothing
            // else. A keep is deliberately absent, so the wall can always cut
            // and a keep can never wedge the model's own turn.
            let protected: crate::engine::context::ProtectedAddresses = failed.clone();

            // INVARIANT: The sweep, before the trim so the backstop sees the smaller
            // array. Ages are noted first, so a result arriving this round is
            // recorded as arriving now and survives to be read.
            if mode_on {
                use crate::engine::chat::process::context_panel as panel;
                let sweeping = schedule.is_sweep_round(rounds);
                // INVARIANT: Last round's panel and document go first. The trimmer then
                // never counts ~10 KB it is about to lose anyway, and never
                // evicts a pair of real history for it. The model's own spans
                // fold ONLY on a sweep round: the penultimate cache breakpoint
                // sits on last round's assistant message.
                panel::collapse_tail_blocks(messages, sweeping);
                panel::note_first_seen(&mut panel_first_seen, messages, rounds);
                let sweep = crate::engine::chat::process::context_mode::sweep_expired_pairs(
                    messages,
                    &panel_first_seen,
                    rounds,
                    schedule,
                );
                if !sweep.removed.is_empty() {
                    log_legacy!(
                        "[Context] swept {} pair(s) and {} message(s) on round {} of thread {}",
                        sweep.removed.len(),
                        sweep.messages_dropped,
                        rounds,
                        thread_id
                    );
                }
                // INVARIANT: Position-aware, because a sweep cuts mid-array. The removal
                // bookkeeping below assumes contiguous removal from index 1,
                // which this is not.
                user_message_idx = sweep.remap(user_message_idx);
                for idx in user_image_idxs
                    .iter_mut()
                    .chain(explicit_image_idxs.iter_mut())
                {
                    *idx = sweep.remap(*idx);
                }
            }
            // INVARIANT: Built AFTER the sweep, because it is a COPY of the two pin lists
            // and the sweep remaps them. Built before, every index in it is one
            // too high once a message goes, and pass 0 strips the user's image.
            let mut keep_image_idxs = user_image_idxs.clone();
            keep_image_idxs.extend_from_slice(&explicit_image_idxs);
            let render_tail_blocks = |messages: &[Message]| -> (String, String) {
                use crate::engine::chat::process::context_panel as panel;
                let items = panel::tool_result_items(messages, &panel_first_seen, rounds, schedule);
                let fixed_chars = system_chars + tools.defs_chars();
                let view = panel::PanelView {
                    items: &items,
                    fixed: panel::FixedRegions {
                        system_chars,
                        tool_defs_chars: tools.defs_chars(),
                    },
                    held_open: &held_open,
                    budget_chars: message_budget + fixed_chars,
                    round: rounds,
                    schedule,
                };
                let document_block = wu::render(&live_document, &live_todo, &notices);
                let rendered = view.render(
                    messages.iter().map(estimate_message_chars).sum::<usize>()
                        + fixed_chars
                        + document_block.chars().count(),
                );
                (rendered, document_block)
            };
            let tail_reserve = if mode_on {
                let (panel, document) = render_tail_blocks(messages);
                (panel.chars().count() + document.chars().count()).min(message_budget / 2)
            } else {
                0
            };
            let trim_outcome = trim_context_if_needed(
                messages,
                message_budget - tail_reserve,
                Some(user_message_idx),
                &keep_image_idxs,
                TrimGuards {
                    protected: &protected,
                    held_open: &held_open,
                    recovery: if mode_on {
                        RecoveryClause::Omit
                    } else {
                        RecoveryClause::State
                    },
                },
            );
            let removed_count = trim_outcome.messages_removed;
            let trimmed = trim_outcome.any();
            let trim_passes: Vec<u8> = (0..8)
                .filter(|bit| trim_outcome.passes & (1 << bit) != 0)
                .collect();
            // INVARIANT: Removal takes oldest messages from index 1; the protected index
            // guard ensures every removal sits strictly below
            // user_message_idx, so it shifts down by removed_count.
            user_message_idx = user_message_idx.saturating_sub(removed_count);
            // INVARIANT: Every pin sits at or above the eviction floor (which is the
            // minimum over the pins), so none of them was removed. The uniform
            // shift stays exact and no pin can end up aliasing a different
            // message.
            for idx in user_image_idxs
                .iter_mut()
                .chain(explicit_image_idxs.iter_mut())
            {
                *idx = idx.saturating_sub(removed_count);
            }
            crate::llm::validate::validate_tool_use_pairing(messages);

            if mode_on {
                use crate::engine::chat::process::context_panel as panel;
                let (rendered, document_block) = render_tail_blocks(messages);
                capture_sections.retain(|section| {
                    section.name != panel::PANEL_SECTION && section.name != wu::SECTION
                });
                capture_sections.push(panel::panel_section(&rendered));
                capture_sections.push(wu::capture_section(&document_block));
                panel::append_to_tail(messages, rendered);
                panel::append_to_tail(messages, document_block);
                notices = wu::RoundNotices::default();
            }

            // INVARIANT: `messages[0]` still lists as stopped whatever was stopped at turn
            // setup, and rewriting it would forfeit the message-prefix cache.
            // One line in the tail is what keeps it from contradicting the
            // array this round carries. Outside the `mode_on` block, because
            // the disagreement is the same either way.
            if let Some(correction) = mcp_correction {
                crate::engine::chat::process::context_panel::append_to_tail(messages, correction);
            }

            // INVARIANT: Always measure AFTER trimming. Pass 0 strips images even when no
            // message is removed, so pre-trim chars can be wildly inflated.
            let context_chars: usize = messages.iter().map(estimate_message_chars).sum();

            let context_tokens = estimate_tokens_from_chars(context_chars);
            let context_messages = messages.len();
            let trimmed_str = if trimmed { " (trimmed)" } else { "" };
            let holding_back = prose_nudge_draft.is_some();
            let context_summary = crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::ThoughtStreamed {
                    text: format!(
                        "Context: {} tokens, {} messages{}",
                        context_tokens, context_messages, trimmed_str
                    ),
                },
                meta: meta.clone(),
            };
            let deferred_summary = if holding_back {
                Some(context_summary)
            } else {
                self.event_bus
                    .emit_or_log(
                        context_summary,
                        "[AgenticLoop] ThoughtStreamed (context summary)",
                    )
                    .await;
                None
            };

            let raw_buffer = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

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
                    if holding_back
                        || crate::engine::inline_question_repair::buffer_contains_inline_tag(&text)
                        || crate::engine::inline_tool_call_repair::buffer_contains_inline_tool_call(
                            &text,
                        )
                        || (mode_on && text.contains(wu::MARKER_PREFIX))
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
                            depth: 0,
                            emitting_trigger_id: None,
                        });
                        let mut last = persisted_len.lock().unwrap();
                        if text.len() > *last {
                            let new_text = text[*last..].to_string();
                            let _ = persist.send(new_text);
                            *last = text.len();
                        }
                    }
                }) as crate::llm::TokenCallback)
            };

            let call_tools = tools.defs().to_vec();

            // INVARIANT: Read per round, never per turn: a question card can hold this
            // turn open for hours, and the user may flip the switch meanwhile.
            let capture_body = match crate::core::prefs::CAPTURE_CONTEXT
                .try_read(&self.pool)
                .await
            {
                Ok(on) => on,
                Err(e) => {
                    ensure_failure_terminator_emitted(
                        &self.event_bus,
                        &self.pool,
                        thread_id,
                        origin_id,
                        response_channel,
                        &e.to_string(),
                    )
                    .await;
                    *terminator_settled = true;
                    return Err(e.into());
                }
            };

            let llm_future = crate::llm::cache_probe::scope(
                crate::llm::cache_probe::ProbeCall {
                    thread_id,
                    turn_id: origin_id,
                    round: rounds,
                },
                crate::engine::model_call::turn_chat(
                    &self.event_bus,
                    thread_id,
                    meta.clone(),
                    rounds,
                    provider.as_ref(),
                    messages.clone(),
                    call_tools,
                    selection,
                    Some(system_prompt),
                    token_cb,
                ),
            );
            let cancel_future = cancel_token.cancelled();

            let (response, turn_capture) = tokio::select! {
                result = llm_future => {
                    match result {
                        Ok(answered) => answered,
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
                                        spend_cap: crate::llm::spend_cap::SpendCapNotice::from_error(e.as_ref()),
                                        agent_exit: None,
                                        provider_error: None,
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
                    let raw_partial = raw_buffer.lock().unwrap().clone();
                    // INVARIANT: A stopped round still wrote what it wrote. Persisting it
                    // here is what keeps the document, and the parse it returns
                    // is what lets the flush below carry prose rather than
                    // markup. `Truncated`, because the Stop is what ended the
                    // text: only a block the model closed before it is kept.
                    // The error arm deliberately persists nothing at all, since
                    // an `Err` can also cut a block short of its marker.
                    let partial = if mode_on {
                        let (parsed, _) = read_working_understanding(
                            &self.event_bus,
                            thread_id,
                            &raw_partial,
                            wu::ReplyEnd::Truncated,
                            &mut live_document,
                            &mut live_todo,
                            &live_todo_notes,
                        )
                        .await;
                        wu::strip_faulted_markup(&wu::splice_spans_out(&raw_partial, &parsed.spans))
                    } else {
                        raw_partial
                    };
                    if !holding_back {
                        let last = *last_persisted_len.lock().unwrap();
                        if let Some(remaining) = partial.get(last..).filter(|r| !r.is_empty()) {
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
                    let partial = prose_nudge_draft.take().unwrap_or(partial);
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
            self.remembered_requests.remember(
                thread_id,
                response.prefill.clone(),
                std::time::Instant::now(),
            );

            let static_total: usize = capture_sections.iter().map(|s| s.budget_delta_chars).sum();
            let bundled_total = static_total.saturating_sub(system_chars);
            let conversation = conversation_section(messages, bundled_total, context_chars);
            let estimated_total_tokens: usize =
                estimate_tokens_from_chars(system_chars + tools.defs_chars() + context_chars);
            let tool_definitions = crate::engine::ContextSection {
                name: format!("Tool Definitions ({})", tools.names().len()),
                content: None,
                budget_delta_chars: tools.defs_chars(),
                content_chars: Some(tools.defs_chars()),
                role: crate::engine::ContextRole::System,
                group: None,
            };
            let iter_sections = round_capture_sections(
                &capture_sections,
                [tool_definitions, conversation],
                capture_body,
            );
            let usage = turn_capture.usage();
            if let Some(u) = usage {
                let counted_chars = system_chars + tools.defs_chars() + context_chars;
                log_legacy!(
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

            // INVARIANT: Post-response repair: the model sometimes emits
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
                    crate::log_legacy!(
                        "[InlineQuestionRepair] thread={} synthesised tool call from leaked ask_user_question payload (synth_id={})",
                        thread_id,
                        synth_id,
                    );
                }
                Some(InlineQuestionLeak::Degenerate { .. }) => {
                    crate::log_legacy!(
                        "[InlineQuestionRepair] thread={} stripped a leaked <ask_user_question> tag with no dispatchable payload, forcing a re-ask",
                        thread_id,
                    );
                }
                None => {}
            }
            let question_leaked_as_text =
                matches!(inline_repair, Some(InlineQuestionLeak::Degenerate { .. }));
            let tool_call_repair = if response.tool_calls.is_empty() {
                response.content.as_deref().and_then(|c| {
                    crate::engine::inline_tool_call_repair::detect_inline_tool_call(c, tools.defs())
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
                crate::log_legacy!(
                    "[InlineToolCallRepair] thread={} synthesised '{}' tool call from inline <invoke> tag (synth_id={})",
                    thread_id,
                    d.name,
                    synth_id,
                );
            }

            if let Some(summary) = deferred_summary.filter(|_| !response.tool_calls.is_empty()) {
                self.event_bus
                    .emit_or_log(summary, "[AgenticLoop] ThoughtStreamed (held-back round)")
                    .await;
            }
            turn_capture
                .finish(
                    |report| crate::engine::thread_events::ThreadEvent::ContextCaptured {
                        producer: crate::engine::ContextProducer::MainLlm,
                        model: capture_seed.model.to_string(),
                        context_window: capture_window,
                        sections: iter_sections,
                        tools: tools.names().to_vec(),
                        estimated_total_tokens,
                        usage: report.usage,
                        trimmed,
                        trim_passes: trim_passes.clone(),
                        purpose: crate::engine::ContextPurpose::Turn,
                        reconstructed: false,
                        parent_tool_use_id: None,
                        api_call_id: None,
                        reasoning_effort: report.detail.reasoning_effort,
                        duration_ms: report.detail.duration_ms,
                        served_model: report.detail.served_model,
                        provider: report.detail.provider,
                        stable_prefix_fingerprint: report.stable_prefix_fingerprint,
                        ttft_ms: report.detail.ttft_ms,
                        request_id: report.detail.request_id,
                        telemetry_owned_by_cli: false,
                        session: None,
                    },
                )
                .await;

            // INVARIANT: Post-response repair (argument text): the model sometimes
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
                    crate::log_legacy!(
                        "[ToolArgEntityRepair] thread={} decoded HTML entities in '{}' arguments",
                        thread_id,
                        tool_call.name,
                    );
                }
            }
            // INVARIANT: The working understanding, read the moment the reply lands. It
            // arranges the NEXT round, so a keep written now is in force before
            // the pass that drops pairs. That is the ordering the tool call
            // used to guarantee by running earlier.
            //
            // The raw text is kept for history. The span stays in the message
            // array, where the sweep folds it, and is spliced out of everything
            // the user reads.
            //
            // ONE accessor for both, and it is `history_text`, never
            // `content`. Gemini narrates its plan in ordinary text beside a
            // `functionCall` and keeps that off the screen by leaving `content`
            // empty. Reading `content` would ignore every document and every
            // keep that model writes on a tool-call round.
            let raw_history_text: Option<String> = response.history_text().map(str::to_string);
            let (parsed, applied) = if mode_on {
                read_working_understanding(
                    &self.event_bus,
                    thread_id,
                    raw_history_text.as_deref().unwrap_or(""),
                    wu::ReplyEnd::Complete,
                    &mut live_document,
                    &mut live_todo,
                    &live_todo_notes,
                )
                .await
            } else {
                (wu::ParsedReply::default(), wu::Applied::default())
            };
            let wrote_document = parsed.wrote_something();
            if wrote_document || !parsed.faults.is_empty() {
                notices.faults = applied.faults;
                // INVARIANT: A keep is consumed too, so a later rewrite cannot re-assert
                // one. It OVERWRITES the first-seen round, which is the whole
                // mechanism: `note_first_seen` never would.
                //
                for address in applied.keep_open {
                    if !crate::engine::chat::process::context_panel::carries_address(
                        messages, &address,
                    ) {
                        notices.faults.push(format!(
                            "`{address}` names nothing your context is carrying, so nothing was \
                             held open."
                        ));
                        continue;
                    }
                    crate::engine::chat::process::context_panel::hold_open(
                        &mut panel_first_seen,
                        &address,
                        rounds,
                    );
                    held_open.insert(address.clone());
                    notices.held_open.push(address.clone());
                    if let Some(id) = crate::core::store::parse_event_address(&address) {
                        self.event_bus
                            .emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event:
                                        crate::engine::thread_events::ThreadEvent::ContextKeptOpen {
                                            kept_open_event_id: id,
                                        },
                                    meta: crate::engine::thread_events::EventMeta::NONE,
                                },
                                "[AgenticLoop] ContextKeptOpen",
                            )
                            .await;
                    }
                }
            }
            // INVARIANT: The splice runs over `content`, which is the half the user reads.
            // The span offsets came from `history_text`, and the two are the
            // same string whenever `content` is set: `model_only_text` is never
            // set beside it. Where `content` is empty nothing reaches the user
            // anyway, so there is nothing to splice.
            //
            // A fault runs it too. An opening the parse could not read leaves no
            // span, so the splice alone would hand the whole block to the user.
            let spliced_something = wrote_document || !parsed.faults.is_empty();
            if let Some(raw) = response.content.as_deref().filter(|_| spliced_something) {
                let cleaned = wu::strip_faulted_markup(&wu::splice_spans_out(raw, &parsed.spans));
                response.content = (!cleaned.is_empty()).then_some(cleaned);
            }

            let kept_draft = prose_nudge_draft.take().filter(|_| {
                response.tool_calls.is_empty()
                    || calls_only_turn_end_decisions(&response.tool_calls)
            });
            if let Some(draft) = &kept_draft {
                log_legacy!(
                    "[AgenticLoop] thread={} kept its prose question after the nudge: {}",
                    thread_id,
                    response.content.as_deref().unwrap_or("").trim()
                );
                response.content = Some(draft.clone());
            }

            let (flush_text, remaining_to_persist) = if kept_draft.is_some() {
                (None, None)
            } else {
                let raw = raw_buffer.lock().unwrap();
                let cleaned_override =
                    (inline_repair.is_some() || tool_call_repair.is_some() || spliced_something)
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
                let remaining = effective
                    .get(last..)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                (cloned, remaining)
            };
            // INVARIANT: Known here rather than read back: the background persist task
            // may not have written this round's deltas yet.
            let round_text = flush_text.clone().unwrap_or_default();
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

            if response.tool_calls.is_empty() {
                // INVARIANT: A reply declined partway ends the turn here, before any path
                // below can send the refused text back to the model. The text
                // already persisted as `TextStreamed` stays beside the failure.
                // The stop reason is checked first, so an ordinary turn does
                // not pay for a second `clean_response`. A held-back reply is
                // exempt: the user never saw it, and the turn ends on the draft.
                if let Some(error) =
                    declined_partway_error(response.stop_reason.as_deref()).filter(|_| {
                        kept_draft.is_none()
                            && response
                                .content
                                .as_deref()
                                .is_some_and(|c| !self.clean_response(c).is_empty())
                    })
                {
                    self.refresh_apps_modified_this_turn(thread_id, &modified_app_uis)
                        .await;
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                    error: error.to_string(),
                                    spend_cap: None,
                                    agent_exit: None,
                                    provider_error: None,
                                },
                                meta: meta.clone(),
                            },
                            "[AgenticLoop] ResponseFailed (declined partway)",
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

                let answer = response
                    .content
                    .as_deref()
                    .map(|c| self.clean_response(c))
                    .filter(|c| !c.is_empty());
                let reask = answer.and_then(|answer| {
                    question_reask_cause(QuestionReaskInputs {
                        ask_failed_last_iter: question_ask_failed_last_iter,
                        leaked_as_text: question_leaked_as_text,
                        ends_in_a_question: reply_ends_in_a_question(&answer),
                        human_can_answer,
                        reask_forced: question_reask_forced,
                        prose_nudges_forced: question_prose_nudges_forced,
                    })
                    .map(|cause| (cause, answer))
                });
                if let Some((cause, answer)) = reask {
                    match cause {
                        QuestionReaskCause::AskedInProse => {
                            question_prose_nudges_forced += 1;
                            prose_nudge_draft = Some(answer.clone());
                        }
                        QuestionReaskCause::CallRejected | QuestionReaskCause::LeakedAsText => {
                            question_reask_forced += 1
                        }
                    }
                    messages.push(Message {
                        role: "assistant".to_string(),
                        content: MessageContent::Text(answer),
                    });
                    messages.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Text(cause.instruction().to_string()),
                    });
                    let reason = cause.retry_reason();
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

                // INVARIANT: A user may have sent follow-ups while this LLM call was in
                // flight. If the model returned a plain final answer (no tool
                // loop), this is the last chance to ingest them before the
                // response terminates and they become orphaned follow-up turns.
                // Preserve the draft assistant text in history, append the
                // coalesced injection as one user message, and continue so the
                // agent answers the queued updates in this same turn.
                // A spoken aside alone is not a follow-up and buys no round:
                // `injections_reopen_a_finished_answer` says why.
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
                if injections_reopen_a_finished_answer(&injected_prompts) {
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
                        self.workspace_path(),
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
                } else if !injected_prompts.is_empty() {
                    log_legacy!(
                        "[AgenticLoop] thread={} {} spoken aside(s) landed on a finished answer, not reopening it",
                        thread_id,
                        injected_prompts.len()
                    );
                }

                let visible = wrote_document
                    .then(|| response.content.as_deref().map(|c| self.clean_response(c)))
                    .flatten();
                if reply_was_bookkeeping_alone(wrote_document, visible.as_deref()) {
                    // INVARIANT: The raw text, span included, so the fold has something to
                    // collapse on the next sweep.
                    if let Some(text) = raw_history_text.as_deref() {
                        messages.push(Message {
                            role: "assistant".to_string(),
                            content: MessageContent::Text(text.to_string()),
                        });
                    }
                    messages.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Text(NOTED_CARRY_ON.to_string()),
                    });
                    log_legacy!(
                        "[AgenticLoop] thread={} reply carried only the working understanding, continuing",
                        thread_id
                    );
                    continue;
                }

                let wake_nudge = if cancel_token.is_cancelled() {
                    None
                } else {
                    let (open, covered) = self.wake_check_facts(thread_id).await;
                    should_nudge_unwatched_turn(open, covered, todo_wake_nudges_forced)
                        .then(|| open.unwrap_or(0))
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
                    if !draft_in_history {
                        messages.push(Message {
                            role: "assistant".to_string(),
                            content: MessageContent::Text(draft),
                        });
                    }
                    messages.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Text(todo_wake_nudge_instruction(open, mode_on)),
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

                self.refresh_apps_modified_this_turn(thread_id, &modified_app_uis)
                    .await;
                let cleaned = response
                    .content
                    .as_deref()
                    .map(|c| self.clean_response(c))
                    .filter(|c| !c.is_empty());

                if let Some(clean_response) = cleaned {
                    self.gate_chat_turn_end(
                        thread_id,
                        origin_id,
                        read_decided,
                        &clean_response,
                        crate::engine::turn_end::TurnFinish::Finished(
                            crate::engine::thread_events::ChildCompletionStatus::Success,
                        ),
                    )
                    .await;
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
                                meta: meta.authored_by(
                                    crate::engine::thread_events::AgentParticipant::LucidosAgent,
                                ),
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

                let stop_reason = response.stop_reason.as_deref().unwrap_or("unknown");
                let reported_output = response.usage.map(|u| u.output_tokens);
                let output_tokens_n = reported_output.unwrap_or(0);
                let model_thought = response.thinking_chars.unwrap_or(0) > 0
                    || response.thinking_blocks.unwrap_or(0) > 0;
                let unknown_sse_dropped = response.unknown_sse_dropped;
                let output_tokens = reported_output
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let thinking_chars = response
                    .thinking_chars
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let class = classify_empty_completion(
                    stop_reason,
                    output_tokens_n,
                    model_thought,
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
                                    spend_cap: None,
                                    agent_exit: None,
                                    provider_error: None,
                                },
                                meta: meta.clone(),
                            },
                            "[AgenticLoop] ResponseFailed (empty completion)",
                        )
                        .await;
                } else {
                    crate::log_legacy!(
                        "[AgenticLoop] thread={} empty completion treated as benign ({})",
                        thread_id,
                        diagnostic
                    );
                    self.gate_chat_turn_end(
                        thread_id,
                        origin_id,
                        read_decided,
                        "",
                        crate::engine::turn_end::TurnFinish::Finished(
                            crate::engine::thread_events::ChildCompletionStatus::Success,
                        ),
                    )
                    .await;
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
                                meta: meta.authored_by(
                                    crate::engine::thread_events::AgentParticipant::LucidosAgent,
                                ),
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

            let mut tool_outputs: Vec<ToolOutput> = Vec::new();
            let mut had_errors = false;
            let mut this_iter_question_ask_failed = false;
            let mut trigger_fail_reason = None;
            let mut quiet_answer = false;
            let mut quiet_reply = String::new();

            let push_circuit_breaker =
                |messages: &mut Vec<Message>, response: &LlmResponse, result_text: &str| {
                    let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
                    // INVARIANT: The RAW reply, span included. `history_text` reads
                    // `content`, which the splice above has already cleaned for
                    // the user, and the array is where the fold collapses a
                    // superseded document.
                    if let Some(content_text) = raw_history_text.as_deref() {
                        assistant_blocks.push(ContentBlock::Text {
                            text: content_text.to_string(),
                        });
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

            if response.tool_calls.len() == 1 {
                let current_tool = &response.tool_calls[0].name;
                let current_args = &response.tool_calls[0].arguments;

                let call_key = derive_call_key(current_tool, current_args);

                let current_call = (current_tool.clone(), call_key.clone());
                let is_repeat = Some(&current_call) == last_tool_call.as_ref();
                if is_repeat {
                    consecutive_same_call += 1;
                } else {
                    consecutive_same_call = 1;
                    last_tool_call = Some(current_call);
                }
                consecutive_failing_call =
                    next_failure_streak(is_repeat, last_call_was_error, consecutive_failing_call);

                if current_tool == tn::LIST_FILES && consecutive_same_call >= 2 {
                    if let Some(ref cached) = cached_list_files {
                        log_legacy!(
                            "[AgentLoop] Returning cached list_files result (call #{})",
                            consecutive_same_call
                        );
                        if consecutive_same_call >= 4 {
                            log_legacy!(
                                "[AgentLoop] Force-breaking tool loop after {} repeated list_files attempts",
                                consecutive_same_call
                            );
                            let msg = "I listed the available files but wasn't able to complete the task. Could you give me more specific instructions?";
                            self.gate_chat_turn_end(
                                thread_id,
                                origin_id,
                                read_decided,
                                msg,
                                crate::engine::turn_end::TurnFinish::CutShort,
                            )
                            .await;
                            self.event_bus.emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                                        text: msg.to_string(), images: images.clone(), model: effective_model.clone(), reasoning_effort: effective_effort.clone(),
                                    },
                                    meta: meta.authored_by(crate::engine::thread_events::AgentParticipant::LucidosAgent),
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

                if current_tool == tn::READ_FILE && consecutive_same_call >= 3 {
                    let path_for_msg = current_args
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    log_legacy!(
                        "[AgentLoop] Blocking repeated read_file of '{}' (call #{})",
                        path_for_msg,
                        consecutive_same_call
                    );
                    if consecutive_same_call >= 5 {
                        log_legacy!(
                            "[AgentLoop] Force-breaking tool loop after {} repeated read_file attempts",
                            consecutive_same_call
                        );
                        let msg = format!("I read the file `{}` but wasn't able to complete the task with it. Could you give me more specific instructions?", path_for_msg);
                        self.gate_chat_turn_end(
                            thread_id,
                            origin_id,
                            read_decided,
                            &msg,
                            crate::engine::turn_end::TurnFinish::CutShort,
                        )
                        .await;
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
                                    meta: meta.authored_by(crate::engine::thread_events::AgentParticipant::LucidosAgent),
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

                let breaker_action = generic_breaker_action(consecutive_failing_call);
                if breaker_action != BreakerAction::None {
                    let target = if call_key.is_empty() {
                        current_tool.to_string()
                    } else {
                        call_key.clone()
                    };
                    if breaker_action == BreakerAction::Break {
                        log_legacy!(
                            "[AgentLoop] Force-breaking loop: {} repeated after errors (attempt {}) on '{}'",
                            current_tool,
                            consecutive_failing_call,
                            target
                        );
                        let msg = format!("I tried to process `{}` but it failed repeatedly. The error may need to be resolved before retrying.", target);
                        self.gate_chat_turn_end(
                            thread_id,
                            origin_id,
                            read_decided,
                            &msg,
                            crate::engine::turn_end::TurnFinish::CutShort,
                        )
                        .await;
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
                                    meta: meta.authored_by(crate::engine::thread_events::AgentParticipant::LucidosAgent),
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
                    // INVARIANT: Warn on attempts 3-4 without executing the call.
                    // Keep the previous error so another retry advances the streak.
                    log_legacy!(
                        "[AgentLoop] Skipping repeated failed call: {} (attempt {}) on '{}'",
                        current_tool,
                        consecutive_failing_call,
                        target
                    );
                    let stop_msg = format!(
                        "STOP: Your call to {} on '{}' failed earlier. This identical retry will not run. Use the results you already have, or resolve the underlying error before retrying.",
                        current_tool, target
                    );
                    push_circuit_breaker(messages, &response, &stop_msg);
                    continue;
                }
            } else {
                consecutive_same_call = 0;
                last_tool_call = None;
                consecutive_failing_call = 0;
                last_call_was_error = false;
            }

            let mut parallel_reads: std::collections::HashMap<usize, ParallelRead> =
                std::collections::HashMap::new();
            for (call_index, tool_call) in response.tool_calls.iter().enumerate() {
                tool_calls_made += 1;

                if !parallel_reads.contains_key(&call_index) {
                    let run_end = parallel_run_end(&response.tool_calls, call_index);
                    if run_end - call_index >= 2 {
                        let reads = self
                            .run_parallel_reads(
                                &response.tool_calls[call_index..run_end],
                                tool_calls_made,
                                max_tool_calls,
                                thread_id,
                                &meta,
                                extraction_ctx,
                                request_id,
                                device_id,
                                cancel_token,
                            )
                            .await;
                        parallel_reads.extend((call_index..run_end).zip(reads));
                    }
                }
                if let Some(read) = parallel_reads.remove(&call_index) {
                    if read.is_error {
                        had_errors = true;
                        note_failure(&mut failed, read.tool_called_event_id, curated.mode);
                        log_legacy!(
                            "[AgentLoop] Step {}/{}: Error, will retry: {}",
                            tool_calls_made,
                            max_tool_calls,
                            read.result
                        );
                    } else {
                        if tool_call.name == tn::LIST_FILES {
                            cached_list_files = Some(read.result);
                        }
                        log_legacy!(
                            "[AgentLoop] Step {}/{}: Success",
                            tool_calls_made,
                            max_tool_calls
                        );
                    }
                    tool_outputs.push(ToolOutput {
                        tool_use_id: tool_call.id.clone(),
                        text: read.llm_text,
                        event_id: read.tool_called_event_id,
                        images: Vec::new(),
                    });
                    continue;
                }

                let tool_called_event_id = self
                    .emit_tool_called(thread_id, &meta, tool_call, tool_calls_made, max_tool_calls)
                    .await;

                let card_refusal = if tool_call.name == tn::ASK_USER_QUESTION && human_can_answer {
                    crate::engine::question_card_gate::refuse_card(
                        &self.pool,
                        thread_id,
                        &tool_call.id,
                        tool_call
                            .arguments
                            .get("questions")
                            .unwrap_or(&serde_json::Value::Null),
                        crate::engine::agent_question::card_message(&tool_call.arguments),
                        Some(crate::engine::question_card_gate::ChatRound {
                            text: &round_text,
                            progress_notes: &response.progress_notes,
                        }),
                    )
                    .await
                } else {
                    None
                };
                if let Some(refusal) = card_refusal {
                    // INVARIANT: Not an error: flagging it would trip the CallRejected
                    // re-ask, which tells the model its `question` was empty.
                    if response.tool_calls.len() == 1 {
                        last_call_was_error = false;
                    }
                    let refusal = refusal.text();
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::ToolResult {
                                    name: tool_call.name.clone(),
                                    result: refusal.clone(),
                                    images: vec![],
                                    success: false,
                                    tool_called_event_id,
                                },
                                meta: meta.clone(),
                            },
                            "[AgenticLoop] ToolResult (question card refused)",
                        )
                        .await;
                    tool_outputs.push(ToolOutput {
                        tool_use_id: tool_call.id.clone(),
                        text: refusal,
                        event_id: tool_called_event_id,
                        images: Vec::new(),
                    });
                    continue;
                }

                // INVARIANT: The mode withdraws `todo_write` from the tools array, so a
                // call for it can only come from a cached prompt or a
                // hallucination. Refused rather than run, because the handler
                // writes `TodoListWritten` and never touches `live_todo`: the
                // list rendered back next round would be the old one, and the
                // model's next document write would then overwrite the
                // projection with it. One list, one write surface.
                if mode_on && tool_call.name == tn::TODO_WRITE {
                    last_call_was_error = true;
                    had_errors = true;
                    note_failure(&mut failed, tool_called_event_id, curated.mode);
                    let refusal = crate::engine::chat::process::context_mode::TODO_TOOL_REFUSAL;
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::ToolResult {
                                    name: tool_call.name.clone(),
                                    result: refusal.to_string(),
                                    images: vec![],
                                    success: false,
                                    tool_called_event_id,
                                },
                                meta: meta.clone(),
                            },
                            "[AgenticLoop] ToolResult (todo_write refused, context mode)",
                        )
                        .await;
                    tool_outputs.push(ToolOutput {
                        tool_use_id: tool_call.id.clone(),
                        text: refusal.to_string(),
                        event_id: tool_called_event_id,
                        images: Vec::new(),
                    });
                    continue;
                }

                let mut command_guard_ctx = crate::engine::command_permission::CommandGuardCtx {
                    enabled: command_guard_enabled,
                    judge_enabled: command_guard_judge_enabled,
                    judge_cache: &mut command_guard_judge_cache,
                };
                if tool_call.name == tn::AWAIT_EVENT {
                    let (result, success) = match self
                        .register_event_wait(thread_id, &tool_call.id, &tool_call.arguments)
                        .await
                    {
                        crate::engine::event_wait::AwaitEventOutcome::Registered(msg) => {
                            (msg, true)
                        }
                        crate::engine::event_wait::AwaitEventOutcome::Refused(msg) => {
                            last_call_was_error = true;
                            had_errors = true;
                            note_failure(&mut failed, tool_called_event_id, curated.mode);
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
                    tool_outputs.push(ToolOutput {
                        tool_use_id: tool_call.id.clone(),
                        text: result,
                        event_id: tool_called_event_id,
                        images: Vec::new(),
                    });
                    continue;
                }

                // INVARIANT: The other two verbs on this thread's own subscriptions. Here
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
                        note_failure(&mut failed, tool_called_event_id, curated.mode);
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
                    tool_outputs.push(ToolOutput {
                        tool_use_id: tool_call.id.clone(),
                        text: result,
                        event_id: tool_called_event_id,
                        images: Vec::new(),
                    });
                    continue;
                }

                let mut pending_checkpoint: Option<
                    crate::engine::command_guard::PendingCheckpoint,
                > = None;
                let execution_sandbox = self.resolve_tool_sandbox(&tool_call.name).await;
                let paths = execution_sandbox
                    .as_ref()
                    .ok()
                    .and_then(|s| s.as_ref())
                    .map(|s| {
                        crate::engine::command_guard::SecretPaths::for_sandbox(
                            s,
                            self.workspace_path(),
                        )
                    })
                    .unwrap_or_else(crate::engine::command_guard::SecretPaths::conservative);
                let open_question = if tool_call.name == tn::ASK_USER_QUESTION {
                    None
                } else {
                    crate::engine::cc_permission::open_question_refusal(&self.pool, thread_id).await
                };
                let decision = match (&execution_sandbox, open_question) {
                    (_, Some(reason)) => {
                        crate::engine::command_guard::GuardDecision::Refuse(reason.to_string())
                    }
                    (Err(reason), None) => {
                        crate::engine::command_guard::GuardDecision::Refuse(reason.clone())
                    }
                    (Ok(_), None) => {
                        self.command_guard_decision(
                            &mut command_guard_ctx,
                            &tool_call.name,
                            &tool_call.arguments,
                            thread_id,
                            &tool_call.id,
                            &meta,
                            cancel_token,
                            &paths,
                        )
                        .await
                    }
                };
                let mut trigger_sandbox = None;
                let outcome: crate::engine::tools::ToolOutcome = match decision
                {
                    crate::engine::command_guard::GuardDecision::Refuse(refusal) => Err(refusal),
                    crate::engine::command_guard::GuardDecision::FailTrigger(blocked) => {
                        let reason = blocked.reason.clone();
                        trigger_fail_reason = Some(blocked);
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
                        if tool_call.name == tn::ASK_USER_QUESTION {
                            let asked = self
                                .handle_chat_ask_user_question(
                                    thread_id,
                                    &tool_call.id,
                                    &tool_call.arguments,
                                    &meta,
                                )
                                .await;
                            quiet_answer |= asked.quiet;
                            if let Some(reply) = asked.quiet_reply {
                                quiet_reply = reply;
                            }
                            crate::engine::tools::lift_legacy_string(asked.result)
                        } else if tool_call.name == tn::REQUEST_READ {
                            run_tool_with_cancel(
                                async {
                                    let (recorded, ack) = self
                                        .decide_chat_agent_read(
                                            thread_id,
                                            origin_id,
                                            &tool_call.arguments,
                                        )
                                        .await?;
                                    read_decided = recorded;
                                    Ok(ack.to_string())
                                },
                                cancel_token,
                            )
                            .await
                        } else if let Some(r) = self
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
                                &caller_route,
                            )
                            .await
                        {
                            crate::engine::tools::lift_legacy_string(r)
                        } else {
                trigger_sandbox = match (
                    execution_sandbox.as_ref().ok().and_then(Option::as_ref),
                    meta.channel,
                    crate::scheduler::user_tasks::current_trigger_id(),
                ) {
                    (
                        Some(sandbox),
                        Some(crate::engine::thread_events::EventChannel::Trigger),
                        Some(id),
                    ) => Some(self.trigger_execution_sandbox(sandbox, &id)),
                    _ => None,
                };
                            // INVARIANT: run_python / run_bash / http_request / mcp__* and
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
                                self.execute_tool_in_sandbox(
                                    &tool_call.name,
                                    &tool_call.arguments,
                                    extraction_ctx,
                                    request_id,
                                    device_id,
                                    thread_id,
                                    trigger_sandbox.as_ref().or_else(|| execution_sandbox.as_ref().ok().and_then(Option::as_ref)),
                                ),
                                cancel_token,
                            )
                            .await
                        }
                    }
                };
                let refusal = trigger_sandbox.as_ref().and_then(|sandbox| {
                    crate::engine::command_guard::command_text(
                        &tool_call.name,
                        &tool_call.arguments,
                    )?;
                    let diagnostic = outcome.as_ref().err()?;
                    let folder = crate::triggers::folder_grant::folder_refusal(
                        sandbox,
                        self.workspace_path(),
                        diagnostic,
                    )?;
                    Some((folder, diagnostic.clone()))
                });
                let outcome = if let Some((folder, diagnostic)) = refusal {
                    let reason = crate::triggers::folder_grant::folder_refusal_reason(&folder);
                    let result = format!("{diagnostic}\n\n{reason}");
                    trigger_fail_reason =
                        Some(crate::engine::command_guard::TriggerCommandBlocked {
                            reason,
                            pattern: None,
                            folder: Some(folder),
                            thread_id,
                        });
                    Err(result)
                } else {
                    outcome
                };
                let (mut result, is_error) = match outcome {
                    Ok(text) => (text, false),
                    Err(text) => (text, true),
                };

                // INVARIANT: Not gated on `is_error`: a shell that exits non-zero is an
                // `Ok` result, and that is how a sandbox refusal arrives.
                if crate::engine::command_guard::command_text(&tool_call.name, &tool_call.arguments)
                    .is_some_and(|text| paths.tool(&tool_call.name, text))
                {
                    result.push_str("\n\n");
                    result.push_str(crate::engine::command_guard::SECRET_READ_STEER);
                }

                // INVARIANT: Close the checkpoint bracket. Deliberately outside the
                // success test: a command that failed or was cancelled midway
                // is exactly the one whose partial destruction the user wants
                // back, and the post image is what says how far it got.
                if let Some(pending) = pending_checkpoint.take() {
                    self.finalize_command_checkpoint(pending, thread_id, &meta)
                        .await;
                }

                if response.tool_calls.len() == 1 {
                    last_call_was_error = is_error;
                }

                // INVARIANT: A failed ask_user_question (empty `question`, empty batch, or
                // non-array `questions`) must force a re-ask next iteration
                // rather than letting the model degrade to prose — record it.
                if tool_call.name == tn::ASK_USER_QUESTION && is_error {
                    this_iter_question_ask_failed = true;
                }

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

                if tool_call.name == tn::REFRESH_APP {
                    if let Some(app_id) = tool_call.arguments.get("app_id").and_then(|v| v.as_str())
                    {
                        modified_app_uis.remove(app_id);
                    }
                }

                if tool_call.name == tn::LIST_FILES && !is_error {
                    cached_list_files = Some(result.clone());
                }

                if tool_call.name == tn::BROWSER_SCREENSHOT && !is_error {
                    if let Some(path) =
                        crate::engine::tools::browser::screenshot_artifact_path(&result)
                    {
                        images.push(path.to_string());
                    }
                }

                let (result, answer_blobs) = take_answer_images(&tool_call.name, result);
                let mut split = split_tool_result(&result);

                let sentinel_event =
                    match_sentinel(&tool_call.name, &tool_call.arguments, split.event_text()).map(
                        |m| {
                            if let Some(redacted) = m.redacted_text {
                                split.redact(redacted);
                            }
                            (m.label, m.event)
                        },
                    );

                self.emit_tool_result(
                    thread_id,
                    &meta,
                    &tool_call.name,
                    &mut split,
                    !is_error,
                    tool_called_event_id,
                )
                .await;

                if let Some((label, event)) = sentinel_event {
                    crate::engine::form_requests::emit_request(
                        &self.pool,
                        &self.event_bus,
                        thread_id,
                        event,
                        meta.clone(),
                        label,
                    )
                    .await;
                }

                if is_error {
                    had_errors = true;
                    log_legacy!(
                        "[AgentLoop] Step {}/{}: Error, will retry: {}",
                        tool_calls_made,
                        max_tool_calls,
                        result
                    );
                } else {
                    log_legacy!(
                        "[AgentLoop] Step {}/{}: Success",
                        tool_calls_made,
                        max_tool_calls
                    );
                }

                if result.starts_with("[PUSH_NOTIFICATION_REQUEST]") {
                    let actor =
                        device_id.map(|id| crate::engine::thread_events::MessageOrigin::Device {
                            device_id: id.to_string(),
                        });
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event:
                                    crate::engine::thread_events::ThreadEvent::PushNotificationRequested,
                                meta: crate::engine::thread_events::EventMeta::with_actor(actor),
                            },
                            "[AgenticLoop] PushNotificationRequested",
                        )
                        .await;
                }

                if is_error {
                    note_failure(&mut failed, tool_called_event_id, curated.mode);
                }

                tool_outputs.push(ToolOutput {
                    tool_use_id: tool_call.id.clone(),
                    images: load_attached_images(self.workspace_path(), &answer_blobs),
                    text: split.llm_text,
                    event_id: tool_called_event_id,
                });

                if trigger_fail_reason.is_some() {
                    break;
                }
                if quiet_answer && injection_rx.is_empty() {
                    break;
                }
            }

            question_ask_failed_last_iter = this_iter_question_ask_failed;

            // INVARIANT: A trigger's command was blocked by an ungranted side-effect (ADR
            // 0002, Phase 5). The blocked command's failed ToolResult is already
            // persisted, so the turn stops here either way. A block that names
            // a grant parks the run on the owner's card, with no terminator, and
            // the owner's Allow resumes this same turn. Any other block fails
            // the run with `ResponseFailed`, and the scheduler sends its notice.
            if let Some(mut blocked) = trigger_fail_reason {
                match crate::scheduler::user_tasks::park_blocked_run(
                    &self.clone_arc(),
                    &mut blocked,
                )
                .await
                {
                    Ok(true) => {
                        *terminator_settled = true;
                        return Err(Box::new(
                            crate::scheduler::user_tasks::TriggerRunAwaitsOwner,
                        ));
                    }
                    Ok(false) => {}
                    Err(e) => log_legacy!(
                        "[AgenticLoop] The grant card for thread {thread_id} was not raised: {e}"
                    ),
                }
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                error: blocked.reason.clone(),
                                spend_cap: None,
                                agent_exit: None,
                                provider_error: None,
                            },
                            meta: meta.clone(),
                        },
                        "[AgenticLoop] ResponseFailed (trigger side-effect not granted)",
                    )
                    .await;
                *terminator_settled = true;
                return Err(blocked.into());
            }

            if quiet_answer && injection_rx.is_empty() {
                self.gate_chat_turn_end(
                    thread_id,
                    origin_id,
                    read_decided,
                    &quiet_reply,
                    crate::engine::turn_end::TurnFinish::Finished(
                        crate::engine::thread_events::ChildCompletionStatus::Success,
                    ),
                )
                .await;
                self.refresh_apps_modified_this_turn(thread_id, &modified_app_uis)
                    .await;
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                                text: quiet_reply.clone(),
                                images: images.clone(),
                                model: effective_model.clone(),
                                reasoning_effort: effective_effort.clone(),
                            },
                            meta: meta.authored_by(
                                crate::engine::thread_events::AgentParticipant::LucidosAgent,
                            ),
                        },
                        "[AgenticLoop] ResponseGenerated (quiet answer)",
                    )
                    .await;
                *terminator_settled = true;
                return Ok(terminal_result(
                    quiet_reply,
                    images,
                    request_id,
                    thread_id,
                    *proposed_change,
                ));
            }

            if let Some(answer) = self
                .final_answer_beside_decision(FinalAnswerBesideDecision {
                    response: &response,
                    had_errors,
                    thread_id,
                    injections_pending: !injection_rx.is_empty(),
                    cancelled: cancel_token.is_cancelled(),
                    question: QuestionReaskInputs {
                        ask_failed_last_iter: question_ask_failed_last_iter,
                        leaked_as_text: question_leaked_as_text,
                        ends_in_a_question: false,
                        human_can_answer,
                        reask_forced: question_reask_forced,
                        prose_nudges_forced: question_prose_nudges_forced,
                    },
                    wake_nudges_forced: todo_wake_nudges_forced,
                })
                .await
            {
                self.gate_chat_turn_end(
                    thread_id,
                    origin_id,
                    read_decided,
                    &answer,
                    crate::engine::turn_end::TurnFinish::Finished(
                        crate::engine::thread_events::ChildCompletionStatus::Success,
                    ),
                )
                .await;
                self.refresh_apps_modified_this_turn(thread_id, &modified_app_uis)
                    .await;
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                                text: answer.clone(),
                                images: images.clone(),
                                model: effective_model.clone(),
                                reasoning_effort: effective_effort.clone(),
                            },
                            meta: meta.authored_by(
                                crate::engine::thread_events::AgentParticipant::LucidosAgent,
                            ),
                        },
                        "[AgenticLoop] ResponseGenerated (beside the turn-end decisions)",
                    )
                    .await;
                *terminator_settled = true;
                return Ok(terminal_result(
                    answer,
                    images,
                    request_id,
                    thread_id,
                    *proposed_change,
                ));
            }

            let mut assistant_blocks = Vec::new();
            // INVARIANT: The RAW reply, span included. Gemini's plan narration is barred
            // from the screen but belongs in its own turn. The working
            // understanding belongs in the array so the sweep can fold it.
            // Without either, the next round starts having forgotten what it
            // decided to do.
            if let Some(text) = raw_history_text.as_deref() {
                assistant_blocks.push(ContentBlock::Text {
                    text: text.to_string(),
                });
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
                TOOL_RESULTS_INSTRUCTION
            };

            let instruction = instruction_after_round(
                instruction,
                &response.progress_notes,
                response
                    .tool_calls
                    .iter()
                    .any(|tc| tc.name == tn::ASK_USER_QUESTION),
            );
            let result_blocks = build_tool_result_blocks(&tool_outputs, &instruction);

            // INVARIANT: Pin before the move. An image the model explicitly asked to see
            // must stay in vision for the rest of the turn, not just until its
            // next tool call. So must one the user attached to an answer.
            let pin_images = holds_explicitly_requested_image(&result_blocks);
            let pin_answer_images = holds_answer_images(&tool_outputs);
            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Blocks(result_blocks),
            });
            if pin_images {
                push_explicit_image_pin(&mut explicit_image_idxs, messages.len() - 1);
            }
            if pin_answer_images {
                user_image_idxs.push(messages.len() - 1);
            }

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
                    self.workspace_path(),
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

            // INVARIANT: After the first LLM call, emit the derived `ImageDescribed` fact(s)
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
