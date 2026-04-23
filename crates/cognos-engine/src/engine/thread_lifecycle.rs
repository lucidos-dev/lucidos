use serde::Serialize;

// ── Core Types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadType {
    Chat,
    /// `CodingAgent` serializes as `"claude_code"` to preserve the wire format
    /// for the only current backend; the rename of the wire string is deferred
    /// until all consumers (DB, frontend, projections) are updated together.
    #[serde(rename = "claude_code")]
    CodingAgent,
}

impl ThreadType {
    pub fn from_source(source: &str) -> Self {
        match source {
            "claude_code" => Self::CodingAgent,
            _ => Self::Chat,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredSection {
    Default,
    Unread,
}

impl StoredSection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Unread => "unread",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "unread" => Self::Unread,
            // "waiting" was removed — treat legacy DB values as default
            _ => Self::Default,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplaySection {
    Running,
    Waiting,
    Review,
    Pinned,
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Idle,
    Running,
    /// CC has finished and proposed changes that need user review (apply/discard).
    /// Chat threads never reach this status — only CC threads with cc_has_changes=true.
    Waiting,
    /// CC paused on an `AskUserQuestion` tool call. The subprocess was killed
    /// after emitting `UserQuestionAsked`; resuming requires the user to
    /// answer (or cancel), at which point the engine respawns CC with
    /// `--resume` and feeds the answer back as a `tool_result`.
    WaitingForUserAnswer,
    /// Last response failed (model error, quota exceeded, etc.). Distinct from
    /// `Waiting` so the UI can show an error indicator instead of the changes
    /// dot. Cleared when the user sends another message (→ `Running`).
    Failed,
}

impl ThreadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::WaitingForUserAnswer => "waiting_for_user_answer",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventClass {
    Metadata,
    Start,
    Activity,
    Terminal,
    ActionRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Done,
    Apply,
    Discard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MessageLabel {
    #[serde(rename = "Requesting")]
    Requesting,
    #[serde(rename = "Working")]
    Working,
    #[serde(rename = "Waiting")]
    Waiting,
    #[serde(rename = "Canceled")]
    Canceled,
    #[serde(rename = "Aborted")]
    Aborted,
}

// ── Event Classification ────────────────────────────────────────────

pub fn classify_event(event_type: &str) -> Option<EventClass> {
    Some(match event_type {
        // Metadata
        "ThreadTitleGenerated" | "ThreadTitleRenamed" => EventClass::Metadata,
        "ThreadPinned" | "ThreadUnpinned" => EventClass::Metadata,
        "ThreadMarkedRead" | "ThreadMarkedUnread" => EventClass::Metadata,
        // Start
        "MessageReceived" | "TriggerStarted" => EventClass::Start,
        "CodingAgentUserMessageSent" | "UserPromptInjected" => EventClass::Start,
        // Session lifecycle — invisible to user (no status change, no activity bump)
        "SessionStarted" | "SessionRecovered" | "CodingAgentSettingsChanged" => {
            EventClass::Metadata
        }
        "MergeConflictDetected" | "MissingHardeningDetected" => EventClass::Start,
        // Activity
        "TextStreamed" | "Thinking" | "MemorySearched" => EventClass::Activity,
        "ToolCalled" | "ToolResult" => EventClass::Activity,
        "CodingAgentTextStreamed" | "CodingAgentToolCalled" | "CodingAgentToolResult" => {
            EventClass::Activity
        }
        "CodingAgentPromptSent" => EventClass::Activity,
        "CredentialRequested" | "McpConsentRequested" => EventClass::Activity,
        // Terminal
        "ResponseGenerated" | "ResponseCanceled" | "ResponseAborted" => EventClass::Terminal,
        "SessionEnded" | "ThreadDismissed" | "TriggerCompleted" => EventClass::Terminal,
        "ChangeApplied" | "ChangeDiscarded" | "ChangeReverted" | "ChangeApplyFailed" => {
            EventClass::Terminal
        }
        // ActionRequired
        "ResponseFailed"
        | "CodingAgentIdled"
        | "ChangeProposed"
        | "UserQuestionAsked"
        | "CodingAgentPermissionRequest" => EventClass::ActionRequired,
        // UserQuestionAnswered is a step inside the same exchange as the
        // question — Activity, not Start. The status transition still moves
        // to Running so the resumed CC turn shows as in-progress.
        "UserQuestionAnswered" | "CodingAgentPermissionResolved" => EventClass::Activity,
        _ => return None,
    })
}

pub fn all_persisted_event_types() -> Vec<&'static str> {
    vec![
        "MessageReceived",
        "TextStreamed",
        "Thinking",
        "MemorySearched",
        "ToolCalled",
        "ToolResult",
        "ResponseGenerated",
        "ResponseCanceled",
        "ResponseAborted",
        "ResponseFailed",
        "SessionStarted",
        "SessionRecovered",
        "SessionEnded",
        "CodingAgentTextStreamed",
        "CodingAgentToolCalled",
        "CodingAgentToolResult",
        "CodingAgentUserMessageSent",
        "CodingAgentPromptSent",
        "CodingAgentIdled",
        "MissingHardeningDetected",
        "ThreadTitleGenerated",
        "ThreadTitleRenamed",
        "ThreadPinned",
        "ThreadUnpinned",
        "ThreadMarkedRead",
        "ThreadMarkedUnread",
        "ThreadDismissed",
        "TriggerStarted",
        "TriggerCompleted",
        "ChangeProposed",
        "ChangeApplied",
        "ChangeDiscarded",
        "ChangeReverted",
        "ChangeApplyFailed",
        "MergeConflictDetected",
        "UserPromptInjected",
        "CredentialRequested",
        "McpConsentRequested",
        "CodingAgentSettingsChanged",
        "UserQuestionAsked",
        "UserQuestionAnswered",
        "CodingAgentPermissionRequest",
        "CodingAgentPermissionResolved",
    ]
}

// ── Legal Sections & Transitions ────────────────────────────────────

pub const CHAT_LEGAL_SECTIONS: &[StoredSection] = &[StoredSection::Default, StoredSection::Unread];
pub const CC_LEGAL_SECTIONS: &[StoredSection] = &[StoredSection::Default, StoredSection::Unread];

pub fn is_section_legal(thread_type: ThreadType, section: StoredSection) -> bool {
    match thread_type {
        ThreadType::Chat => CHAT_LEGAL_SECTIONS.contains(&section),
        ThreadType::CodingAgent => CC_LEGAL_SECTIONS.contains(&section),
    }
}

/// Custom error type (exception to the Box<dyn Error> convention in CLAUDE.md)
/// because lifecycle violations carry structured data needed for fail-fast
/// notifications: which event, which thread type, which section.
#[derive(Debug, Clone)]
pub struct LifecycleViolation {
    pub event_type: String,
    pub thread_type: ThreadType,
    pub current_section: StoredSection,
    pub reason: String,
}

impl std::fmt::Display for LifecycleViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Thread lifecycle violation: '{}' is not valid for {:?} threads (section: {:?}). {}",
            self.event_type, self.thread_type, self.current_section, self.reason
        )
    }
}

