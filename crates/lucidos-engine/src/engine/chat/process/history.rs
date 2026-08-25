//! Conversation-history + resume-context loading for a chat turn. Derives the
//! stringified `[CONVERSATION HISTORY]` block, the verbatim resume tool
//! blocks, and the per-thread loaded-knowhow set from a single events fetch.
//! Split out of `process_message_with_steps_internal`.

use crate::core::events::{image_handle, ImageRef};
use crate::engine::context::{
    format_history_content, format_history_steps, format_image_refs, HISTORY_COMPRESS_THRESHOLD,
    HISTORY_MSG_TRUNCATE, HISTORY_OLDER_UNCOVERED_TURNS, HISTORY_OLDER_USER_BUDGET,
    HISTORY_RECENT_MESSAGES, HISTORY_SUMMARY_REFRESH_AFTER, HISTORY_VERBATIM_TAIL,
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
use super::super::process_helpers::summarize_or_none;
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
        // This turn's start instant, from `LucidosEngine::turn_started_at`. The
        // image ages below sit in the message prefix, which the prompt cache
        // keys on. So they are derived, never read off the wall clock.
        turn_started_at: chrono::DateTime<Utc>,
        // ADR 0109: the conversation summariser is off under the context mode.
        // The model writes notes as it goes, so the older region is already
        // compressed by the party that knew what mattered.
        context_mode: super::context_mode::ContextMode,
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
        // Filled from the same events walk, for the same reason: the loaded set
        // is keyed by doc id and the release key is the call's address.
        // The thread's cached conversation summary, from the same events walk.
        // `None` before its first successful summarisation (ADR 0102).
        let mut cached_summary: Option<crate::core::store::CachedSummary> = None;

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
                        cached_summary = crate::core::store::newest_conversation_summary(&events);
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
                                ", image not included, may be outdated"
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
                            // Handles for USER images only, because their
                            // hashes ARE what `walk_thread_image_refs` reads
                            // for a `MessageReceived`. An assistant message's
                            // `images` is not that faithful:
                            // `build_session_messages` fills it from
                            // `ResponseGenerated.payload.images` OR from
                            // accumulated `browser_screenshot` artifact PATHS.
                            // The walker reads neither of those second ones,
                            // so a handle from a path would name nothing.
                            let handles: Vec<String> = m
                                .user_image_hashes
                                .iter()
                                .map(|h| image_handle(ImageRef::BlobHash(h)))
                                .collect();
                            let range = format_image_refs(img_start, n, &handles);
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
                        // Long conversation. The older region keeps every user
                        // turn verbatim and compresses only the assistant side
                        // (ADR 0102).
                        let split_point = all_prior.len().saturating_sub(HISTORY_RECENT_MESSAGES);
                        let older = &all_prior[..split_point];
                        let recent = &all_prior[split_point..];

                        let mut plan = SummaryPlan::for_region(older, cached_summary.as_ref());

                        // A refresh is attempted only once the uncovered turns
                        // have piled up. Until then they render compacted, so
                        // waiting costs a few hundred chars and no model call.
                        //
                        // Under the context mode there is no refresh at all. An
                        // auxiliary model does not know what the thread is
                        // doing, so it writes a generic precis and drops the one
                        // constant the task needed. The working understanding
                        // covers the same region, written by the party that
                        // does know, and the compacted rendering is the
                        // fallback.
                        if plan.needs_refresh(context_mode) {
                            let fresh = self
                                .refresh_conversation_summary(
                                    thread_id,
                                    older,
                                    &msg_image_starts,
                                    &format_history_msg,
                                    !is_new_thread,
                                )
                                .await;
                            plan.apply_refresh(older, fresh);
                        }

                        let covered = plan.covered();
                        let older_region = render_older_region(
                            older,
                            &msg_image_starts,
                            covered,
                            &format_history_msg,
                        );

                        let recent_turns = format_tiered(recent, split_point);

                        let history = format!(
                            "[CONVERSATION HISTORY (recent)]\n{}\n\nRecent:\n{}\n[END HISTORY]",
                            older_region,
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

impl LucidosEngine {
    /// Re-summarise this thread's older assistant turns, and cache the result.
    ///
    /// `None` when the call failed, timed out or came back empty, and the
    /// caller then keeps whatever summary it already had (ADR 0102). User
    /// turns are deliberately not in the input.
    async fn refresh_conversation_summary<F>(
        &self,
        thread_id: Uuid,
        older: &[crate::core::store::SessionMessage],
        msg_image_starts: &[usize],
        format_msg: &F,
        // Whether `older` is this thread's OWN history. A new thread's older
        // region comes from `get_recent_messages`, which is other threads'
        // messages. See the guard below for what that forbids.
        thread_local: bool,
    ) -> Option<String>
    where
        F: Fn(&crate::core::store::SessionMessage, bool, usize, usize) -> String,
    {
        let extractor = self.extractor.as_ref()?;
        let newest = older.iter().rposition(|m| m.role == "assistant")?;
        let turns: Vec<String> = older
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == "assistant")
            .map(|(i, m)| format_msg(m, false, msg_image_starts[i], i))
            .collect();
        let covered_count = turns.len();
        let purpose = crate::engine::ContextPurpose::ConversationSummary;
        let capture = crate::engine::AuxCapture::new(&self.event_bus, thread_id, purpose);
        let call = crate::engine::aux_purpose::AuxCall::resolve(&self.pool, purpose).await;
        // Record what actually ran. `is_extractor_default` asks the same
        // question `provider_for_model` branches on, so the recorded name
        // cannot drift from the model the call hit.
        let recorded_model = if crate::engine::aux_purpose::is_extractor_default(call.model()) {
            extractor.default_background_model().to_string()
        } else {
            call.model().to_string()
        };
        let text = summarize_or_none(
            extractor.summarize_conversation(&turns.join("\n"), &call, Some(&capture)),
            covered_count,
            call.deadline(),
        )
        .await?;

        // A new thread must not cache. Its older region is a global recent
        // window over OTHER threads. A row written from it would file their
        // content as this thread's own, and its boundary would name an event
        // this thread's history never contains. The paragraph still rides
        // this turn, which is what the global window is for.
        if !thread_local {
            return Some(text);
        }

        // The cache is keyed on the boundary turn's address. Without one the
        // next turn could not tell how far this paragraph reaches, so the
        // paragraph rides this turn and is not persisted.
        match older[newest]
            .event_id
            .as_deref()
            .and_then(|id| Uuid::parse_str(id).ok())
        {
            Some(handle) => {
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event:
                                crate::engine::thread_events::ThreadEvent::ConversationSummarized {
                                    summary: text.clone(),
                                    covers_through_event_id: handle,
                                    covered_count: covered_count as u32,
                                    model: recorded_model,
                                },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[Chat] ConversationSummarized",
                    )
                    .await;
            }
            None => log!("[Chat] summary not cached: newest covered turn has no event id"),
        }
        Some(text)
    }
}

