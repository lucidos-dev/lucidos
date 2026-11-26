use super::super::types::*;
use super::collect_dismissed_event_ids;
use super::spoken_merge::{is_one_utterance, join_spoken};
use crate::core::changes::ChangeStatus;
use crate::core::EventRow;
use crate::engine::thread_events::{AgentParticipant, MessageOrigin};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use uuid::Uuid;

enum SpokenSide {
    Caller,
    Talker(Option<AgentParticipant>),
}

struct SpokenTail {
    index: usize,
    from_caller: bool,
    created: DateTime<Utc>,
    /// INVARIANT: Which CALL it belonged to. A dropped line redialled within the gap
    /// bound would otherwise glue the end of one call onto the start of the
    /// next: nothing between them reaches `messages`, so adjacency alone reads
    /// them as neighbours.
    session: Option<String>,
    notes_after: usize,
}

fn session_of(event: &EventRow) -> Option<String> {
    event
        .payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn push_spoken(
    messages: &mut Vec<SessionMessage>,
    tail: &mut Option<SpokenTail>,
    side: SpokenSide,
    event: &EventRow,
    text: &str,
    current_thread_id: Option<String>,
) {
    if text.trim().is_empty() {
        return;
    }
    let from_caller = matches!(side, SpokenSide::Caller);
    let session = session_of(event);
    if let Some(prev) = tail.as_mut() {
        let adjacent = prev.index + 1 + prev.notes_after == messages.len();
        let gap = (event.created - prev.created).num_milliseconds() as f64 / 1000.0;
        let same_speaker = prev.from_caller == from_caller && prev.session == session;
        if adjacent && is_one_utterance(gap, same_speaker) {
            let merged = join_spoken(&messages[prev.index].content, text);
            messages[prev.index].content = merged;
            prev.created = event.created;
            return;
        }
    }
    let (role, agent, completed) = match side {
        SpokenSide::Caller => ("user", None, None),
        SpokenSide::Talker(agent) => ("assistant", agent, Some(true)),
    };
    messages.push(SessionMessage {
        role: role.to_string(),
        content: text.trim().to_string(),
        created_at: event.created,
        channel: None,
        steps: vec![],
        image_handles: vec![],
        user_image_hashes: vec![],
        image_description: None,
        completed,
        canceled: false,
        aborted: false,
        text_chunks: vec![],
        events: vec![],
        request_event_id: None,
        event_id: Some(event.id.to_string()),
        thread_id: event
            .thread_id
            .map(|uuid| uuid.to_string())
            .or(current_thread_id),
        agent,
        sender_device_id: None,
    });
    *tail = Some(SpokenTail {
        index: messages.len() - 1,
        from_caller,
        created: event.created,
        session,
        notes_after: 0,
    });
}

fn authoring_agent(event: &EventRow) -> Option<AgentParticipant> {
    serde_json::from_value::<MessageOrigin>(event.payload.get("actor")?.clone())
        .ok()?
        .agent()
        .cloned()
}

pub fn typed_on_device(event_type: &str, payload: &serde_json::Value) -> Option<String> {
    let key = match event_type {
        "MessageReceived" => "origin",
        "UserOnlyMessagePosted" => "actor",
        _ => return None,
    };
    match serde_json::from_value::<MessageOrigin>(payload.get(key)?.clone()).ok()? {
        MessageOrigin::Device { device_id } => Some(device_id),
        _ => None,
    }
}

fn str_field(payload: &serde_json::Value, key: &str) -> String {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn user_image_hashes_of(event: &EventRow) -> Vec<String> {
    event
        .payload
        .get("user_image_hashes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|h| h.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub type ChangeStatuses = HashMap<Uuid, Option<ChangeStatus>>;

/// INVARIANT: The two events a child's report arrives as: a card that woke this thread,
/// or one filed for its next turn. Both carry the same payload.
pub const CHILD_REPORT_EVENT_TYPES: &[&str] = &["ChildThreadCompleted", "ChildReportFiled"];

pub(crate) fn reported_change_ids(events: &[EventRow]) -> Vec<Uuid> {
    let parse = |v: &serde_json::Value| v.as_str().and_then(|s| Uuid::parse_str(s).ok());
    events
        .iter()
        .filter(|event| CHILD_REPORT_EVENT_TYPES.contains(&event.event_type.as_str()))
        .flat_map(|event| {
            let own = event
                .payload
                .get("pending_change_ids")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .filter_map(parse);
            let below = event
                .payload
                .get("sub_thread_pending_changes")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.get("change_id").and_then(parse));
            own.chain(below).collect::<Vec<_>>()
        })
        .collect()
}

fn moved_on_since(statuses: &ChangeStatuses, change_id: Uuid) -> Option<&'static str> {
    match statuses.get(&change_id)? {
        Some(ChangeStatus::Pending) => None,
        Some(ChangeStatus::SetAside) => Some("now set aside"),
        Some(ChangeStatus::Applied) => Some("now applied"),
        Some(ChangeStatus::Discarded) => Some("now discarded"),
        Some(ChangeStatus::Reverted) => Some("now reverted"),
        Some(ChangeStatus::Withdrawn) => Some("now withdrawn"),
        None => Some("now deleted"),
    }
}

pub(crate) const CHANGES_MOVED_ON_NOTE: &str =
    "A change marked \"now ...\" left Review after this report and no longer waits for \
     Apply. Anything in this conversation that calls it pending is out of date.";

pub fn format_child_thread_completed_block(event: &EventRow, statuses: &ChangeStatuses) -> String {
    format_child_report_block(event, statuses, "[CHILD THREAD COMPLETED]", "")
}

pub(crate) const REPORT_FILED_NOTE: &str = "\nThis report did not wake you, so it was filed: \
     its wake mode or the child's own decision said so. You read it now, at the start of \
     your next turn. Act on it if your work needs it.";

pub fn format_child_report_filed_block(event: &EventRow, statuses: &ChangeStatuses) -> String {
    format_child_report_block(
        event,
        statuses,
        "[CHILD THREAD REPORT FILED]",
        REPORT_FILED_NOTE,
    )
}

fn format_child_report_block(
    event: &EventRow,
    statuses: &ChangeStatuses,
    heading: &str,
    note: &str,
) -> String {
    use crate::engine::thread_events::ChildCompletionStatus;

    let child_thread_id = event
        .payload
        .get("child_thread_id")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let title = event
        .payload
        .get("child_thread_title")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    // INVARIANT: Exhaustive match against the typed enum so a new ChildCompletionStatus
    // variant becomes a compile error here, not a silent fall-through to
    // bare "completed".
    let status = match serde_json::from_value::<ChildCompletionStatus>(
        event
            .payload
            .get("status")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    ) {
        Ok(ChildCompletionStatus::Success) => "completed (success)",
        Ok(ChildCompletionStatus::Failure) => "completed (failure)",
        Ok(ChildCompletionStatus::NoChanges) => "completed (no changes)",
        Ok(ChildCompletionStatus::Canceled) => "canceled",
        Ok(ChildCompletionStatus::Interrupted) => "interrupted by an engine restart",
        Err(_) => "completed",
    };
    let summary = event
        .payload
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let pending_change_ids: Vec<String> = event
        .payload
        .get("pending_change_ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let pending_section = if pending_change_ids.is_empty() {
        "none".to_string()
    } else {
        pending_change_ids
            .iter()
            .map(|id| {
                match Uuid::parse_str(id)
                    .ok()
                    .and_then(|uuid| moved_on_since(statuses, uuid))
                {
                    Some(since) => format!("{id} ({since})"),
                    None => id.clone(),
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let sub_thread_section = sub_thread_pending_section(event, statuses);
    let any_moved_on = reported_change_ids(std::slice::from_ref(event))
        .iter()
        .any(|id| moved_on_since(statuses, *id).is_some());
    let moved_on_note = if any_moved_on {
        format!("\n{CHANGES_MOVED_ON_NOTE}")
    } else {
        String::new()
    };
    let title_line = if title.is_empty() {
        String::new()
    } else {
        format!("\nTitle: {}", title)
    };
    let summary_section = if summary.is_empty() {
        String::new()
    } else {
        format!("\nSummary: {}", summary)
    };
    format!(
        "{} {} {}\nevent_id: {}{}\nPending changes: {}{}{}{}{}\n\
         Note: phrases like \"session can finish\" or \"## Session Summary\" in \
         the summary describe the child subprocess only — if you were following \
         a multi-step procedure, continue with the next step. Otherwise use \
         run_thread to refine.{}",
        heading,
        child_thread_id,
        status,
        event.id,
        title_line,
        pending_section,
        sub_thread_section,
        integrated_branch_section(event),
        moved_on_note,
        summary_section,
        note
    )
}

fn integrated_branch_section(event: &EventRow) -> String {
    let Some(branch) = event
        .payload
        .get("integrated_branch")
        .cloned()
        .and_then(|v| {
            serde_json::from_value::<crate::engine::thread_events::IntegratedBranch>(v).ok()
        })
    else {
        return String::new();
    };
    format!(
        "\nIntegrated branch: {} at {}. This child is integrated into your branch, so it \
         proposes nothing. Merge it now with `git merge {}`, then run `/harden` once over the \
         merged work.",
        branch.branch_name, branch.head_sha, branch.branch_name
    )
}

fn sub_thread_pending_section(event: &EventRow, statuses: &ChangeStatuses) -> String {
    use crate::engine::thread_events::SubThreadPendingChange;

    let entries: Vec<SubThreadPendingChange> = event
        .payload
        .get("sub_thread_pending_changes")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    if entries.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = entries
        .iter()
        .map(|entry| {
            let state = match moved_on_since(statuses, entry.change_id) {
                Some(since) => since,
                None if entry.thread_unsettled => "still working",
                None => "settled",
            };
            let title = entry.thread_title.as_deref().unwrap_or(UNTITLED_SUB_THREAD);
            format!(
                "\n- {} from sub-thread \"{}\" ({}): {}",
                entry.change_id, title, entry.thread_id, state
            )
        })
        .collect();
    format!(
        "\nPending changes in its sub-threads (state when this card was sent):{}",
        lines.concat()
    )
}

const UNTITLED_SUB_THREAD: &str = "untitled";

pub fn format_child_thread_stopped_block(event: &EventRow) -> String {
    let child_thread_id = event
        .payload
        .get("child_thread_id")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let title_line = match event
        .payload
        .get("child_thread_title")
        .and_then(|v| v.as_str())
    {
        Some(title) if !title.is_empty() => format!("\nTitle: {title}"),
        _ => String::new(),
    };
    format!(
        "[CHILD THREAD STOPPED] {child_thread_id}\nevent_id: {}{title_line}\n\
         The user stopped this child's turn. The child is NOT finished and NOT \
         dead: it is waiting for the user, who may send it a new message. You \
         will get a [CHILD THREAD COMPLETED] block when it next finishes, or one \
         with status canceled if the user archives or discards it. Until then, \
         do not roll back its work, respawn it, or send it a follow-up.",
        event.id
    )
}

pub fn format_child_thread_detached_block(event: &EventRow) -> String {
    let child_thread_id = event
        .payload
        .get("child_thread_id")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let title_line = match event
        .payload
        .get("child_thread_title")
        .and_then(|v| v.as_str())
    {
        Some(title) if !title.is_empty() => format!("\nTitle: {title}"),
        _ => String::new(),
    };
    format!(
        "[CHILD THREAD MOVED OUT] {child_thread_id}\nevent_id: {}{title_line}\n\
         This child was moved to top level. It keeps running on its own, but it is \
         no longer your child: you will not get its result, and you cannot follow up \
         on it. Do not wait for it or respawn it.",
        event.id
    )
}

fn thread_image_handles_of(event: &EventRow) -> Vec<String> {
    crate::core::events::thread_image_refs_of(&event.event_type, &event.payload)
        .into_iter()
        .map(|(_, image_ref)| crate::core::events::image_handle(image_ref))
        .collect()
}

#[cfg(any(test, feature = "test-support"))]
pub fn build_session_messages(events: &[EventRow]) -> Vec<SessionMessage> {
    build_session_messages_with(events, &ChangeStatuses::new())
}

/// INVARIANT: Build session messages from a list of events (pure function, no DB access).
///
/// `statuses` says what became of each change a child report names. Load it
/// with `EventStore::build_messages_now` rather than passing an empty map.
///
/// Interruption semantics:
/// - `completed: Some(false)` — the response was interrupted by a follow-up user message
///   arriving mid-stream (a `MessageReceived` event flushed the text buffer before the
///   response could complete with `ResponseGenerated`).
/// - `completed: None` — still in progress or unknown (trailing buffer flush at end of events).
/// - `completed: Some(true)` — completed normally (`ResponseGenerated` or `ResponseFailed`).
pub(crate) fn build_session_messages_with(
    events: &[EventRow],
    statuses: &ChangeStatuses,
) -> Vec<SessionMessage> {
    let mut messages: Vec<SessionMessage> = Vec::new();
    let mut pending_steps: Vec<Step> = Vec::new();
    let mut pending_image_handles: Vec<String> = Vec::new();
    let mut claude_code_text_buf = String::new();
    let mut claude_code_text_last_ts: Option<DateTime<Utc>> = None;
    let mut text_buf = String::new();
    let mut text_last_ts: Option<DateTime<Utc>> = None;
    let mut pending_text_chunks: Vec<String> = Vec::new();
    let mut last_cc_chunk_len: usize = 0;
    let mut last_text_chunk_len: usize = 0;
    let mut pending_events: Vec<ResponseEvent> = Vec::new();
    let mut last_cc_event_len: usize = 0;
    let mut last_text_event_len: usize = 0;
    let mut current_request_event_id: Option<String> = None;
    let mut current_thread_id: Option<String> = None;
    let mut spoken_tail: Option<SpokenTail> = None;
    let mut turn_open = false;
    let mut held_users: Vec<SessionMessage> = Vec::new();

    let dismissed_event_ids = collect_dismissed_event_ids(events);

    let image_descriptions = collect_image_descriptions(events);

    let get_thread_id =
        |event: &EventRow| -> Option<String> { event.thread_id.map(|uuid| uuid.to_string()) };

    for event in events {
        // INVARIANT: Skip dismissed entries before any per-event handling so accumulated
        // text buffers / pending steps stay untouched. Today this only fires
        // for `ChildThreadCompleted` (`ToolCalled` + `ToolResult` skipping
        // happens at the resume helper level so the projection can still show
        // step rows in the UI).
        if event.event_type == "ChildThreadCompleted"
            && dismissed_event_ids.contains(&event.id.to_string())
        {
            continue;
        }
        match event.event_type.as_str() {
            "MessageReceived" | "UserMessage" => {
                if !claude_code_text_buf.is_empty() {
                    if claude_code_text_buf.len() > last_cc_chunk_len {
                        pending_text_chunks
                            .push(claude_code_text_buf[last_cc_chunk_len..].to_string());
                    }
                    if claude_code_text_buf.len() > last_cc_event_len {
                        pending_events.push(ResponseEvent::Text {
                            md: claude_code_text_buf[last_cc_event_len..].to_string(),
                        });
                    }
                    let created_at = claude_code_text_last_ts.take().unwrap_or(event.created);
                    messages.push(SessionMessage {
                        role: "assistant".to_string(),
                        content: std::mem::take(&mut claude_code_text_buf),
                        created_at,
                        channel: Some("claude_code".to_string()),
                        steps: std::mem::take(&mut pending_steps),
                        image_handles: std::mem::take(&mut pending_image_handles),
                        user_image_hashes: vec![],
                        image_description: None,
                        completed: Some(false),
                        canceled: false,
                        aborted: false,
                        text_chunks: std::mem::take(&mut pending_text_chunks),
                        events: std::mem::take(&mut pending_events),

                        request_event_id: current_request_event_id.clone(),
                        event_id: None,
                        thread_id: current_thread_id.clone(),
                        agent: None,
                        sender_device_id: None,
                    });
                    last_cc_chunk_len = 0;
                    last_cc_event_len = 0;
                }
                if !text_buf.is_empty() {
                    if text_buf.len() > last_text_chunk_len {
                        pending_text_chunks.push(text_buf[last_text_chunk_len..].to_string());
                    }
                    if text_buf.len() > last_text_event_len {
                        pending_events.push(ResponseEvent::Text {
                            md: text_buf[last_text_event_len..].to_string(),
                        });
                    }
                    let created_at = text_last_ts.take().unwrap_or(event.created);
                    messages.push(SessionMessage {
                        role: "assistant".to_string(),
                        content: std::mem::take(&mut text_buf),
                        created_at,
                        channel: None,
                        steps: std::mem::take(&mut pending_steps),
                        image_handles: std::mem::take(&mut pending_image_handles),
                        user_image_hashes: vec![],
                        image_description: None,
                        completed: Some(false),
                        canceled: false,
                        aborted: false,
                        text_chunks: std::mem::take(&mut pending_text_chunks),
                        events: std::mem::take(&mut pending_events),

                        request_event_id: current_request_event_id.clone(),
                        event_id: None,
                        thread_id: current_thread_id.clone(),
                        agent: None,
                        sender_device_id: None,
                    });
                    last_text_chunk_len = 0;
                    last_text_event_len = 0;
                }

                let content = str_field(&event.payload, "text");
                let user_image_hashes = user_image_hashes_of(event);

                pending_steps.clear();
                pending_image_handles.clear();
                pending_events.clear();

                let image_description = image_descriptions
                    .get(&event.id.to_string())
                    .cloned()
                    .or_else(|| {
                        event
                            .payload
                            .get("image_description")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    });

                let channel = event
                    .payload
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                current_request_event_id = Some(event.id.to_string());
                current_thread_id = get_thread_id(event);
                messages.append(&mut held_users);
                turn_open = true;

                messages.push(SessionMessage {
                    role: "user".to_string(),
                    content,
                    created_at: event.created,
                    channel,
                    steps: vec![],
                    image_handles: thread_image_handles_of(event),
                    user_image_hashes,
                    image_description,
                    completed: None,
                    canceled: false,
                    aborted: false,
                    text_chunks: vec![],
                    events: vec![],
                    request_event_id: None,
                    event_id: Some(event.id.to_string()),
                    thread_id: current_thread_id.clone(),
                    agent: None,
                    sender_device_id: typed_on_device(&event.event_type, &event.payload),
                });
            }
            "UserOnlyMessagePosted" => {
                let target = match turn_open {
                    true => &mut held_users,
                    false => &mut messages,
                };
                target.push(SessionMessage {
                    role: "user".to_string(),
                    content: str_field(&event.payload, "text"),
                    created_at: event.created,
                    channel: None,
                    steps: vec![],
                    image_handles: thread_image_handles_of(event),
                    user_image_hashes: user_image_hashes_of(event),
                    image_description: image_descriptions.get(&event.id.to_string()).cloned(),
                    completed: None,
                    canceled: false,
                    aborted: false,
                    text_chunks: vec![],
                    events: vec![],
                    request_event_id: None,
                    event_id: Some(event.id.to_string()),
                    thread_id: get_thread_id(event),
                    agent: None,
                    sender_device_id: typed_on_device(&event.event_type, &event.payload),
                });
            }
            // INVARIANT: "Thinking" is the legacy DB string; T7 renamed the variant to
            // ThoughtStreamed. New rows store "ThoughtStreamed"; old rows are
            // unchanged. Both must hit this arm so legacy threads still render.
            "Thinking" | "ThoughtStreamed" => {
                let context_tokens = event
                    .payload
                    .get("context_tokens")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                let context_messages = event
                    .payload
                    .get("context_messages")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                let trimmed = event.payload.get("trimmed").and_then(|v| v.as_bool());
                pending_steps.push(Step {
                    description: "Requesting".to_string(),
                    tool_name: None,
                    success: true,
                    context_tokens,
                    context_messages,
                    trimmed,
                    tool_called_event_id: None,
                });
                pending_events.push(ResponseEvent::Step {
                    description: "Requesting".to_string(),
                    tool_name: None,
                    success: true,
                    detail: None,
                    context_tokens,
                    context_messages,
                    trimmed,
                    tool_called_event_id: None,
                });
            }
            // INVARIANT: "MemorySearched" is the legacy DB string; the variant was renamed
            // to MemoryRecalled so it stops mirroring the `memory` tool's own
            // "Searching memory" step. New rows store "MemoryRecalled"; old
            // rows are unchanged. Both must hit this arm so legacy threads
            // still render.
            "MemorySearched" | "MemoryRecalled" => {
                let results = event
                    .payload
                    .get("results")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let queries: Vec<String> = event
                    .payload
                    .get("queries")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let desc = crate::core::store::memory_recalled_label(results);
                let detail = if queries.is_empty() {
                    None
                } else {
                    Some(queries.join(", "))
                };
                pending_steps.push(Step {
                    description: desc.clone(),
                    tool_name: None,
                    success: true,
                    context_tokens: None,
                    context_messages: None,
                    trimmed: None,
                    tool_called_event_id: None,
                });
                pending_events.push(ResponseEvent::Step {
                    description: desc,
                    tool_name: None,
                    success: true,
                    detail,
                    context_tokens: None,
                    context_messages: None,
                    trimmed: None,
                    tool_called_event_id: None,
                });
            }
            "ToolCalled" => {
                if claude_code_text_buf.len() > last_cc_chunk_len {
                    let delta = &claude_code_text_buf[last_cc_chunk_len..];
                    if delta.len() >= 80 {
                        pending_text_chunks.push(delta.to_string());
                        last_cc_chunk_len = claude_code_text_buf.len();
                    }
                } else if text_buf.len() > last_text_chunk_len {
                    let delta = &text_buf[last_text_chunk_len..];
                    if delta.len() >= 80 {
                        pending_text_chunks.push(delta.to_string());
                        last_text_chunk_len = text_buf.len();
                    }
                }

                // INVARIANT: For events: snapshot text delta (no minimum size — interleaving
                // with step events matters more than avoiding small text blocks)
                if claude_code_text_buf.len() > last_cc_event_len {
                    let delta = &claude_code_text_buf[last_cc_event_len..];
                    pending_events.push(ResponseEvent::Text {
                        md: delta.to_string(),
                    });
                    last_cc_event_len = claude_code_text_buf.len();
                } else if text_buf.len() > last_text_event_len {
                    let delta = &text_buf[last_text_event_len..];
                    pending_events.push(ResponseEvent::Text {
                        md: delta.to_string(),
                    });
                    last_text_event_len = text_buf.len();
                }

                let (tool_name, description) = super::super::describe_tool_event(event);
                pending_steps.push(Step {
                    description: description.clone(),
                    tool_name: Some(tool_name.clone()),
                    success: true,
                    context_tokens: None,
                    context_messages: None,
                    trimmed: None,
                    tool_called_event_id: Some(event.id.to_string()),
                });
                pending_events.push(ResponseEvent::Step {
                    description,
                    tool_name: Some(tool_name),
                    success: true,
                    detail: None,
                    context_tokens: None,
                    context_messages: None,
                    trimmed: None,
                    tool_called_event_id: Some(event.id.to_string()),
                });
            }
            "ToolResult" => {
                let success = event
                    .payload
                    .get("success")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true);

                let call_id =
                    super::tool_called_event_id_of(&event.payload).map(|id| id.to_string());
                let answers = |step_call: &Option<String>| match &call_id {
                    Some(id) => step_call.as_deref() == Some(id.as_str()),
                    None => true,
                };
                if let Some(step) = pending_steps
                    .iter_mut()
                    .rev()
                    .find(|step| answers(&step.tool_called_event_id))
                {
                    step.success = success;
                }

                let tool_name = event
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let result_text = event
                    .payload
                    .get("result")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let detail =
                    super::super::super::describe_tool_result(tool_name, result_text, success);

                if let Some(ResponseEvent::Step {
                    success: ref mut s,
                    detail: ref mut d,
                    ..
                }) = pending_events.iter_mut().rev().find(|e| match e {
                    ResponseEvent::Step {
                        tool_called_event_id,
                        ..
                    } => answers(tool_called_event_id),
                    _ => false,
                }) {
                    *s = success;
                    *d = detail;
                }

                pending_image_handles.extend(thread_image_handles_of(event));
            }
            "CodingAgentTextStreamed" | "TextStreamed" => {
                if let Some(text) = event.payload.get("text").and_then(|v| v.as_str()) {
                    let is_cc = event.event_type == "CodingAgentTextStreamed"
                        || event.payload.get("channel").and_then(|v| v.as_str())
                            == Some("claude_code");
                    if is_cc {
                        claude_code_text_buf.push_str(text);
                        claude_code_text_last_ts = Some(event.created);
                    } else {
                        text_buf.push_str(text);
                        text_last_ts = Some(event.created);
                    }
                }
            }
            "ResponseAborted" | "ResponseCanceled" | "ResponseGenerated" | "AssistantResponse" => {
                let is_canceled = event.event_type == "ResponseCanceled";
                let is_aborted = event.event_type == "ResponseAborted";

                let result_text = event
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if claude_code_text_buf.len() > last_cc_chunk_len {
                    pending_text_chunks.push(claude_code_text_buf[last_cc_chunk_len..].to_string());
                    last_cc_chunk_len = claude_code_text_buf.len();
                } else if text_buf.len() > last_text_chunk_len {
                    pending_text_chunks.push(text_buf[last_text_chunk_len..].to_string());
                    last_text_chunk_len = text_buf.len();
                }

                if claude_code_text_buf.len() > last_cc_event_len {
                    pending_events.push(ResponseEvent::Text {
                        md: claude_code_text_buf[last_cc_event_len..].to_string(),
                    });
                } else if text_buf.len() > last_text_event_len {
                    pending_events.push(ResponseEvent::Text {
                        md: text_buf[last_text_event_len..].to_string(),
                    });
                }

                let is_cc_channel =
                    event.payload.get("channel").and_then(|v| v.as_str()) == Some("claude_code");
                if is_cc_channel && !result_text.is_empty() && !claude_code_text_buf.is_empty() {
                    let buf = claude_code_text_buf.trim();
                    let result = result_text.trim();
                    if result.len() > buf.len() && result.starts_with(buf) {
                        let extra = result[buf.len()..].trim();
                        if !extra.is_empty() {
                            pending_events.push(ResponseEvent::Text {
                                md: extra.to_string(),
                            });
                        }
                    }
                } else if !is_cc_channel && !result_text.is_empty() && !text_buf.is_empty() {
                    let buf = text_buf.trim();
                    let result = result_text.trim();
                    if result.len() > buf.len() && result.starts_with(buf) {
                        let extra = result[buf.len()..].trim();
                        if !extra.is_empty() {
                            pending_events.push(ResponseEvent::Text {
                                md: extra.to_string(),
                            });
                        }
                    }
                } else if !result_text.is_empty()
                    && pending_events
                        .iter()
                        .any(|e| matches!(e, ResponseEvent::Step { .. }))
                    && !pending_events
                        .iter()
                        .any(|e| matches!(e, ResponseEvent::Text { .. }))
                {
                    pending_events.push(ResponseEvent::Text {
                        md: result_text.clone(),
                    });
                }

                let channel = event
                    .payload
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let content = if channel.as_deref() == Some("claude_code")
                    && !claude_code_text_buf.is_empty()
                {
                    if result_text.starts_with(claude_code_text_buf.trim())
                        || claude_code_text_buf.trim() == result_text.trim()
                    {
                        claude_code_text_buf.clear();
                        claude_code_text_last_ts = None;
                        last_cc_chunk_len = 0;
                        result_text
                    } else {
                        let full = format!("{}\n\n{}", claude_code_text_buf, result_text);
                        claude_code_text_buf.clear();
                        claude_code_text_last_ts = None;
                        last_cc_chunk_len = 0;
                        full
                    }
                } else if !text_buf.is_empty() {
                    if result_text.starts_with(text_buf.trim())
                        || text_buf.trim() == result_text.trim()
                    {
                        text_buf.clear();
                        text_last_ts = None;
                        last_text_chunk_len = 0;
                        result_text
                    } else {
                        let full = format!("{}\n\n{}", text_buf, result_text);
                        text_buf.clear();
                        text_last_ts = None;
                        last_text_chunk_len = 0;
                        full
                    }
                } else {
                    result_text
                };

                let user_eid = event
                    .payload
                    .get("request_event_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| current_request_event_id.clone());

                messages.push(SessionMessage {
                    role: "assistant".to_string(),
                    content,
                    created_at: event.created,
                    channel,
                    steps: std::mem::take(&mut pending_steps),
                    image_handles: std::mem::take(&mut pending_image_handles),
                    user_image_hashes: vec![],
                    image_description: None,
                    completed: Some(true),
                    canceled: is_canceled,
                    aborted: is_aborted,
                    text_chunks: std::mem::take(&mut pending_text_chunks),
                    events: std::mem::take(&mut pending_events),

                    request_event_id: user_eid,
                    event_id: Some(event.id.to_string()),
                    thread_id: get_thread_id(event).or_else(|| current_thread_id.clone()),
                    agent: authoring_agent(event),
                    sender_device_id: None,
                });
                last_cc_event_len = 0;
                last_text_event_len = 0;
                messages.append(&mut held_users);
                turn_open = false;
            }
            "ResponseFailed" => {
                let error = event
                    .payload
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error");

                let user_eid = event
                    .payload
                    .get("request_event_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| current_request_event_id.clone());

                let error_content = format!("[ERROR] **Error:** {}", error);
                let mut events = std::mem::take(&mut pending_events);
                if !events.is_empty() {
                    events.push(ResponseEvent::Text {
                        md: error_content.clone(),
                    });
                }
                messages.push(SessionMessage {
                    role: "assistant".to_string(),
                    content: error_content,
                    created_at: event.created,
                    channel: None,
                    steps: std::mem::take(&mut pending_steps),
                    image_handles: std::mem::take(&mut pending_image_handles),
                    user_image_hashes: vec![],
                    image_description: None,
                    completed: Some(true),
                    canceled: false,
                    aborted: false,
                    text_chunks: std::mem::take(&mut pending_text_chunks),
                    events,

                    request_event_id: user_eid,
                    event_id: Some(event.id.to_string()),
                    thread_id: get_thread_id(event).or_else(|| current_thread_id.clone()),
                    agent: None,
                    sender_device_id: None,
                });
                messages.append(&mut held_users);
                turn_open = false;
            }
            "TriggerStarted" => {
                let prompt = event
                    .payload
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                pending_steps.clear();
                pending_image_handles.clear();
                pending_events.clear();

                let channel = event
                    .payload
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                current_request_event_id = Some(event.id.to_string());
                current_thread_id = get_thread_id(event);
                messages.append(&mut held_users);
                turn_open = true;

                messages.push(SessionMessage {
                    role: "user".to_string(),
                    content: prompt,
                    created_at: event.created,
                    channel,
                    steps: vec![],
                    image_handles: vec![],
                    user_image_hashes: vec![],
                    image_description: None,
                    completed: None,
                    canceled: false,
                    aborted: false,
                    text_chunks: vec![],
                    events: vec![],

                    request_event_id: None,
                    event_id: Some(event.id.to_string()),
                    thread_id: current_thread_id.clone(),
                    agent: None,
                    sender_device_id: None,
                });
            }
            "TriggerCompleted" => {
                // INVARIANT: TriggerCompleted is a bookkeeping event — the actual response
                // is already captured by the ResponseGenerated event in the same session.
                // Attach any pending steps to the existing assistant message instead of
                // creating a duplicate that would overwrite it.
                if !pending_steps.is_empty() {
                    if let Some(last_assistant) =
                        messages.iter_mut().rev().find(|m| m.role == "assistant")
                    {
                        last_assistant.steps.append(&mut pending_steps);
                    } else {
                        pending_steps.clear();
                    }
                }
            }
            "ChildThreadCompleted" | "ChildReportFiled" => {
                let content = if event.event_type == "ChildThreadCompleted" {
                    format_child_thread_completed_block(event, statuses)
                } else {
                    format_child_report_filed_block(event, statuses)
                };

                messages.push(SessionMessage {
                    role: "user".to_string(),
                    content,
                    created_at: event.created,
                    channel: None,
                    steps: vec![],
                    image_handles: vec![],
                    user_image_hashes: vec![],
                    image_description: None,
                    completed: None,
                    canceled: false,
                    aborted: false,
                    text_chunks: vec![],
                    events: vec![],
                    request_event_id: None,
                    event_id: Some(event.id.to_string()),
                    thread_id: get_thread_id(event).or_else(|| current_thread_id.clone()),
                    agent: None,
                    sender_device_id: None,
                });
            }
            "ChildThreadStopped" | "ChildThreadDetached" => {
                let content = if event.event_type == "ChildThreadStopped" {
                    format_child_thread_stopped_block(event)
                } else {
                    format_child_thread_detached_block(event)
                };
                messages.push(SessionMessage {
                    role: "user".to_string(),
                    content,
                    created_at: event.created,
                    channel: None,
                    steps: vec![],
                    image_handles: vec![],
                    user_image_hashes: vec![],
                    image_description: None,
                    completed: None,
                    canceled: false,
                    aborted: false,
                    text_chunks: vec![],
                    events: vec![],
                    request_event_id: None,
                    event_id: Some(event.id.to_string()),
                    thread_id: get_thread_id(event).or_else(|| current_thread_id.clone()),
                    agent: None,
                    sender_device_id: None,
                });
            }
            "SpokenReplyGenerated" => {
                // INVARIANT: What the talker said out loud. It reaches the doer under
                // the talker's own speaker label. So the doer reads what
                // was already said in its name, never as its own turn.
                //
                // It consumes NOTHING pending. A spoken reply lands mid-call,
                // often while the doer's turn is still running, and those
                // steps and text belong to that turn.
                let text = event
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                push_spoken(
                    &mut messages,
                    &mut spoken_tail,
                    SpokenSide::Talker(authoring_agent(event)),
                    event,
                    text,
                    current_thread_id.clone(),
                );
            }
            "SpokenMessageReceived" => {
                // INVARIANT: Something the caller said on a call. It started no turn, so
                // it is not a `MessageReceived`. The doer still has to read it:
                // the next question can lean on it.
                //
                // A `user` message, because the caller said it. Consumes
                // nothing pending, exactly as a spoken reply does.
                let text = event
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                push_spoken(
                    &mut messages,
                    &mut spoken_tail,
                    SpokenSide::Caller,
                    event,
                    text,
                    current_thread_id.clone(),
                );
            }
            "WorkDelegated" => {
                let reason = event
                    .payload
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if !reason.trim().is_empty() {
                    messages.push(SessionMessage {
                        role: "assistant".to_string(),
                        content: format!("[Asked for you] {}", reason.trim()),
                        created_at: event.created,
                        channel: None,
                        steps: vec![],
                        image_handles: vec![],
                        user_image_hashes: vec![],
                        image_description: None,
                        completed: Some(true),
                        canceled: false,
                        aborted: false,
                        text_chunks: vec![],
                        events: vec![],
                        request_event_id: None,
                        event_id: Some(event.id.to_string()),
                        thread_id: get_thread_id(event).or_else(|| current_thread_id.clone()),
                        agent: authoring_agent(event),
                        sender_device_id: None,
                    });
                    if let Some(tail) = spoken_tail.as_mut() {
                        if tail.from_caller {
                            tail.notes_after += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // INVARIANT: Flush any remaining streaming text or pending events as a single assistant message.
    // Both text_buf (outer LLM preamble) and claude_code_text_buf (CC output) may
    // have content when the user reloads mid-Claude Code session. They're part of the same
    // response and must be combined — creating separate messages would orphan one.
    // Also flush when pending_events exist without text (e.g. CC made tool calls
    // before sending any text) — without this, step events are lost on reload and
    // the frontend has no events to show in the More/Less and Steps toggles.
    // completed: None — still in progress, not an interruption.
    let has_cc_text = !claude_code_text_buf.is_empty();
    let has_text = !text_buf.is_empty();
    let has_pending_events = !pending_events.is_empty();
    if has_cc_text || has_text || has_pending_events {
        if claude_code_text_buf.len() > last_cc_chunk_len {
            pending_text_chunks.push(claude_code_text_buf[last_cc_chunk_len..].to_string());
        }
        if text_buf.len() > last_text_chunk_len {
            pending_text_chunks.push(text_buf[last_text_chunk_len..].to_string());
        }
        if claude_code_text_buf.len() > last_cc_event_len {
            pending_events.push(ResponseEvent::Text {
                md: claude_code_text_buf[last_cc_event_len..].to_string(),
            });
        }
        if text_buf.len() > last_text_event_len {
            pending_events.push(ResponseEvent::Text {
                md: text_buf[last_text_event_len..].to_string(),
            });
        }

        let content = if has_text && has_cc_text {
            format!("{}\n\n{}", text_buf, claude_code_text_buf)
        } else if has_cc_text {
            claude_code_text_buf.clone()
        } else if has_text {
            text_buf.clone()
        } else {
            String::new()
        };
        let channel = if has_cc_text || (!has_text && has_pending_events) {
            Some("claude_code".to_string())
        } else {
            None
        };
        let created_at = claude_code_text_last_ts
            .or(text_last_ts)
            .unwrap_or_else(|| messages.last().map(|m| m.created_at).unwrap_or_default());

        claude_code_text_buf.clear();
        text_buf.clear();

        messages.push(SessionMessage {
            role: "assistant".to_string(),
            content,
            created_at,
            channel,
            steps: std::mem::take(&mut pending_steps),
            image_handles: std::mem::take(&mut pending_image_handles),
            user_image_hashes: vec![],
            image_description: None,
            completed: None,
            canceled: false,
            aborted: false,
            text_chunks: std::mem::take(&mut pending_text_chunks),
            events: std::mem::take(&mut pending_events),
            request_event_id: current_request_event_id.clone(),
            event_id: None,
            thread_id: current_thread_id.clone(),
            agent: None,
            sender_device_id: None,
        });
    }

    if !pending_steps.is_empty() {
        if let Some(last_user) = messages.iter_mut().rev().find(|m| m.role == "user") {
            last_user.steps = pending_steps;
        }
    }

    let held_at = match messages.last() {
        Some(last) if last.role == "assistant" && last.completed.is_none() => messages.len() - 1,
        _ => messages.len(),
    };
    messages.splice(held_at..held_at, held_users);

    messages
}

pub fn newest_conversation_summary(events: &[EventRow]) -> Option<CachedSummary> {
    let mut found: Option<CachedSummary> = None;
    for event in events {
        if event.event_type != "ConversationSummarized" {
            continue;
        }
        let Some(summary) = event
            .payload
            .get("summary")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        else {
            continue;
        };
        let Some(covers_through) = event
            .payload
            .get("covers_through_event_id")
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        found = Some(CachedSummary {
            summary: summary.to_string(),
            covers_through_event_id: covers_through.to_string(),
        });
    }
    found
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedSummary {
    pub summary: String,
    pub covers_through_event_id: String,
}

fn collect_image_descriptions(events: &[EventRow]) -> std::collections::HashMap<String, String> {
    let mut by_source: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for event in events {
        if event.event_type != "ImageDescribed" {
            continue;
        }
        let Some(source_id) = event
            .payload
            .get("source_event_id")
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        let Some(desc) = event.payload.get("description").and_then(|v| v.as_str()) else {
            continue;
        };
        // INVARIANT: Last write wins — `events` arrives in chronological order from
        // the loader, so the most recently emitted description survives.
        by_source.insert(source_id.to_string(), desc.to_string());
    }
    by_source
}
