use std::collections::HashMap;

use crate::core::events::{image_handle, thread_image_refs_of};
use crate::core::EventRow;
use crate::engine::consumption_claim::is_permission_held;
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

pub(super) fn speaker_label<'a>(
    m: &'a crate::core::store::SessionMessage,
    users: &crate::gateway_users::UserSnapshot,
) -> std::borrow::Cow<'a, str> {
    match (m.role.as_str(), m.agent.as_ref()) {
        ("user", _) => users
            .speaker_of_device(m.sender_device_id.as_deref())
            .into(),
        (_, Some(agent)) => agent.speaker_label().into(),
        _ => "Assistant".into(),
    }
}

#[derive(Default)]
pub(super) struct HistoryImageWalk {
    pub(super) handles: Vec<String>,
    held_starts: HashMap<String, usize>,
}

impl HistoryImageWalk {
    pub(super) fn from_events(events: &[EventRow]) -> Self {
        let mut walk = Self::default();
        for event in events {
            let images = thread_image_refs_of(&event.event_type, &event.payload);
            if images.is_empty() {
                continue;
            }
            if is_permission_held(event) {
                walk.held_starts
                    .insert(event.id.to_string(), walk.handles.len());
                walk.handles
                    .resize(walk.handles.len() + images.len(), String::new());
            } else {
                walk.handles
                    .extend(images.into_iter().map(|(_, image)| image_handle(image)));
            }
        }
        walk
    }
}

pub(super) fn locate_message_images(
    messages: &[crate::core::store::SessionMessage],
    walk: &HistoryImageWalk,
) -> Vec<Option<usize>> {
    let mut cursor = 0;
    messages
        .iter()
        .map(|m| {
            let handles = m.image_handles.as_slice();
            if handles.is_empty() {
                return None;
            }
            if let Some(start) = m.event_id.as_ref().and_then(|id| walk.held_starts.get(id)) {
                return Some(*start);
            }
            let found = walk
                .handles
                .get(cursor..)?
                .windows(handles.len())
                .position(|window| window == handles)
                .map(|offset| cursor + offset)?;
            cursor = found + handles.len();
            Some(found)
        })
        .collect()
}

pub(super) fn bodies_of_loaded_knowhow(docs: &[LoadedKnowhow]) -> Vec<&str> {
    docs.iter().map(|d| d.body.as_str()).collect()
}

pub(super) fn remove_current_prompt(
    messages: &mut Vec<crate::core::store::SessionMessage>,
    request_id: Option<Uuid>,
    user_message: &str,
) {
    let woke_at = if let Some(id) = request_id {
        let id = id.to_string();
        messages.iter().position(|message| {
            message.role == "user" && message.event_id.as_deref() == Some(id.as_str())
        })
    } else {
        messages
            .iter()
            .rposition(|message| message.role == "user" && message.content == user_message)
            .filter(|&at| {
                messages[at + 1..]
                    .iter()
                    .all(|message| message.role == "user")
            })
    };
    if let Some(at) = woke_at {
        messages.remove(at);
    }
}

pub(super) struct ChatHistoryLoad {
    pub resume_tool_blocks: Vec<Message>,
    pub loaded_knowhow_docs: Vec<LoadedKnowhow>,
    pub history_context: String,
    pub conversation_summary: String,
    pub history_image_hashes: Vec<Vec<String>>,
}

