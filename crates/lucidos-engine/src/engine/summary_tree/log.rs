//! The log entry projection: which thread events become which entry kind.
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

use crate::engine::thread_events::{ActorMode, AnswerKind, QuestionOption, ThreadEvent};

/// What produced an entry. OptChat's kinds, plus `work` for a sub-thread's
/// report and the two workspace leaf kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind {
    /// The owner's words, or another agent addressing this thread.
    User,
    /// The agent's reply.
    Talk,
    /// A tool call, as its name and JSON input.
    Tool,
    /// A tool result.
    Echo,
    /// A sub-thread's report.
    Work,
    /// Workspace tree: one settled thread turn.
    Turn,
    /// Workspace tree: one artifact write.
    Artifact,
}

impl EntryKind {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Talk => "talk",
            Self::Tool => "tool",
            Self::Echo => "echo",
            Self::Work => "work",
            Self::Turn => "turn",
            Self::Artifact => "artifact",
        }
    }
}

/// One log entry: a message the tree's leaf stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LogEntry {
    pub(crate) kind: EntryKind,
    pub(crate) text: String,
    /// The event this entry is read back from by `zoom`.
    pub(crate) event_id: Uuid,
}

impl LogEntry {
    /// The entry as a leaf sees it, `kind: text`. A leaf of at most
    /// [`super::NODE_BYTES`] is this string verbatim.
    pub(crate) fn message(&self) -> String {
        format!("{}: {}", self.kind.as_str(), self.text)
    }

    /// The entry as a line no model wrote: its message, head and tail kept
    /// within a node's size. A view shows an unbuilt leaf this way, and a
    /// workspace turn leaf reads its turn this way.
    pub(crate) fn raw_line(&self) -> String {
        crate::engine::context::truncate_head_tail(&self.message(), super::NODE_BYTES)
    }
}

/// Which agent answers in a thread, which decides its fidelity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThreadKind {
    LucidosAgent,
    CodingAgent,
}

impl ThreadKind {
    /// From `thread_summaries.is_coding_agent`.
    pub(crate) fn of(is_coding_agent: bool) -> Self {
        if is_coding_agent {
            Self::CodingAgent
        } else {
            Self::LucidosAgent
        }
    }
}

/// One event row, parsed.
pub(crate) struct StoredEvent {
    pub(crate) id: Uuid,
    pub(crate) event: ThreadEvent,
}

/// A settled turn: the entries its settle event closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Turn {
    pub(crate) settle_event_id: Uuid,
    pub(crate) entries: Range<usize>,
}

/// A thread's projected log.
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

/// Events that end a turn. Each also settles one for the workspace log.
pub(crate) const TURN_END_EVENT_TYPES: &[&str] = &[
    "ResponseGenerated",
    "ResponseCanceled",
    "ResponseAborted",
    "ResponseFailed",
    "CodingAgentIdled",
];

/// Events that can produce an entry before a turn ends. [`project`] reads no
/// other type, and the workspace log uses this list to tell an empty turn
/// from one with content.
pub(crate) const ENTRY_EVENT_TYPES: &[&str] = &[
    "MessageReceived",
    "SpokenMessageReceived",
    "UserPromptInjected",
    "TriggerStarted",
    "TextStreamed",
    "ToolCalled",
    "ToolResult",
    "ChildThreadCompleted",
    "ImageDescribed",
    "CodingAgentUserMessageSent",
    "CodingAgentTextStreamed",
    "UserQuestionAsked",
    "UserQuestionAnswered",
];

/// Project a thread's events, in sequence order, into its log.
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
        p.apply(e);
    }
    p.log
}

