//! The log entry projection, per channel.

use serde_json::json;
use uuid::Uuid;

use super::event;
use crate::engine::summary_tree::log::{
    cap, project, EntryKind, StoredEvent, ThreadKind, ENTRY_EVENT_TYPES, TURN_END_EVENT_TYPES,
};
use crate::engine::thread_events::ActorMode;

fn stored(values: Vec<serde_json::Value>) -> Vec<StoredEvent> {
    values
        .into_iter()
        .map(|v| StoredEvent {
            id: Uuid::new_v4(),
            event: event(v),
        })
        .collect()
}

fn kinds_and_texts(kind: ThreadKind, values: Vec<serde_json::Value>) -> Vec<(EntryKind, String)> {
    project(kind, &stored(values))
        .entries
        .into_iter()
        .map(|e| (e.kind, e.text))
        .collect()
}

/// A Lucidos Agent thread logs everything: the user, the tool call, its
/// result and the reply. The reply's `TextStreamed` and `ResponseGenerated`
/// carry the same text, and it is logged once.
#[test]
fn a_lucidos_agent_turn_logs_at_full_fidelity() {
    let entries = kinds_and_texts(
        ThreadKind::LucidosAgent,
        vec![
            json!({"type": "MessageReceived", "text": "find the plan", "mode": "human"}),
            json!({"type": "ToolCalled", "name": "read_file", "args": {"path": "plan.md"}}),
            json!({"type": "ToolResult", "name": "read_file", "result": "# Plan"}),
            json!({"type": "TextStreamed", "text": "The plan has one heading."}),
            json!({"type": "ResponseGenerated", "text": "The plan has one heading."}),
        ],
    );
    assert_eq!(
        entries,
        vec![
            (
                EntryKind::Prompt(ActorMode::Human),
                "find the plan".to_string()
            ),
            (
                EntryKind::Call,
                r#"read_file {"path":"plan.md"}"#.to_string()
            ),
            (EntryKind::Result, "[read_file] # Plan".to_string()),
            (EntryKind::Response, "The plan has one heading.".to_string()),
        ]
    );
}

/// The loop persists a round's text once per paragraph. The deltas of a
/// round before a tool call are one entry, and the final round is the
/// response, logged once.
#[test]
fn a_rounds_text_deltas_are_one_entry() {
    let entries = kinds_and_texts(
        ThreadKind::LucidosAgent,
        vec![
            json!({"type": "MessageReceived", "text": "go", "mode": "human"}),
            json!({"type": "TextStreamed", "text": "First, "}),
            json!({"type": "TextStreamed", "text": "the plan.\n\n"}),
            json!({"type": "ToolCalled", "name": "read_file", "args": {}}),
            json!({"type": "ToolResult", "name": "read_file", "result": "ok"}),
            json!({"type": "TextStreamed", "text": "P1\n\n"}),
            json!({"type": "TextStreamed", "text": "P2"}),
            json!({"type": "ResponseGenerated", "text": "P1\n\nP2"}),
        ],
    );
    assert_eq!(
        entries,
        vec![
            (EntryKind::Prompt(ActorMode::Human), "go".to_string()),
            (EntryKind::Response, "First, the plan.".to_string()),
            (EntryKind::Call, "read_file {}".to_string()),
            (EntryKind::Result, "[read_file] ok".to_string()),
            (EntryKind::Response, "P1\n\nP2".to_string()),
        ]
    );
}