/// The covered assistant turns, exactly as the history block prints them.
///
/// The "resolved" claim rides here and nowhere else. It is true of work a
/// paragraph actually describes. ADR 0102 records what it cost when the same
/// wording introduced an empty fallback instead.
const EARLIER_SUMMARISED: &str =
    "[Earlier assistant work (resolved: do NOT re-attempt fixes described here): {}]";

/// Assistant turns past `HISTORY_OLDER_UNCOVERED_TURNS` that no summary covers.
///
/// It claims nothing about them, and names the way back (ADR 0102). The route
/// is the grouped `events` tool, matching `context_mode`'s recovery table: the
/// flat `query_events` still resolves as an alias, and two names for one
/// capability is the drift `.claude/rules/glossary.md` bans.
///
/// Naming ONE event type is right here, unlike the whole-history row in that
/// table. What left is one side of the conversation, so one type reaches all
/// of it.
const EARLIER_ASSISTANT_ELIDED: &str = "[Earlier: {} assistant turns before this are not shown. \
     Read them with events(action=\"query\", thread_id=\"current\", \
     event_type=\"ResponseGenerated\").]";

/// User turns past `HISTORY_OLDER_USER_BUDGET`.
///
/// Only a thread of very large pasted messages reaches this line. A user turn
/// is otherwise verbatim for the life of the thread.
const EARLIER_USER_ELIDED: &str = "[Earlier: {} of the user's own messages before this are not \
     shown. Read them with events(action=\"query\", thread_id=\"current\", \
     event_type=\"MessageReceived\").]";