struct Projector {
    kind: ThreadKind,
    log: ThreadLog,
    turn_start: usize,
    /// Lucidos Agent: this round's text so far, and its first delta. The loop
    /// persists `TextStreamed` once per paragraph, so a round's reply is the
    /// deltas joined. The final round's text is also the response's.
    round_text: Option<(Uuid, String)>,
    /// Coding agents: the newest text block since the last reply entry.
    pending_reply: Option<String>,
    /// Coding agents: options per question, to name an answer by its label.
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
        });
    }

    fn end_turn(&mut self, settle_event_id: Uuid) {
        self.log.turns.push(Turn {
            settle_event_id,
            entries: self.turn_start..self.log.entries.len(),
        });
        self.turn_start = self.log.entries.len();
    }

    /// Log the round's text as one talk entry, under its first delta.
    fn flush_round_text(&mut self) {
        if let Some((id, text)) = self.round_text.take() {
            self.push(EntryKind::Talk, text.trim().to_string(), id);
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
                ..
            } => self.push(
                EntryKind::User,
                with_images(text, user_image_hashes.len()),
                id,
            ),
            ThreadEvent::SpokenMessageReceived { text, .. }
            | ThreadEvent::CodingAgentUserMessageSent { text, .. } => {
                self.push(EntryKind::User, text.clone(), id)
            }
            // One carrying `injected_message_id` acknowledges a message the
            // log already holds, with the same text.
            ThreadEvent::UserPromptInjected {
                text,
                mode,
                injected_message_id: None,
                ..
            } => {
                let kind = match mode {
                    ActorMode::Human => EntryKind::User,
                    ActorMode::Agent | ActorMode::Engine => EntryKind::Work,
                };
                self.push(kind, text.clone(), id)
            }
            ThreadEvent::TriggerStarted {
                trigger_name,
                prompt: Some(prompt),
                ..
            } => {
                let name = trigger_name.as_deref().unwrap_or("a trigger");
                self.push(EntryKind::User, format!("[{name} fired] {prompt}"), id)
            }
            ThreadEvent::ChildThreadCompleted {
                child_thread_title,
                summary,
                ..
            } => {
                let title = child_thread_title.as_deref().unwrap_or("sub-thread");
                self.push(EntryKind::Work, format!("[{title}] {summary}"), id)
            }
            ThreadEvent::ImageDescribed { description, .. } => {
                self.push(EntryKind::Echo, format!("image: {description}"), id)
            }
            ThreadEvent::TextStreamed { text } if self.kind == ThreadKind::LucidosAgent => {
                self.round_text
                    .get_or_insert_with(|| (id, String::new()))
                    .1
                    .push_str(text);
            }
            ThreadEvent::ToolCalled { name, args, .. } if self.kind == ThreadKind::LucidosAgent => {
                self.push(EntryKind::Tool, cap(&format!("{name} {args}")), id)
            }
            ThreadEvent::ToolResult { name, result, .. }
                if self.kind == ThreadKind::LucidosAgent =>
            {
                self.push(EntryKind::Echo, cap(&format!("[{name}] {result}")), id)
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
                self.push(EntryKind::Talk, text, id)
            }
            ThreadEvent::UserQuestionAnswered {
                tool_use_id,
                answer,
            } if self.kind == ThreadKind::CodingAgent => {
                if let Some(text) = self.answer_text(tool_use_id, answer) {
                    self.push(EntryKind::User, format!("Answered: {text}"), id)
                }
            }
            ThreadEvent::ResponseGenerated { text, .. } => self.settle_reply(text, None, id),
            ThreadEvent::ResponseCanceled { text, .. } => {
                self.settle_reply(text, Some("stopped by the user"), id)
            }
            ThreadEvent::ResponseAborted { text, .. } => {
                self.settle_reply(text, Some("interrupted"), id)
            }
            ThreadEvent::ResponseFailed { error } => {
                self.flush_round_text();
                let (kind, text) = match self.pending_reply.take() {
                    Some(reply) => (EntryKind::Talk, format!("{reply}\n\nfailed: {error}")),
                    None => (EntryKind::Echo, format!("failed: {error}")),
                };
                self.push(kind, text, id);
                self.end_turn(id);
            }
            ThreadEvent::CodingAgentIdled { .. } => {
                if let Some(reply) = self.pending_reply.take() {
                    self.push(EntryKind::Talk, reply, id);
                }
                self.end_turn(id);
            }
            _ => {}
        }
    }

    /// Close a turn on a response event. The reply is the response text, else
    /// the round's text (Lucidos Agent) or the last text block (coding agent).
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
        self.push(EntryKind::Talk, text, id);
        self.end_turn(id);
    }

    fn answer_text(&self, tool_use_id: &str, answer: &AnswerKind) -> Option<String> {
        let label = |option_id: &str| -> String {
            self.questions
                .iter()
                .rev()
                .find(|(q, _)| q == tool_use_id)
                .and_then(|(_, options)| options.iter().find(|o| o.id == option_id))
                .map(|o| o.label.clone())
                .unwrap_or_else(|| option_id.to_string())
        };
        match answer {
            AnswerKind::Selected { option_id } => Some(label(option_id)),
            AnswerKind::FreeText { text, image_hashes } => {
                Some(with_images(text, image_hashes.len()))
            }
            AnswerKind::MultiSelected {
                option_ids,
                text,
                image_hashes,
            } => {
                let mut parts: Vec<String> = option_ids.iter().map(|o| label(o)).collect();
                if let Some(text) = text.as_deref().filter(|t| !t.trim().is_empty()) {
                    parts.push(text.to_string());
                }
                Some(with_images(&parts.join("; "), image_hashes.len()))
            }
            AnswerKind::Canceled | AnswerKind::Superseded => None,
        }
    }
}

/// A message's text, noting attached images so an image-only message still
/// leaves an entry.
fn with_images(text: &str, images: usize) -> String {
    match images {
        0 => text.to_string(),
        1 => format!("{text} [1 image]").trim().to_string(),
        n => format!("{text} [{n} images]").trim().to_string(),
    }
}

/// Keep the head and tail of a long tool call or result, noting the cut.
pub(crate) fn cap(text: &str) -> String {
    crate::engine::context::truncate_head_tail(text, super::CAP_CHARS)
}