#[easy_ext::ext(HistoryExt)]
pub(crate) impl LucidosEngine {
    async fn load_chat_history(
        &self,
        skip_history: bool,
        request_id: Option<Uuid>,
        thread_id: Uuid,
        user_message: &str,
        turn_started_at: chrono::DateTime<Utc>,
        gates: &crate::llm::ToolCapabilities,
    ) -> ChatHistoryLoad {
        let context_mode = super::context_mode::ContextMode::from_capabilities(gates);
        if skip_history {
            return ChatHistoryLoad {
                resume_tool_blocks: Vec::new(),
                loaded_knowhow_docs: Vec::new(),
                history_context: String::new(),
                conversation_summary: user_message.to_string(),
                history_image_hashes: Vec::new(),
            };
        }
        if gates.memory_tree {
            return ChatHistoryLoad {
                resume_tool_blocks: Vec::new(),
                loaded_knowhow_docs: match self.thread_loaded_knowhow(thread_id).await {
                    Ok(docs) => docs,
                    Err(e) => {
                        log_legacy!(
                            "[Chat] Could not read the loaded knowhow of {}: {}",
                            thread_id,
                            e
                        );
                        Vec::new()
                    }
                },
                history_context: String::new(),
                conversation_summary: user_message.to_string(),
                history_image_hashes: Vec::new(),
            };
        }

        let mut resume_tool_blocks: Vec<Message> = Vec::new();
        let mut resume_skip_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut loaded_knowhow_docs: Vec<crate::engine::loaded_knowhow::LoadedKnowhow> = Vec::new();
        let mut cached_summary: Option<crate::core::store::CachedSummary> = None;

        let (history_context, conversation_summary, history_image_hashes) = {
            let messages_result = match self
                .event_store
                .get_thread_events(&thread_id.to_string())
                .await
            {
                Ok(mut events) => {
                    loaded_knowhow_docs = crate::engine::loaded_knowhow::loaded_knowhow(&events);
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
                    let image_walk = HistoryImageWalk::from_events(&events);
                    let held_request = request_id.filter(|&id| {
                        events
                            .iter()
                            .any(|row| row.id == id && is_permission_held(row))
                    });
                    if let Err(error) = self
                        .event_store
                        .project_permission_held_inputs(thread_id, &mut events)
                        .await
                    {
                        log_legacy!(
                            "[Chat] could not load delivery receipts for {thread_id}: {error}"
                        );
                        events.retain(|row| !is_permission_held(row));
                    }
                    Ok((
                        self.event_store.build_messages_now(&events).await,
                        image_walk,
                        held_request,
                    ))
                }
                Err(e) => {
                    log_legacy!(
                        "[Chat] resume context load failed (DB error): {}; \
                         orchestrator will resume without verbatim tool history",
                        e
                    );
                    Err(e)
                }
            };

            match messages_result {
                Ok((messages, image_walk, held_request)) => {
                    let mut all_prior = messages;
                    remove_current_prompt(&mut all_prior, held_request, user_message);

                    let prior_image_hashes: Vec<Vec<String>> =
                        filter_recent_history_image_hashes(&all_prior, MAX_HISTORY_IMAGE_MESSAGES);

                    let image_data_included: Vec<bool> = {
                        let cutoff = image_recency_cutoff(&all_prior, MAX_HISTORY_IMAGE_MESSAGES);
                        let mut user_idx = 0usize;
                        all_prior
                            .iter()
                            .map(|m| {
                                if m.role == "user" {
                                    let included =
                                        user_idx >= cutoff && !m.user_image_hashes.is_empty();
                                    user_idx += 1;
                                    included
                                } else {
                                    false
                                }
                            })
                            .collect()
                    };

                    let image_starts = locate_message_images(&all_prior, &image_walk);

                    let loaded_knowhow_bodies = bodies_of_loaded_knowhow(&loaded_knowhow_docs);
                    let users = crate::gateway_users::current();

                    let format_history_msg = |m: &crate::core::store::SessionMessage,
                                              is_verbatim: bool,
                                              msg_idx: usize|
                     -> String {
                        let role = speaker_label(m, &users);
                        let content = format_history_content(
                            &m.content,
                            &m.role,
                            is_verbatim,
                            &loaded_knowhow_bodies,
                        );
                        let n = m.image_handles.len();
                        let (label, stale_note) = if m.role == "user" {
                            let included =
                                image_data_included.get(msg_idx).copied().unwrap_or(false);
                            let stale = if !included {
                                ", image not included, may be outdated"
                            } else {
                                ""
                            };
                            ("attached", stale)
                        } else {
                            ("generated", "")
                        };
                        let image_note = if n == 0 {
                            m.image_description
                                .as_ref()
                                .map(|d| format!(" [image description: {}]", d))
                                .unwrap_or_default()
                        } else {
                            let age = format_relative_age(turn_started_at - m.created_at);
                            let range = format_image_refs(image_starts[msg_idx], &m.image_handles);
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
                        let steps_summary = if m.role == "assistant" {
                            format_history_steps(&m.steps, &resume_skip_ids).unwrap_or_default()
                        } else {
                            String::new()
                        };
                        format!("{}: {}{}{}", role, content, steps_summary, image_note)
                    };

                    let format_tiered = |msgs: &[crate::core::store::SessionMessage],
                                         idx_offset: usize|
                     -> Vec<String> {
                        let tail_start = msgs.len().saturating_sub(HISTORY_VERBATIM_TAIL);
                        msgs.iter()
                            .enumerate()
                            .map(|(i, m)| format_history_msg(m, i >= tail_start, idx_offset + i))
                            .collect()
                    };

                    if all_prior.is_empty() {
                        (String::new(), user_message.to_string(), prior_image_hashes)
                    } else if all_prior.len() <= HISTORY_COMPRESS_THRESHOLD {
                        let turns = format_tiered(&all_prior, 0);
                        let history = format!(
                            "[CONVERSATION HISTORY (recent)]\n{}\n[END HISTORY]",
                            turns.join("\n")
                        );
                        let summary = summarize_user_topics(&all_prior, user_message);
                        (history, summary, prior_image_hashes)
                    } else {
                        let split_point = all_prior.len().saturating_sub(HISTORY_RECENT_MESSAGES);
                        let older = &all_prior[..split_point];
                        let recent = &all_prior[split_point..];

                        let plan = SummaryPlan::for_region(older, cached_summary.as_ref());

                        if let Some(boundary) = plan.refresh_boundary(older, context_mode) {
                            self.spawn_conversation_summary_refresh(
                                thread_id,
                                older,
                                &format_history_msg,
                                boundary,
                            )
                            .await;
                        }

                        let covered = plan.covered();
                        let older_region = render_older_region(older, covered, &format_history_msg);

                        let recent_turns = format_tiered(recent, split_point);

                        let history = format!(
                            "[CONVERSATION HISTORY (recent)]\n{}\n\nRecent:\n{}\n[END HISTORY]",
                            older_region,
                            recent_turns.join("\n")
                        );
                        let summary = summarize_user_topics(recent, user_message);
                        (history, summary, prior_image_hashes)
                    }
                }
                Err(_) => (String::new(), user_message.to_string(), vec![]),
            }
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

pub(super) struct SummaryInFlight {
    threads: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<Uuid>>>,
    thread_id: Uuid,
}

impl SummaryInFlight {
    pub(super) fn claim(
        threads: &std::sync::Arc<std::sync::Mutex<std::collections::HashSet<Uuid>>>,
        thread_id: Uuid,
    ) -> Option<Self> {
        let claimed = threads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(thread_id);
        claimed.then(|| Self {
            threads: threads.clone(),
            thread_id,
        })
    }
}

impl Drop for SummaryInFlight {
    fn drop(&mut self) {
        self.threads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.thread_id);
    }
}

#[easy_ext::ext(ConversationSummaryExt)]
pub(crate) impl LucidosEngine {
    async fn spawn_conversation_summary_refresh<F>(
        &self,
        thread_id: Uuid,
        older: &[crate::core::store::SessionMessage],
        format_msg: &F,
        boundary: Uuid,
    ) where
        F: Fn(&crate::core::store::SessionMessage, bool, usize) -> String,
    {
        let Some(in_flight) = SummaryInFlight::claim(&self.summarizing_threads, thread_id) else {
            return;
        };

        let turns: Vec<String> = older
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == "assistant")
            .map(|(i, m)| format_msg(m, false, i))
            .collect();
        let covered_count = turns.len();
        let body = turns.join("\n");

        let purpose = crate::engine::ContextPurpose::ConversationSummary;
        let call = self.aux_call(purpose).await;
        let provider = call.provider();
        let capture = crate::engine::AuxCapture::new(&self.event_bus, thread_id, purpose);
        let bus = self.event_bus.clone();
        let effort = call.reasoning().map(str::to_string);
        let deadline = call.deadline();

        tokio::spawn(async move {
            let _in_flight = in_flight;
            let summarized = crate::memory::summarize_conversation(
                provider.as_ref(),
                &body,
                effort.as_deref(),
                &capture,
            );
            let Some(text) = summarize_or_none(summarized, covered_count, deadline).await else {
                return;
            };
            bus.emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::ConversationSummarized {
                        summary: text,
                        covers_through_event_id: boundary,
                        covered_count: covered_count as u32,
                        model: provider.default_model().to_string(),
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Chat] ConversationSummarized",
            )
            .await;
        });
    }
}

const EARLIER_SUMMARISED: &str =
    "[Earlier assistant work (resolved: do NOT re-attempt fixes described here): {}]";

const EARLIER_ASSISTANT_ELIDED: &str = "[Earlier: {} assistant turns before this are not shown. \
     Read them with events(action=\"query\", thread_id=\"current\", \
     event_type=\"ResponseGenerated\").]";

const EARLIER_USER_ELIDED: &str = "[Earlier: {} of the user's own messages before this are not \
     shown. Read them with events(action=\"query\", thread_id=\"current\", \
     event_type=\"MessageReceived\").]";

#[derive(Clone, Copy)]
pub(super) struct CoveredSummary<'a> {
    pub text: &'a str,
    pub boundary: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct SummaryPlan {
    text: Option<String>,
    boundary: Option<usize>,
    uncovered: usize,
}

impl SummaryPlan {
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