/// A coding-agent turn logs one prompt and one reply. Its tool traffic and the
/// narration between tool calls stay out.
#[test]
fn a_coding_agent_turn_logs_one_prompt_and_one_reply() {
    let entries = kinds_and_texts(
        ThreadKind::CodingAgent,
        vec![
            json!({"type": "MessageReceived", "text": "fix the bug", "mode": "human"}),
            json!({"type": "CodingAgentTextStreamed", "text": "Let me look."}),
            json!({"type": "CodingAgentToolCalled", "name": "Read", "args": {}}),
            json!({"type": "CodingAgentToolResult", "name": "Read", "result": "code"}),
            json!({"type": "CodingAgentTextStreamed", "text": "Fixed it."}),
            json!({"type": "ResponseGenerated", "text": "Fixed it, and the tests pass."}),
            json!({"type": "CodingAgentIdled"}),
        ],
    );
    assert_eq!(
        entries,
        vec![
            (
                EntryKind::Prompt(ActorMode::Human),
                "fix the bug".to_string()
            ),
            (
                EntryKind::Response,
                "Fixed it, and the tests pass.".to_string()
            ),
        ]
    );
}

/// A question splits the reply, and the answer is named by its label: the
/// owner's decision is worth its own entry.
#[test]
fn a_coding_agent_question_and_its_answer_are_entries() {
    let entries = kinds_and_texts(
        ThreadKind::CodingAgent,
        vec![
            json!({"type": "MessageReceived", "text": "plan it", "mode": "human"}),
            json!({"type": "CodingAgentTextStreamed", "text": "The plan is ready."}),
            json!({"type": "UserQuestionAsked", "tool_use_id": "q1", "cc_session_id": "s",
                   "question": "Approve the plan?",
                   "options": [{"id": "opt-0", "label": "Approve"}, {"id": "opt-1", "label": "Request changes"}]}),
            json!({"type": "UserQuestionAnswered", "tool_use_id": "q1",
                   "answer": {"kind": "Selected", "option_id": "opt-0"}}),
            json!({"type": "CodingAgentPromptSent", "text": ""}),
            json!({"type": "CodingAgentTextStreamed", "text": "Done."}),
            json!({"type": "CodingAgentIdled"}),
        ],
    );
    assert_eq!(
        entries,
        vec![
            (EntryKind::Prompt(ActorMode::Human), "plan it".to_string()),
            (
                EntryKind::Response,
                "The plan is ready.\n\nAsked: Approve the plan? [Approve | Request changes]"
                    .to_string()
            ),
            (
                EntryKind::Prompt(ActorMode::Human),
                "Answered: Approve".to_string()
            ),
            (EntryKind::Response, "Done.".to_string()),
        ]
    );
}

/// A turn is the entries its settle event closed. A second settle event with
/// nothing new between closes an empty turn.
/// A prompt names the ActorMode its event recorded: a person's message, an
/// agent's, a trigger the engine fired, and an image description. A turn that
/// failed before any reply is its response.
#[test]
fn a_prompt_names_its_sender_and_a_failed_turn_is_a_response() {
    let device = json!({"kind": "device", "device_id": "phone"});
    let entries = kinds_and_texts(
        ThreadKind::LucidosAgent,
        vec![
            json!({"type": "MessageReceived", "text": "stop the downloads", "mode": "agent"}),
            json!({"type": "ResponseFailed", "error": "overloaded"}),
            json!({"type": "TriggerStarted", "trigger_id": "t1", "trigger_name": "Mail", "prompt": "check the mail"}),
            json!({"type": "ResponseGenerated", "text": "No new mail."}),
            json!({"type": "TriggerStarted", "trigger_id": "t1", "trigger_name": "Mail", "prompt": "check again", "origin": device}),
            json!({"type": "ImageDescribed", "source_event_id": Uuid::new_v4(), "hash": "h", "description": "a red door", "model": "m"}),
            json!({"type": "PromptInjected", "text": "the wait delivered", "mode": "engine"}),
            json!({"type": "ResponseGenerated", "text": "Checked."}),
        ],
    );
    let kinds: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "prompt (agent)",
            "response",
            "prompt (engine)",
            "response",
            "prompt (human)",
            "prompt (engine)",
            "prompt (engine)",
            "response",
        ]
    );
    assert_eq!(entries[1].1, "failed: overloaded");
}