/// A summary and the newest turn it accounts for, which travel together.
///
/// One value rather than two options, so the region cannot render a boundary
/// with no paragraph behind it.
#[derive(Clone, Copy)]
pub(super) struct CoveredSummary<'a> {
    pub text: &'a str,
    /// Index into the older slice of the newest turn the text covers.
    pub boundary: usize,
}

/// What this turn does about the older region's summary (ADR 0102).
///
/// Holds the paragraph in play and how far it reaches. A refresh replaces
/// both, and a FAILED refresh replaces neither, which is what makes one
/// success stick for the rest of the thread.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct SummaryPlan {
    text: Option<String>,
    /// Index into the older slice of the newest turn `text` covers.
    boundary: Option<usize>,
    /// Assistant turns past that boundary, or all of them with no summary.
    uncovered: usize,
}

impl SummaryPlan {
    /// Read the thread's cached summary against the region it has to cover.
    ///
    /// A cached paragraph whose boundary turn has left the region is
    /// unusable: nothing says how far it reaches any more, so it is dropped
    /// rather than trusted at the wrong width.
    pub(super) fn for_region(
        older: &[crate::core::store::SessionMessage],
        cached: Option<&crate::core::store::CachedSummary>,
    ) -> Self {
        let boundary = cached.and_then(|c| {
            older
                .iter()
                .position(|m| m.event_id.as_deref() == Some(c.covers_through_event_id.as_str()))
        });
        let text = boundary.and(cached).map(|c| c.summary.clone());
        let uncovered = older
            .iter()
            .enumerate()
            .filter(|(i, m)| m.role == "assistant" && boundary.is_none_or(|b| *i > b))
            .count();
        Self {
            text,
            boundary,
            uncovered,
        }
    }

    /// Whether this turn re-summarises the older region.
    ///
    /// Two conditions, both required. The uncovered turns must have piled up
    /// past [`HISTORY_SUMMARY_REFRESH_AFTER`], and the context mode must be
    /// off. Under the mode the model writes the notes itself. An auxiliary pass
    /// over the same region is then a second summary, worse and for a fee
    /// (ADR 0109).
    pub(super) fn needs_refresh(&self, mode: super::context_mode::ContextMode) -> bool {
        !mode.is_on() && self.uncovered > HISTORY_SUMMARY_REFRESH_AFTER
    }

    /// Take a fresh paragraph, which now covers every assistant turn here.
    ///
    /// `None` is the failure case and deliberately changes nothing. The
    /// thread keeps whatever it had, so a summariser that lands once holds
    /// even when every later call errors or times out.
    pub(super) fn apply_refresh(
        &mut self,
        older: &[crate::core::store::SessionMessage],
        fresh: Option<String>,
    ) {
        let Some(text) = fresh else {
            return;
        };
        self.boundary = older.iter().rposition(|m| m.role == "assistant");
        self.uncovered = 0;
        self.text = Some(text);
    }

    /// The paragraph and its boundary, when both are present.
    pub(super) fn covered(&self) -> Option<CoveredSummary<'_>> {
        self.text
            .as_deref()
            .zip(self.boundary)
            .map(|(text, boundary)| CoveredSummary { text, boundary })
    }
}