    pub(super) fn refresh_boundary(
        &self,
        older: &[crate::core::store::SessionMessage],
        mode: super::context_mode::ContextMode,
    ) -> Option<Uuid> {
        if mode.is_on() || self.uncovered <= HISTORY_SUMMARY_REFRESH_AFTER {
            return None;
        }
        let newest = older.iter().rposition(|m| m.role == "assistant")?;
        older[newest]
            .event_id
            .as_deref()
            .and_then(|id| Uuid::parse_str(id).ok())
    }

    pub(super) fn covered(&self) -> Option<CoveredSummary<'_>> {
        self.text
            .as_deref()
            .zip(self.boundary)
            .map(|(text, boundary)| CoveredSummary { text, boundary })
    }
}

pub(super) fn render_older_region<F>(
    older: &[crate::core::store::SessionMessage],
    covered: Option<CoveredSummary<'_>>,
    format_msg: &F,
) -> String
where
    F: Fn(&crate::core::store::SessionMessage, bool, usize) -> String,
{
    let is_covered = |i: usize| covered.is_some_and(|c| i <= c.boundary);
    let first_covered = (0..older.len()).find(|&i| older[i].role == "assistant" && is_covered(i));

    let (keep_user, dropped_users) = user_turns_within_budget(older);
    let (keep_assistant, dropped_assistant) = uncovered_turns_within_cap(older, &is_covered);

    let elided_note = (dropped_assistant > 0)
        .then(|| EARLIER_ASSISTANT_ELIDED.replace("{}", &dropped_assistant.to_string()));
    let elided_anchor = elided_note
        .as_ref()
        .and_then(|_| (0..older.len()).find(|&i| keep_assistant[i]));

    let mut lines: Vec<String> = Vec::new();
    if dropped_users > 0 {
        lines.push(EARLIER_USER_ELIDED.replace("{}", &dropped_users.to_string()));
    }
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
            lines.push(format_msg(msg, false, i));
        }
    }
    lines.join("\n")
}

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