#[test]
fn turns_cover_the_entries_their_settle_event_closed() {
    let events = stored(vec![
        json!({"type": "MessageReceived", "text": "one", "mode": "human"}),
        json!({"type": "ResponseGenerated", "text": "reply one"}),
        json!({"type": "CodingAgentIdled"}),
        json!({"type": "MessageReceived", "text": "two", "mode": "human"}),
        json!({"type": "ResponseFailed", "error": "provider down"}),
    ]);
    let log = project(ThreadKind::CodingAgent, &events);
    let ranges: Vec<_> = log.turns.iter().map(|t| t.entries.clone()).collect();
    assert_eq!(ranges, vec![0..2, 2..2, 2..4]);
    assert_eq!(log.turns[0].settle_event_id, events[1].id);
    assert_eq!(log.entries[3].text, "failed: provider down");
}

/// An entry is emitted when its last source lands and never revised, so a
/// longer prefix of events only appends.
#[test]
fn the_log_only_grows_at_the_end() {
    let events = stored(vec![
        json!({"type": "MessageReceived", "text": "a", "mode": "human"}),
        json!({"type": "CodingAgentTextStreamed", "text": "working"}),
        json!({"type": "CodingAgentTextStreamed", "text": "done"}),
        json!({"type": "CodingAgentIdled"}),
        json!({"type": "MessageReceived", "text": "b", "mode": "human"}),
        json!({"type": "ResponseGenerated", "text": "ok"}),
    ]);
    let full = project(ThreadKind::CodingAgent, &events).entries;
    for n in 0..=events.len() {
        let prefix = project(ThreadKind::CodingAgent, &events[..n]).entries;
        assert_eq!(
            prefix[..],
            full[..prefix.len()],
            "prefix {n} rewrote an entry"
        );
    }
}

/// Every entry has its own source event, which is what realigning a moved
/// leaf keys on.
#[test]
fn no_two_entries_share_a_source_event() {
    let events = stored(vec![
        json!({"type": "MessageReceived", "text": "a", "mode": "human"}),
        json!({"type": "CodingAgentTextStreamed", "text": "half done"}),
        json!({"type": "ResponseFailed", "error": "boom"}),
    ]);
    let log = project(ThreadKind::CodingAgent, &events);
    let mut ids: Vec<_> = log.entries.iter().map(|e| e.event_id).collect();
    ids.dedup();
    assert_eq!(ids.len(), log.entries.len());
    assert_eq!(log.entries[1].text, "half done\n\nfailed: boom");
}

/// The engine's own acknowledgement of a queued message repeats text the log
/// already holds, so it is not an entry.
#[test]
fn an_injected_acknowledgement_is_not_logged_twice() {
    let entries = kinds_and_texts(
        ThreadKind::LucidosAgent,
        vec![
            json!({"type": "MessageReceived", "text": "and also this", "mode": "human"}),
            json!({"type": "PromptInjected", "text": "and also this",
                   "injected_message_id": Uuid::new_v4()}),
        ],
    );
    assert_eq!(entries.len(), 1);
}

#[test]
fn a_long_tool_result_keeps_its_head_and_tail() {
    let long = format!("HEAD{}TAIL", "x".repeat(40_000));
    let capped = cap(&long);
    assert!(capped.starts_with("HEAD"));
    assert!(capped.ends_with("TAIL"));
    assert!(capped.contains("chars omitted"));
    assert!(capped.chars().count() < 31_000);
}

/// The projection reads only these types. A new arm in `project` needs its
/// name here, or the drain never loads its events.
#[test]
fn every_projected_type_is_in_a_type_list() {
    for name in [
        "MessageReceived",
        "TextStreamed",
        "ToolCalled",
        "ToolResult",
        "CodingAgentTextStreamed",
        "UserQuestionAsked",
        "UserQuestionAnswered",
    ] {
        assert!(ENTRY_EVENT_TYPES.contains(&name), "{name}");
    }
    for name in ["ResponseGenerated", "CodingAgentIdled"] {
        assert!(TURN_END_EVENT_TYPES.contains(&name), "{name}");
    }
}