/// The older region of `[CONVERSATION HISTORY]`, in chronological order.
///
/// Three kinds of line come out (ADR 0102). A user turn is verbatim. The
/// assistant turns a summary covers become one paragraph, printed where the
/// oldest of them sat. An uncovered assistant turn renders compacted, so the
/// gap between refreshes drops nothing.
///
/// Two caps bound the region, both counted newest-first, each replaced by one
/// count line naming the way back. `HISTORY_OLDER_USER_BUDGET` bounds the
/// verbatim user side. `HISTORY_OLDER_UNCOVERED_TURNS` bounds the compacted
/// assistant side, which is what a thread gets when the summariser keeps
/// failing.
pub(super) fn render_older_region<F>(
    older: &[crate::core::store::SessionMessage],
    msg_image_starts: &[usize],
    covered: Option<CoveredSummary<'_>>,
    format_msg: &F,
) -> String
where
    F: Fn(&crate::core::store::SessionMessage, bool, usize, usize) -> String,
{
    let is_covered = |i: usize| covered.is_some_and(|c| i <= c.boundary);
    let first_covered = (0..older.len()).find(|&i| older[i].role == "assistant" && is_covered(i));

    let (keep_user, dropped_users) = user_turns_within_budget(older);
    let (keep_assistant, dropped_assistant) = uncovered_turns_within_cap(older, &is_covered);

    // The elided assistant turns sit between the summarised run and the ones
    // still shown, so their count line goes there rather than at the top.
    // Anchored above the region it would put a NEWER run above an older
    // summary, and its own "before this" would point at the wrong place.
    let elided_note = (dropped_assistant > 0)
        .then(|| EARLIER_ASSISTANT_ELIDED.replace("{}", &dropped_assistant.to_string()));
    let elided_anchor = elided_note
        .as_ref()
        .and_then(|_| (0..older.len()).find(|&i| keep_assistant[i]));

    let mut lines: Vec<String> = Vec::new();
    // Elided user turns ARE the oldest thing in the region, so their count
    // opens it.
    if dropped_users > 0 {
        lines.push(EARLIER_USER_ELIDED.replace("{}", &dropped_users.to_string()));
    }
    // Nothing uncovered survived the cap, so there is no turn for the count to
    // sit above. Unreachable while the cap is above zero, and cheaper to hold
    // than to reason about again.
    if let (Some(note), None) = (&elided_note, elided_anchor) {
        lines.push(note.clone());
    }
    for (i, msg) in older.iter().enumerate() {
        if elided_anchor == Some(i) {
            if let Some(note) = &elided_note {
                lines.push(note.clone());
            }
        }
        let keep = if msg.role == "user" {
            keep_user[i]
        } else if is_covered(i) {
            // The paragraph stands in for the whole covered run, printed
            // once where the run begins.
            if Some(i) == first_covered {
                if let Some(c) = covered {
                    lines.push(EARLIER_SUMMARISED.replace("{}", c.text));
                }
            }
            continue;
        } else {
            keep_assistant[i]
        };
        if keep {
            lines.push(format_msg(msg, false, msg_image_starts[i], i));
        }
    }
    lines.join("\n")
}

/// Which user turns fit `HISTORY_OLDER_USER_BUDGET`, and how many do not.
///
/// Counted newest-first, and the walk stops at the first turn that does not
/// fit. So the kept set is always a contiguous run ending at the newest turn,
/// rather than whichever turns happened to be small.
///
/// A turn is charged what it will actually PRINT, which
/// `format_history_content` caps at `HISTORY_MSG_TRUNCATE`. Charge the raw
/// length instead and a pasted log that fits once truncated is elided anyway.
/// Newest-first, so the turn dropped would be the most recent one.
fn user_turns_within_budget(older: &[crate::core::store::SessionMessage]) -> (Vec<bool>, usize) {
    let mut keep = vec![false; older.len()];
    let mut left = HISTORY_OLDER_USER_BUDGET;
    for i in (0..older.len()).rev() {
        if older[i].role != "user" {
            continue;
        }
        let cost = older[i].content.chars().count().min(HISTORY_MSG_TRUNCATE);
        if cost > left {
            let dropped = older[..=i].iter().filter(|m| m.role == "user").count();
            return (keep, dropped);
        }
        left -= cost;
        keep[i] = true;
    }
    (keep, 0)
}

/// Which uncovered assistant turns fit `HISTORY_OLDER_UNCOVERED_TURNS`, and
/// how many do not. Newest-first, same contiguous-run rule as above.
fn uncovered_turns_within_cap<C>(
    older: &[crate::core::store::SessionMessage],
    is_covered: &C,
) -> (Vec<bool>, usize)
where
    C: Fn(usize) -> bool,
{
    let mut keep = vec![false; older.len()];
    let mut left = HISTORY_OLDER_UNCOVERED_TURNS;
    let mut dropped = 0usize;
    for i in (0..older.len()).rev() {
        if older[i].role == "user" || is_covered(i) {
            continue;
        }
        if left == 0 {
            dropped += 1;
            continue;
        }
        left -= 1;
        keep[i] = true;
    }
    (keep, dropped)
}