impl std::error::Error for LifecycleViolation {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionResult {
    pub new_section: Option<StoredSection>,
    pub side_effects: Vec<SideEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SideEffect {
    EmitThreadMarkedUnread,
    EmitThreadMarkedRead,
}

pub fn resolve_transition(
    event_type: &str,
    thread_type: ThreadType,
    current_section: StoredSection,
    is_top_level: bool,
) -> Result<TransitionResult, LifecycleViolation> {
    let no_change = Ok(TransitionResult {
        new_section: None,
        side_effects: vec![],
    });
    let violation = |reason: &str| {
        Err(LifecycleViolation {
            event_type: event_type.to_string(),
            thread_type,
            current_section,
            reason: reason.to_string(),
        })
    };

    let result = match event_type {
        // ResponseGenerated marks chat threads as unread (top-level only)
        "ResponseGenerated" => match thread_type {
            ThreadType::Chat => Ok(TransitionResult {
                new_section: Some(StoredSection::Unread),
                side_effects: vec![SideEffect::EmitThreadMarkedUnread],
            }),
            ThreadType::CodingAgent => no_change,
        },
        // CodingAgentIdled marks CC threads as unread (top-level only)
        "CodingAgentIdled" => match thread_type {
            ThreadType::CodingAgent => Ok(TransitionResult {
                new_section: Some(StoredSection::Unread),
                side_effects: vec![SideEffect::EmitThreadMarkedUnread],
            }),
            ThreadType::Chat => violation("CodingAgentIdled is CC-only"),
        },
        // ChangeApplied/ChangeDiscarded: no transition — thread stays unread so Done button appears
        "ChangeApplied" | "ChangeDiscarded" => no_change,
        // ThreadMarkedRead always resets to default
        "ThreadMarkedRead" => Ok(TransitionResult {
            new_section: Some(StoredSection::Default),
            side_effects: vec![],
        }),
        // ThreadMarkedUnread sets unread (top-level only, both thread types)
        "ThreadMarkedUnread" => Ok(TransitionResult {
            new_section: Some(StoredSection::Unread),
            side_effects: vec![],
        }),
        // ThreadDismissed clears unread → default (both thread types)
        "ThreadDismissed" => Ok(TransitionResult {
            new_section: Some(StoredSection::Default),
            side_effects: vec![],
        }),
        // CC-only events illegal for Chat
        "SessionStarted"
        | "SessionRecovered"
        | "SessionEnded"
        | "CodingAgentTextStreamed"
        | "CodingAgentToolCalled"
        | "CodingAgentToolResult"
        | "CodingAgentUserMessageSent"
        | "CodingAgentPromptSent"
        | "MissingHardeningDetected"
        | "CodingAgentSettingsChanged" => match thread_type {
            ThreadType::CodingAgent => no_change,
            ThreadType::Chat => violation("CC-specific event on Chat thread"),
        },
        // ResponseAborted/ResponseFailed mark threads as unread — the user
        // needs to know their request was interrupted (system kill / engine
        // restart) or failed (model error / quota), so the thread surfaces in
        // REVIEW with its error indicator instead of disappearing into HISTORY.
        "ResponseAborted" | "ResponseFailed" => Ok(TransitionResult {
            new_section: Some(StoredSection::Unread),
            side_effects: vec![SideEffect::EmitThreadMarkedUnread],
        }),
        // ChangeProposed marks CC threads as unread — proposed changes require
        // user action (apply/discard) and must surface in REVIEW. Without this,
        // CC sessions that finish without an intermediate CodingAgentIdled (or where
        // the user already read the thread) would go straight to HISTORY.
        "ChangeProposed" => match thread_type {
            ThreadType::CodingAgent => Ok(TransitionResult {
                new_section: Some(StoredSection::Unread),
                side_effects: vec![SideEffect::EmitThreadMarkedUnread],
            }),
            ThreadType::Chat => violation("ChangeProposed is CC-only"),
        },
        // UserQuestionAsked surfaces the thread in REVIEW so the user sees the
        // question card and the action buttons. CC-only.
        "UserQuestionAsked" => match thread_type {
            ThreadType::CodingAgent => Ok(TransitionResult {
                new_section: Some(StoredSection::Unread),
                side_effects: vec![SideEffect::EmitThreadMarkedUnread],
            }),
            ThreadType::Chat => violation("UserQuestionAsked is CC-only"),
        },
        // UserQuestionAnswered does not change section — CC will resume and the
        // thread stays in REVIEW until the next terminal event.
        "UserQuestionAnswered" => match thread_type {
            ThreadType::CodingAgent => no_change,
            ThreadType::Chat => violation("UserQuestionAnswered is CC-only"),
        },
        // CC permission request — surfaces the thread in REVIEW so the user
        // sees the PermissionCard. CC-only.
        "CodingAgentPermissionRequest" => match thread_type {
            ThreadType::CodingAgent => Ok(TransitionResult {
                new_section: Some(StoredSection::Unread),
                side_effects: vec![SideEffect::EmitThreadMarkedUnread],
            }),
            ThreadType::Chat => violation("CodingAgentPermissionRequest is CC-only"),
        },
        // CodingAgentPermissionResolved is a step inside the same exchange —
        // CC resumes its tool call once the answer arrives.
        "CodingAgentPermissionResolved" => match thread_type {
            ThreadType::CodingAgent => no_change,
            ThreadType::Chat => violation("CodingAgentPermissionResolved is CC-only"),
        },
        // Events legal for both, no section change
        "MessageReceived"
        | "TextStreamed"
        | "Thinking"
        | "MemorySearched"
        | "ToolCalled"
        | "ToolResult"
        | "ResponseCanceled"
        | "ThreadTitleGenerated"
        | "ThreadTitleRenamed"
        | "ThreadPinned"
        | "ThreadUnpinned"
        | "TriggerStarted"
        | "TriggerCompleted"
        | "ChangeReverted"
        | "ChangeApplyFailed"
        | "MergeConflictDetected"
        | "UserPromptInjected"
        | "CredentialRequested"
        | "McpConsentRequested" => no_change,
        _ => violation("Unknown event type"),
    }?;

    // Chat sub-threads never go to Unread — agentic loop children shouldn't
    // surface in REVIEW. CC threads always go to Unread regardless of depth,
    // because every CC session needs user action (Apply/Discard/Done).
    if !is_top_level
        && thread_type != ThreadType::CodingAgent
        && result.new_section == Some(StoredSection::Unread)
    {
        return Ok(TransitionResult {
            new_section: None,
            side_effects: vec![],
        });
    }

    Ok(result)
}

// ── Display Section Mapping ─────────────────────────────────────────

pub fn display_section(
    stored: StoredSection,
    status: ThreadStatus,
    is_pinned: bool,
    has_active_children: bool,
) -> DisplaySection {
    if status == ThreadStatus::Running {
        return DisplaySection::Running;
    }
    // CC paused on a user question — always surface in REVIEW so the question
    // card is reachable, even if stored=default (e.g. legacy DB rows).
    if status == ThreadStatus::WaitingForUserAnswer {
        return DisplaySection::Review;
    }
    if has_active_children {
        return DisplaySection::Waiting;
    }
    match stored {
        StoredSection::Unread => DisplaySection::Review,
        StoredSection::Default => {
            if is_pinned {
                DisplaySection::Pinned
            } else {
                DisplaySection::History
            }
        }
    }
}

/// Resolve which actions are available for a thread in its current state.
/// This is the single source of truth — frontend imports the codegen'd version.
pub fn resolve_actions(
    thread_type: ThreadType,
    status: ThreadStatus,
    stored_section: StoredSection,
    has_pending_changes: bool,
) -> Vec<Action> {
    // Only idle/waiting threads in Unread get actions
    if stored_section != StoredSection::Unread {
        return vec![];
    }
    // No bottom-bar actions while CC is mid-turn — there's nothing to dismiss yet.
    if status == ThreadStatus::Running {
        return vec![];
    }
    // While waiting for an answer the QuestionCard owns answer/free-text input,
    // but the user must still be able to abandon the question and dismiss the
    // thread. Show only Done — Apply/Discard would commit incomplete mid-turn work.
    if status == ThreadStatus::WaitingForUserAnswer {
        return vec![Action::Done];
    }

    match thread_type {
        ThreadType::Chat => vec![Action::Done],
        ThreadType::CodingAgent => {
            if has_pending_changes {
                vec![Action::Discard, Action::Apply]
            } else {
                vec![Action::Done]
            }
        }
    }
}

// ── TypeScript Codegen ──────────────────────────────────────────────

/// Event types whose projection sets `last_activity = NOW()` in event_bus.rs.
/// Must match exactly the match arms in EventBus::update_thread_projection().
/// The frontend uses this to keep thread.meta.updatedAt in sync with the backend.
pub const LAST_ACTIVITY_EVENTS: &[&str] = &[
    // Thread start events (upsert INSERT with last_activity = NOW())
    "MessageReceived",
    "TriggerStarted",
    // Activity events (UPDATE SET last_activity = NOW())
    "ResponseGenerated",
    "ResponseAborted",
    "CodingAgentIdled",
    "ChangeApplied",
    "ChangeDiscarded",
    "CodingAgentUserMessageSent",
    "UserPromptInjected",
    "TriggerCompleted",
    "UserQuestionAsked",
    "UserQuestionAnswered",
    "CodingAgentPermissionRequest",
    "CodingAgentPermissionResolved",
    // Step events (keep timestamp current during long agentic responses)
    "ToolCalled",
    "ToolResult",
    "TextStreamed",
    "Thinking",
    "CodingAgentTextStreamed",
    "CodingAgentToolCalled",
    "CodingAgentToolResult",
];

/// Event types that increment message_count in the thread_summaries projection.
/// Must match the events that do `message_count + 1` in event_bus.rs.
pub const MESSAGE_COUNT_EVENTS: &[&str] = &[
    "MessageReceived",
    "TriggerStarted",
    "CodingAgentUserMessageSent",
    "UserPromptInjected",
];

/// How an event changes thread status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusRule {
    /// Set status to a fixed value.
    Set(ThreadStatus),
    /// Status depends on cc_has_changes: first = with changes, second = without.
    ConditionalCc(ThreadStatus, ThreadStatus),
    /// No status change.
    NoChange,
}

/// How an event changes CC flags in thread_summaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CcFlagRule {
    /// Clear all CC flags (cc_has_changes, cc_requires_restart, cc_is_external_repo, cc_applying).
    ClearAll,
    /// Set cc_has_changes = true.
    SetChanges,
    /// Set cc_applying = true.
    SetApplying,
    /// Clear cc_applying only.
    ClearApplying,
    /// Read CC flags from event payload (CodingAgentIdled).
    FromPayload,
    /// No CC flag changes.
    None,
}

