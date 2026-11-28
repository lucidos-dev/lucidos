//! INVARIANT: The log entry projection: which thread events become which entry kind.
//!
//! A thread's log is a pure function of its events in sequence order, and it
//! only ever grows at the end. An entry is emitted when its last source event
//! lands, never revised later, so a built node never goes stale.
//!
//! - A **Lucidos Agent** thread logs at full fidelity: user turns, replies,
//!   tool calls and their results.
//! - A **coding-agent** thread logs one prompt and one reply per turn. A
//!   question the agent asks splits the reply there, because the owner's
//!   answer is a decision worth its own entry.

use std::ops::Range;

use uuid::Uuid;

use crate::engine::thread_events::{
    with_image_note, ActorMode, AnswerKind, MessageOrigin, QuestionOption, ThreadEvent,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind {
    Prompt(ActorMode),
    Response,
    Call,
    Result,
    Report,
    Turn,
    Artifact,
}

impl EntryKind {
    pub(crate) const ALL: [EntryKind; 9] = [
        Self::Prompt(ActorMode::Human),
        Self::Prompt(ActorMode::Agent),
        Self::Prompt(ActorMode::Engine),
        Self::Response,
        Self::Call,
        Self::Result,
        Self::Report,
        Self::Turn,
        Self::Artifact,
    ];

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Prompt(ActorMode::Human) => "prompt (human)",
            Self::Prompt(ActorMode::Agent) => "prompt (agent)",
            Self::Prompt(ActorMode::Engine) => "prompt (engine)",
            Self::Response => "response",
            Self::Call => "call",
            Self::Result => "result",
            Self::Report => "report",
            Self::Turn => "turn",
            Self::Artifact => "artifact",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LogEntry {
    pub(crate) kind: EntryKind,
    pub(crate) text: String,
    pub(crate) event_id: Uuid,
    pub(crate) sender_device_id: Option<String>,
}

impl LogEntry {
    pub(crate) fn message(&self) -> String {
        format!("{}: {}", self.kind.as_str(), self.text)
    }

    pub(crate) fn raw_line(&self) -> String {
        crate::engine::context::truncate_head_tail(&self.message(), super::NODE_BYTES)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThreadKind {
    LucidosAgent,
    CodingAgent,
}

impl ThreadKind {
    pub(crate) fn of(is_coding_agent: bool) -> Self {
        if is_coding_agent {
            Self::CodingAgent
        } else {
            Self::LucidosAgent
        }
    }
}

pub(crate) struct StoredEvent {
    pub(crate) id: Uuid,
    pub(crate) event: ThreadEvent,
    pub(crate) actor: Option<MessageOrigin>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Turn {
    pub(crate) settle_event_id: Uuid,
    pub(crate) entries: Range<usize>,
}

#[derive(Debug, Default)]
pub(crate) struct ThreadLog {
    pub(crate) entries: Vec<LogEntry>,
    pub(crate) turns: Vec<Turn>,
}

impl ThreadLog {
    pub(crate) fn turn(&self, settle_event_id: Uuid) -> Option<&Turn> {
        self.turns
            .iter()
            .find(|t| t.settle_event_id == settle_event_id)
    }
}

pub(crate) const TURN_END_EVENT_TYPES: &[&str] = &[
    "ResponseGenerated",
    "ResponseCanceled",
    "ResponseAborted",
    "ResponseFailed",
    "CodingAgentIdled",
];

/// INVARIANT: Events that can produce an entry before a turn ends. [`project`] reads no
/// other type, and the workspace log uses this list to tell an empty turn
/// from one with content.
pub(crate) const ENTRY_EVENT_TYPES: &[&str] = &[
    "MessageReceived",
    "SpokenMessageReceived",
    "PromptInjected",
    "TriggerStarted",
    "TextStreamed",
    "ToolCalled",
    "ToolResult",
    "ChildThreadCompleted",
    "ChildReportFiled",
    "ImageDescribed",
    "CodingAgentUserMessageSent",
    "CodingAgentTextStreamed",
    "UserQuestionAsked",
    "UserQuestionAnswered",
];

pub(crate) fn project(kind: ThreadKind, events: &[StoredEvent]) -> ThreadLog {
    let mut p = Projector {
        kind,
        log: ThreadLog::default(),
        turn_start: 0,
        round_text: None,
        pending_reply: None,
        questions: Vec::new(),
    };
    for e in events {
        let start = p.log.entries.len();
        p.apply(e);
        if let Some(MessageOrigin::Device { device_id }) = &e.actor {
            for entry in &mut p.log.entries[start..] {
                if entry.kind == EntryKind::Prompt(ActorMode::Human) {
                    entry.sender_device_id = Some(device_id.clone());
                }
            }
        }
    }
    p.log
}

struct Projector {
    kind: ThreadKind,
    log: ThreadLog,
    turn_start: usize,
    /// INVARIANT: Lucidos Agent: this round's text so far, and its first delta. The loop
    /// persists `TextStreamed` once per paragraph, so a round's reply is the
    /// deltas joined. The final round's text is also the response's.
    round_text: Option<(Uuid, String)>,
    pending_reply: Option<String>,
    questions: Vec<(String, Vec<QuestionOption>)>,
}

impl Projector {
    fn push(&mut self, kind: EntryKind, text: String, event_id: Uuid) {
        if text.trim().is_empty() {
            return;
        }
        self.log.entries.push(LogEntry {
            kind,
            text,
            event_id,
            sender_device_id: None,
        });
    }

    fn end_turn(&mut self, settle_event_id: Uuid) {
        self.log.turns.push(Turn {
            settle_event_id,
            entries: self.turn_start..self.log.entries.len(),
        });
        self.turn_start = self.log.entries.len();
    }

    fn flush_round_text(&mut self) {
        if let Some((id, text)) = self.round_text.take() {
            self.push(EntryKind::Response, text.trim().to_string(), id);
        }
    }

    fn apply(&mut self, e: &StoredEvent) {
        let id = e.id;
        let is_delta = matches!(e.event, ThreadEvent::TextStreamed { .. });
        let ends_turn = TURN_END_EVENT_TYPES.contains(&e.event.event_type());
        if !is_delta && !ends_turn {
            self.flush_round_text();
        }
        match &e.event {
            ThreadEvent::MessageReceived {
                text,
                user_image_hashes,
                mode,
                ..
            } => self.push(
                EntryKind::Prompt(*mode),
                with_image_note(text, user_image_hashes.len()),
                id,
            ),
            ThreadEvent::SpokenMessageReceived { text, .. }
            | ThreadEvent::CodingAgentUserMessageSent { text, .. } => {
                self.push(EntryKind::Prompt(ActorMode::Human), text.clone(), id)
            }
            ThreadEvent::PromptInjected {
                text,
                mode,
                injected_message_id: None,
                ..
            } => self.push(EntryKind::Prompt(*mode), text.clone(), id),
            ThreadEvent::TriggerStarted {
                trigger_name,
                prompt: Some(prompt),
                origin,
                ..
            } => {
                let name = trigger_name.as_deref().unwrap_or("a trigger");
                let mode = MessageOrigin::mode_or(origin.as_ref(), ActorMode::Engine);
                self.push(
                    EntryKind::Prompt(mode),
                    format!("[{name} fired] {prompt}"),
                    id,
                )
            }
            ThreadEvent::ChildThreadCompleted {
                child_thread_title,
                summary,
                ..
            }
            | ThreadEvent::ChildReportFiled {
                child_thread_title,
                summary,
                ..
            } => {
                let title = child_thread_title.as_deref().unwrap_or("sub-thread");
                self.push(EntryKind::Report, format!("[{title}] {summary}"), id)
            }
            ThreadEvent::ImageDescribed { description, .. } => self.push(
                EntryKind::Prompt(ActorMode::Engine),
                format!("image: {description}"),
                id,
            ),
            ThreadEvent::TextStreamed { text } if self.kind == ThreadKind::LucidosAgent => {
                self.round_text
                    .get_or_insert_with(|| (id, String::new()))
                    .1
                    .push_str(text);
            }
            ThreadEvent::ToolCalled { name, args, .. } if self.kind == ThreadKind::LucidosAgent => {
                self.push(EntryKind::Call, cap(&format!("{name} {args}")), id)
            }
            ThreadEvent::ToolResult {
                name,
                result,
                success,
                ..
            } if self.kind == ThreadKind::LucidosAgent => {
                let logged = if *success && knowhow_body(name, result) {
                    LOADED_KNOWHOW_POINTER
                } else {
                    result
                };
                self.push(EntryKind::Result, cap(&format!("[{name}] {logged}")), id)
            }
            ThreadEvent::CodingAgentTextStreamed { text, .. } => {
                if !text.trim().is_empty() {
                    self.pending_reply = Some(text.trim().to_string());
                }
            }
            ThreadEvent::UserQuestionAsked {
                tool_use_id,
                question,
                options,
                ..
            } if self.kind == ThreadKind::CodingAgent => {
                let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
                let mut asked = format!("Asked: {question}");
                if !labels.is_empty() {
                    asked.push_str(&format!(" [{}]", labels.join(" | ")));
                }
                let text = match self.pending_reply.take() {
                    Some(reply) => format!("{reply}\n\n{asked}"),
                    None => asked,
                };
                self.questions.push((tool_use_id.clone(), options.clone()));
                self.push(EntryKind::Response, text, id)
            }
            ThreadEvent::UserQuestionAnswered {
                tool_use_id,
                answer,
                ..
            } if self.kind == ThreadKind::CodingAgent => {
                if let Some(text) = self.answer_text(tool_use_id, answer) {
                    self.push(
                        EntryKind::Prompt(ActorMode::Human),
                        format!("Answered: {text}"),
                        id,
                    )
                }
            }
            ThreadEvent::ResponseGenerated { text, .. } => self.settle_reply(text, None, id),
            ThreadEvent::ResponseCanceled { text, .. } => {
                self.settle_reply(text, Some("stopped by the user"), id)
            }
            ThreadEvent::ResponseAborted { text, .. } => {
                self.settle_reply(text, Some("interrupted"), id)
            }
            ThreadEvent::ResponseFailed { error, .. } => {
                self.flush_round_text();
                let (kind, text) = match self.pending_reply.take() {
                    Some(reply) => (EntryKind::Response, format!("{reply}\n\nfailed: {error}")),
                    None => (EntryKind::Response, format!("failed: {error}")),
                };
                self.push(kind, text, id);
                self.end_turn(id);
            }
            ThreadEvent::CodingAgentIdled { .. } => {
                if let Some(reply) = self.pending_reply.take() {
                    self.push(EntryKind::Response, reply, id);
                }
                self.end_turn(id);
            }
            _ => {}
        }
    }

    fn settle_reply(&mut self, text: &str, ended: Option<&str>, id: Uuid) {
        let round = self.round_text.take().map(|(_, text)| text);
        let block = self.pending_reply.take();
        let mut reply = text.trim().to_string();
        if reply.is_empty() {
            reply = round.or(block).unwrap_or_default().trim().to_string();
        }
        let text = match ended {
            Some(how) if reply.is_empty() => format!("({how})"),
            Some(how) => format!("{reply} ({how})"),
            None => reply,
        };
        self.push(EntryKind::Response, text, id);
        self.end_turn(id);
    }

    fn answer_text(&self, tool_use_id: &str, answer: &AnswerKind) -> Option<String> {
        let options = self
            .questions
            .iter()
            .rev()
            .find(|(q, _)| q == tool_use_id)
            .map_or(&[][..], |(_, options)| options.as_slice());
        answer.text(options)
    }
}

pub(crate) const LOADED_KNOWHOW_POINTER: &str = "Loaded. While it stays loaded, the body is in \
    the [LOADED KNOWHOW] section. Call load_knowhow again once it has left.";

fn knowhow_body(tool: &str, result: &str) -> bool {
    tool == crate::llm::tool_names::LOAD_KNOWHOW && !crate::core::knowhow::is_not_found_body(result)
}

pub(crate) fn cap(text: &str) -> String {
    crate::engine::context::truncate_head_tail(text, super::CAP_CHARS)
}
