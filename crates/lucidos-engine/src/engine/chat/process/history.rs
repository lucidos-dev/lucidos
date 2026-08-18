//! Conversation-history + resume-context loading for a chat turn. Derives the
//! stringified `[CONVERSATION HISTORY]` block, the verbatim resume tool
//! blocks, and the per-thread loaded-knowhow set from a single events fetch.
//! Split out of `process_message_with_steps_internal`.

use crate::engine::context::{
    format_history_content, format_history_steps, HISTORY_COMPRESS_THRESHOLD,
    HISTORY_RECENT_MESSAGES, HISTORY_VERBATIM_TAIL,
};
use crate::engine::loaded_knowhow::LoadedKnowhow;
use crate::engine::LucidosEngine;
use crate::llm::Message;
use chrono::Utc;
use uuid::Uuid;

use super::super::events::format_relative_age;
use super::super::images::{
    filter_recent_history_image_hashes, image_recency_cutoff, MAX_HISTORY_IMAGE_MESSAGES,
};
use super::super::process_helpers::summarize_or_fallback;
use super::context_build::summarize_user_topics;

/// Result of [`LucidosEngine::load_chat_history`]: everything derived from the
/// single per-thread events fetch that the rest of the turn consumes.
pub(super) struct ChatHistoryLoad {
    /// Verbatim `(ToolUse, ToolResult)` Message pairs for the most recent N
    /// tool calls (plus pinned `load_knowhow` results), prepended to the LLM
    /// messages vec.
    pub resume_tool_blocks: Vec<Message>,
    /// Per-thread loaded knowhow docs (warm- and cold-path), consumed by the
    /// resume-block stub swap, history body-strip, `[LOADED KNOWHOW]` block,
    /// and capture sections.
    pub loaded_knowhow_docs: Vec<LoadedKnowhow>,
    /// Stringified `[CONVERSATION HISTORY]` block (may be trimmed by budget).
    pub history_context: String,
    /// 500-char extraction summary fed to memory classification/extraction.
    pub conversation_summary: String,
    /// Per-message image hashes carried into the LLM content builder.
    pub history_image_hashes: Vec<Vec<String>>,
}