pub struct StatusTransition {
    pub status: StatusRule,
    pub cc_flags: CcFlagRule,
}

/// Status transitions for each event type, mirroring event_bus.rs update_thread_projection().
/// Events not listed here have no status or CC flag effect.
pub fn status_transitions() -> Vec<(&'static str, StatusTransition)> {
    vec![
        // Start events → running
        (
            "MessageReceived",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "TriggerStarted",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "CodingAgentUserMessageSent",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "UserPromptInjected",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "CodingAgentPromptSent",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        // Response completed → depends on pending changes
        (
            "ResponseGenerated",
            StatusTransition {
                status: StatusRule::ConditionalCc(ThreadStatus::Waiting, ThreadStatus::Idle),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "ResponseCanceled",
            StatusTransition {
                status: StatusRule::ConditionalCc(ThreadStatus::Waiting, ThreadStatus::Idle),
                cc_flags: CcFlagRule::None,
            },
        ),
        // Exception: reason=StaleResume skips this transition (handled imperatively
        // in event_bus.rs and thread-events.ts) because the user's message is still
        // being processed in a fresh session.
        (
            "SessionEnded",
            StatusTransition {
                status: StatusRule::ConditionalCc(ThreadStatus::Waiting, ThreadStatus::Idle),
                cc_flags: CcFlagRule::None,
            },
        ),
        // System interruption — surfaces in REVIEW with the same red error
        // indicator as ResponseFailed, unless a CC session left pending
        // changes (then 'waiting' → changes dot wins, since reviewing the
        // changes is more actionable than acknowledging the interrupt).
        (
            "ResponseAborted",
            StatusTransition {
                status: StatusRule::ConditionalCc(ThreadStatus::Waiting, ThreadStatus::Failed),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "ResponseFailed",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Failed),
                cc_flags: CcFlagRule::None,
            },
        ),
        // Scheduled task done → idle
        (
            "TriggerCompleted",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Idle),
                cc_flags: CcFlagRule::None,
            },
        ),
        // CC lifecycle
        (
            "CodingAgentIdled",
            StatusTransition {
                status: StatusRule::ConditionalCc(ThreadStatus::Waiting, ThreadStatus::Idle),
                cc_flags: CcFlagRule::FromPayload,
            },
        ),
        // ChangeProposed only sets CC flags, not status. Status is already correct:
        // - If CC just idled → CodingAgentIdled already set 'waiting'
        // - If CC is still running (mid-session commit) → stays 'running' (no premature buttons)
        // - SessionEnded handles the terminal status via ConditionalCc
        (
            "ChangeProposed",
            StatusTransition {
                status: StatusRule::NoChange,
                cc_flags: CcFlagRule::SetChanges,
            },
        ),
        (
            "ChangeApplied",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Idle),
                cc_flags: CcFlagRule::ClearAll,
            },
        ),
        (
            "ChangeDiscarded",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Idle),
                cc_flags: CcFlagRule::ClearAll,
            },
        ),
        (
            "MergeConflictDetected",
            StatusTransition {
                status: StatusRule::NoChange,
                cc_flags: CcFlagRule::SetApplying,
            },
        ),
        (
            "ChangeApplyFailed",
            StatusTransition {
                status: StatusRule::NoChange,
                cc_flags: CcFlagRule::ClearApplying,
            },
        ),
        // CC AskUserQuestion — pauses CC, surfaces a question card.
        // UserQuestionAnswered transitions back to Running so the resume code path
        // (POST /api/cc/answer-question → resume_cc_with_tool_result) can take over.
        (
            "UserQuestionAsked",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::WaitingForUserAnswer),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "UserQuestionAnswered",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        // CC permission prompt — pauses CC's tool call, surfaces a PermissionCard.
        // Mirrors UserQuestionAsked/Answered, but the CC subprocess is NOT
        // killed; the MCP stdio server inside the subprocess blocks on the
        // engine's HTTP response, so the resolution event transitions back to
        // Running so the in-flight tool call can complete.
        (
            "CodingAgentPermissionRequest",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::WaitingForUserAnswer),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "CodingAgentPermissionResolved",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        // Dismiss → idle, clear CC flags
        (
            "ThreadDismissed",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Idle),
                cc_flags: CcFlagRule::ClearAll,
            },
        ),
    ]
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "thread_lifecycle_tests/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "thread_lifecycle_tests/scenario_tests.rs"]
mod scenario_tests;