impl LucidosEngine {
    /// Load conversation history + resume tool blocks + loaded knowhow for a
    /// chat turn. Verbatim extraction of the inline block from
    /// `process_message_with_steps_internal`.
    pub(super) async fn load_chat_history(
        &self,
        is_trigger: bool,
        is_new_thread: bool,
        thread_id: Uuid,
        user_message: &str,
        memory_model_pref: &str,
        // This turn's start instant, from `LucidosEngine::turn_started_at`. The
        // image ages below sit in the message prefix, which the prompt cache
        // keys on. So they are derived, never read off the wall clock.
        turn_started_at: chrono::DateTime<Utc>,
    ) -> ChatHistoryLoad {
        // Resume tool blocks: full ToolUse + ToolResult Message pairs for the
        // most recent N tool calls (Phase 3). Pinned `load_knowhow` results
        // survive regardless of N — see
        // `build_resume_tool_blocks_with_skip_ids`. Empty for triggers and
        // the no-history path.
        let mut resume_tool_blocks: Vec<Message> = Vec::new();
        let mut resume_skip_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // Per-thread loaded knowhow docs. Populated inside the follow-up
        // branch below (after `recover_for_thread` runs) so it carries every
        // successful `load_knowhow` for this thread — warm-path
        // (handler-populated) and cold-path (recovery-replayed). Consumed by
        // (a) the resume-block body-stub swap (by id), (b)
        // `format_history_content`'s body-strip (by exact body substring),
        // (c) the `[LOADED KNOWHOW]` user-message block, and (d)
        // `build_capture_sections`. Single fetch, multiple consumers — bodies
        // can be 5-50KB each so the prior triple-fetch wasted ~150KB clones
        // per turn.
        let mut loaded_knowhow_docs: Vec<crate::engine::loaded_knowhow::LoadedKnowhow> = Vec::new();

        // Load conversation history from DB.
        // For follow-ups, load only the thread's messages to avoid cross-thread leakage.
        // If >HISTORY_COMPRESS_THRESHOLD messages, older messages are summarized via Flash
        // and only the last HISTORY_RECENT_MESSAGES are included verbatim.
        let (history_context, conversation_summary, history_image_hashes) = if !is_trigger {
            // Follow-ups: scope to thread; new threads: load global recent messages.
            //
            // For follow-ups we fetch the thread's events ONCE and derive both
            // the SessionMessage history (for stringified `[CONVERSATION
            // HISTORY]` formatting) AND the verbatim resume tool blocks (most
            // recent N + pinned `load_knowhow` results) from that single
            // walk. The earlier shape did two separate DB calls
            // (`get_thread_messages` then `get_thread_events`) for the same
            // rows — same SQL, same thread_id, twice the round-trip — and
            // also silently swallowed the second call's error, losing
            // procedure context exactly when the bug Phase 3 fixes recurs.
            let messages_result = if !is_new_thread {
                match self
                    .event_store
                    .get_thread_events(&thread_id.to_string())
                    .await
                {
                    Ok(events) => {
                        // Engine restart loses the per-thread loaded-knowhow set
                        // (in-memory only). Replay this thread's load_knowhow
                        // ToolResult events into the store before deriving the
                        // resume tool blocks so the dedupe in Phase 4 still
                        // fires after a cold start. Idempotent — re-running on
                        // the same events converges. Only replay when the slot
                        // is empty, so this is a no-op on the warm path
                        // (handler already populated it on the prior turn).
                        let docs_empty = self.loaded_knowhow.for_thread(thread_id).await.is_empty();
                        if docs_empty {
                            self.loaded_knowhow
                                .recover_for_thread(thread_id, &events)
                                .await;
                        }
                        // Single fetch — every consumer below borrows from
                        // this Vec instead of cloning anew.
                        loaded_knowhow_docs = self.loaded_knowhow.for_thread(thread_id).await;
                        let loaded_knowhow_ids: std::collections::HashSet<String> =
                            loaded_knowhow_docs.iter().map(|d| d.id.clone()).collect();
                        let (blocks, skip_ids) =
                            crate::core::store::build_resume_tool_blocks_with_skip_ids(
                                &events,
                                crate::core::store::RESUME_VERBATIM_TOOL_TAIL,
                                &loaded_knowhow_ids,
                            );
                        resume_tool_blocks = blocks;
                        resume_skip_ids = skip_ids;
                        Ok(crate::core::store::build_session_messages(&events))
                    }
                    Err(e) => {
                        log!(
                            "[Chat] resume context load failed (DB error): {}; \
                             orchestrator will resume without verbatim tool history",
                            e
                        );
                        Err(e)
                    }
                }
            } else {
                self.event_store
                    .get_recent_messages((HISTORY_RECENT_MESSAGES * 2 + 2) as i64, None)
                    .await
            };

            match messages_result {
                Ok(messages) => {
                    // All messages except the one we just appended (last one)
                    let all_prior: Vec<_> = if messages
                        .last()
                        .map(|m| m.role == "user" && m.content == user_message)
                        .unwrap_or(false)
                    {
                        messages[..messages.len().saturating_sub(1)].to_vec()
                    } else {
                        messages
                    };

                    // New threads pull history from multiple recent threads — their
                    // images are irrelevant (and can consume hundreds of thousands of tokens).
                    let prior_image_hashes: Vec<Vec<String>> = if is_new_thread {
                        vec![]
                    } else {
                        filter_recent_history_image_hashes(&all_prior, MAX_HISTORY_IMAGE_MESSAGES)
                    };

                    // Per-message flag: is this message's image data included in the
                    // LLM context? Used by format_history_msg to annotate dropped
                    // images with "image not included, may be outdated".
                    let image_data_included: Vec<bool> = {
                        let cutoff = image_recency_cutoff(&all_prior, MAX_HISTORY_IMAGE_MESSAGES);
                        let mut user_idx = 0usize;
                        all_prior
                            .iter()
                            .map(|m| {
                                if m.role == "user" {
                                    let included = !is_new_thread
                                        && user_idx >= cutoff
                                        && !m.user_image_hashes.is_empty();
                                    user_idx += 1;
                                    included
                                } else {
                                    false
                                }
                            })
                            .collect()
                    };

                    // Pre-compute thread image indices per message so history annotations
                    // can include thread:N references (e.g. "[attached image (thread:3)]").
                    // This counts ALL images (user + generated) in sequential order to match
                    // the thread:N numbering used by walk_thread_images.
                    let msg_image_starts: Vec<usize> = {
                        let mut starts = Vec::with_capacity(all_prior.len());
                        let mut idx: usize = 0;
                        for m in all_prior.iter() {
                            starts.push(idx);
                            if m.role == "user" {
                                idx += m.user_image_hashes.len();
                            } else {
                                idx += m.images.len();
                            }
                        }
                        starts
                    };

                    // Bodies of currently-loaded knowhow docs — used to strip
                    // verbatim repeats from `[CONVERSATION HISTORY]`. The
                    // body IS the formatted `[SYSTEM-KNOWHOW: <name>]` block
                    // produced by `load_one_knowhow_section`, so substring
                    // match against the same source is exact and avoids the
                    // id-vs-name pitfall (loaded set is keyed by id, but the
                    // marker uses name).
                    let loaded_knowhow_bodies: Vec<&str> = loaded_knowhow_docs
                        .iter()
                        .map(|d| d.body.as_str())
                        .collect();

                    // Format a message for history context with tiered truncation.
                    // - Last HISTORY_VERBATIM_TAIL messages: fully verbatim (only 15K safety net)
                    // - Earlier messages: user messages verbatim, assistant messages compacted to ~1500 chars
                    // `msg_idx` indexes into `all_prior` to look up `image_data_included`.
                    let format_history_msg = |m: &crate::core::store::SessionMessage,
                                              is_verbatim: bool,
                                              img_start: usize,
                                              msg_idx: usize|
                     -> String {
                        let role = if m.role == "user" {
                            "User"
                        } else {
                            "Assistant"
                        };
                        let content = format_history_content(
                            &m.content,
                            &m.role,
                            is_verbatim,
                            &loaded_knowhow_bodies,
                        );
                        // Determine image kind: user-attached (with staleness tracking) or generated
                        let (label, n, stale_note) = if !m.user_image_hashes.is_empty() {
                            let included =
                                image_data_included.get(msg_idx).copied().unwrap_or(false);
                            let stale = if !included {
                                " — image not included, may be outdated"
                            } else {
                                ""
                            };
                            ("attached", m.user_image_hashes.len(), stale)
                        } else if !m.images.is_empty() {
                            ("generated", m.images.len(), "")
                        } else {
                            ("", 0, "")
                        };
                        let image_note = if n == 0 {
                            // No image data, but if a description survived, show it as text context
                            m.image_description
                                .as_ref()
                                .map(|d| format!(" [image description: {}]", d))
                                .unwrap_or_default()
                        } else {
                            let age = format_relative_age(turn_started_at - m.created_at);
                            let range = if n <= 1 {
                                format!("thread:{}", img_start + 1)
                            } else {
                                format!("thread:{}-thread:{}", img_start + 1, img_start + n)
                            };
                            let count_prefix = if n <= 1 {
                                format!("{} image", label)
                            } else {
                                format!("{} {} images", label, n)
                            };
                            let desc_suffix = m
                                .image_description
                                .as_ref()
                                .map(|d| format!(": {}", d))
                                .unwrap_or_default();
                            format!(
                                " [{} ({}, {}{}){}]",
                                count_prefix, range, age, stale_note, desc_suffix
                            )
                        };
                        // Assistant turns may have only tool calls; m.content covers prose only.
                        // Tools whose `tool_called_event_id` is in `resume_skip_ids`
                        // are already represented as full Message::Blocks(...) pairs
                        // prepended to the LLM messages vec — suppress the
                        // duplicate `[tools: ...]` summary for them.
                        let steps_summary = if m.role == "assistant" {
                            format_history_steps(&m.steps, &resume_skip_ids).unwrap_or_default()
                        } else {
                            String::new()
                        };
                        format!("{}: {}{}{}", role, content, steps_summary, image_note)
                    };

                    // Format messages with tiered truncation based on position.
                    // `idx_offset` is the index into both msg_image_starts and image_data_included.
                    let format_tiered = |msgs: &[crate::core::store::SessionMessage],
                                         idx_offset: usize|
                     -> Vec<String> {
                        let tail_start = msgs.len().saturating_sub(HISTORY_VERBATIM_TAIL);
                        msgs.iter()
                            .enumerate()
                            .map(|(i, m)| {
                                format_history_msg(
                                    m,
                                    i >= tail_start,
                                    msg_image_starts[idx_offset + i],
                                    idx_offset + i,
                                )
                            })
                            .collect()
                    };

                    if all_prior.is_empty() {
                        (String::new(), user_message.to_string(), prior_image_hashes)
                    } else if all_prior.len() <= HISTORY_COMPRESS_THRESHOLD {
                        // Short conversation — include all messages with tiered truncation
                        let turns = format_tiered(&all_prior, 0);
                        let history = format!(
                            "[CONVERSATION HISTORY (recent)]\n{}\n[END HISTORY]",
                            turns.join("\n")
                        );

                        let summary = summarize_user_topics(&all_prior, user_message);
                        (history, summary, prior_image_hashes)
                    } else {
                        // Long conversation — summarize older messages, keep recent with tiered truncation
                        let split_point = all_prior.len().saturating_sub(HISTORY_RECENT_MESSAGES);
                        let older = &all_prior[..split_point];
                        let recent = &all_prior[split_point..];

                        let older_summary = if let Some(ref extractor) = self.extractor {
                            let older_turns: Vec<String> = older
                                .iter()
                                .enumerate()
                                .map(|(i, m)| format_history_msg(m, false, msg_image_starts[i], i))
                                .collect();
                            let older_text = older_turns.join("\n");
                            summarize_or_fallback(
                                extractor
                                    .summarize_conversation(&older_text, Some(memory_model_pref)),
                                older.len(),
                            )
                            .await
                        } else {
                            format!("({} earlier messages not shown)", older.len())
                        };

                        let recent_turns = format_tiered(recent, split_point);

                        let history = format!(
                            "[CONVERSATION HISTORY (recent)]\n[Earlier (resolved — do NOT re-attempt fixes described here): {}]\n\nRecent:\n{}\n[END HISTORY]",
                            older_summary,
                            recent_turns.join("\n")
                        );

                        // Build extraction context from recent messages only
                        let summary = summarize_user_topics(recent, user_message);
                        (history, summary, prior_image_hashes)
                    }
                }
                Err(_) => (String::new(), user_message.to_string(), vec![]),
            }
        } else {
            (String::new(), user_message.to_string(), vec![])
        };

        ChatHistoryLoad {
            resume_tool_blocks,
            loaded_knowhow_docs,
            history_context,
            conversation_summary,
            history_image_hashes,
        }
    }
}
