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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveState {
    Archived,
    Inbox,
}

impl ArchiveState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Archived => "archived",
            Self::Inbox => "inbox",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "inbox" => Self::Inbox,
            // Legacy values from before the rename also map to Archived.
            _ => Self::Archived,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplaySection {
    /// Local work in progress OR waiting on active children. Combines what was
    /// previously `Running` and `Waiting` — the user doesn't care whether the
    /// thread is doing the work itself or waiting on a delegated child.
    Active,
    /// Saved threads stay here regardless of any other state. Highest-priority
    /// route. The saved-section header carries a CTA badge so unaddressed
    /// changes/questions/errors aren't lost.
    Saved,
    /// Anything that needs the user's attention to progress and isn't saved
    /// or archived.
    Review,
    /// User-archived threads — long-term storage.
    Archive,
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
    Archive,
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
        "ThreadSaved" | "ThreadUnsaved" => EventClass::Metadata,
        // Compose lifecycle — orthogonal to the section/status machinery.
        "ThreadStarted" | "ThreadDiscarded" => EventClass::Metadata,
        // ImageUploaded — passive bookkeeping for content-addressed blob
        // uploads. Doesn't change thread status or surface section. The
        // SSE broadcast lets peer devices prefetch; no display impact.
        "ImageUploaded" => EventClass::Metadata,
        // Start
        "MessageReceived" | "TriggerStarted" => EventClass::Start,
        "CodingAgentUserMessageSent" | "UserPromptInjected" => EventClass::Start,
        // Session lifecycle — invisible to user (no status change, no activity bump)
        "SessionStarted" | "ContinuationStarted" | "CodingAgentSettingsChanged" => {
            EventClass::Metadata
        }
        "MergeConflictDetected" | "MissingHardeningDetected" => EventClass::Start,
        // Activity
        "TextStreamed" | "Thinking" | "MemorySearched" => EventClass::Activity,
        "ContextCaptured" => EventClass::Metadata,
        "ToolCalled" | "ToolResult" => EventClass::Activity,
        // Background-bash lifecycle. Started fires synchronously inside
        // the LLM tool turn; Completed fires asynchronously from the
        // tokio watcher when the child exits. Both classified as Metadata
        // so they don't bump status / activity (the paired ToolCalled /
        // ToolResult already covers that for the started case, and
        // completion happens outside any LLM turn).
        "BackgroundBashStarted" | "BackgroundBashCompleted" => EventClass::Metadata,
        "CodingAgentTextStreamed" | "CodingAgentToolCalled" | "CodingAgentToolResult" => {
            EventClass::Activity
        }
        "CodingAgentPromptSent" => EventClass::Activity,
        "CredentialRequested" | "McpConsentRequested" => EventClass::Activity,
        // Terminal
        "ResponseGenerated" | "ResponseCanceled" | "ResponseAborted" => {
            EventClass::Terminal
        }
        "SessionEnded" | "ThreadArchived" | "TriggerCompleted" => EventClass::Terminal,
        "ChangeApplied" | "ChangeDiscarded" | "ChangeReverted" | "ChangeApplyFailed" => {
            EventClass::Terminal
        }
        // ActionRequired
        "ResponseFailed"
        | "CodingAgentIdled"
        | "ChangeProposed"
        | "UserQuestionAsked"
        | "CodingAgentPermissionRequest" => EventClass::ActionRequired,
        // ContinueSignal — a continuation request, classified as Start so
        // the recipient thread surfaces the spawn as the beginning of a
        // new exchange. Emitted by Phase 5.3 recovery paths; the spawn
        // dispatcher (Task 5.2) actuates a CC re-spawn against the same
        // session id without a fresh user message.
        "ContinueSignal" => EventClass::Start,
        // UserQuestionAnswered is a step inside the same exchange as the
        // question — Activity, not Start. The status transition still moves
        // to Running so the resumed CC turn shows as in-progress.
        "UserQuestionAnswered" | "CodingAgentPermissionResolved" => EventClass::Activity,
        // WorktreeCleaned — passive bookkeeping for the background cleanup
        // worker (Phase 10.2). Classified as Metadata so it doesn't disturb
        // the thread's display section or activity timestamps.
        "WorktreeCleaned" => EventClass::Metadata,
        // Passive change-lifecycle bookkeeping that mutates only `changes`
        // table fields (hardened flag, merge worktree paths). No section or
        // activity-timestamp impact.
        "ChangeHardened" | "MergeResolutionStarted" | "MergeResolutionCleared" => {
            EventClass::Metadata
        }
        // Phase 4 fan-in: parent's resume path renders this as the rich
        // child-completion card (an exchange-starter, like MessageReceived).
        // The parent LLM's response after the wake becomes the response panel
        // of THIS exchange — same shape as a user-message exchange.
        // See docs/plans/2026-05-12-child-completion-card-design.md.
        "ChildThreadCompleted" => EventClass::Start,
        // Resume-helper input from the `dismiss_from_context` tool. Pure
        // bookkeeping — no UI surface, no activity bump.
        "ContextDismissed" => EventClass::Metadata,
        _ => return None,
    })
}

pub fn all_persisted_event_types() -> Vec<&'static str> {
    vec![
        "MessageReceived",
        "TextStreamed",
        "Thinking",
        "ContextCaptured",
        "MemorySearched",
        "ToolCalled",
        "ToolResult",
        "ResponseGenerated",
        "ResponseCanceled",
        "ResponseAborted",
        "ResponseFailed",
        "SessionStarted",
        "ContinuationStarted",
        "SessionEnded",
        "CodingAgentTextStreamed",
        "CodingAgentToolCalled",
        "CodingAgentToolResult",
        "CodingAgentUserMessageSent",
        "CodingAgentPromptSent",
        "CodingAgentIdled",
        "ContinueSignal",
        "MissingHardeningDetected",
        "ThreadTitleGenerated",
        "ThreadTitleRenamed",
        "ThreadSaved",
        "ThreadUnsaved",
        "ThreadArchived",
        "ThreadStarted",
        "ThreadDiscarded",
        "ImageUploaded",
        "TriggerStarted",
        "TriggerCompleted",
        "ChangeProposed",
        "ChangeApplied",
        "ChangeDiscarded",
        "ChangeReverted",
        "ChangeApplyFailed",
        "ChangeHardened",
        "MergeConflictDetected",
        "MergeResolutionStarted",
        "MergeResolutionCleared",
        "UserPromptInjected",
        "CredentialRequested",
        "McpConsentRequested",
        "CodingAgentSettingsChanged",
        "UserQuestionAsked",
        "UserQuestionAnswered",
        "CodingAgentPermissionRequest",
        "CodingAgentPermissionResolved",
        "WorktreeCleaned",
        // Phase 4 fan-in / resume bookkeeping.
        "ChildThreadCompleted",
        "ContextDismissed",
        // Background-bash lifecycle (run_bash_background trio).
        "BackgroundBashStarted",
        "BackgroundBashCompleted",
    ]
}

// ── Legal Sections & Transitions ────────────────────────────────────

pub const CHAT_LEGAL_SECTIONS: &[ArchiveState] = &[ArchiveState::Archived, ArchiveState::Inbox];
pub const CC_LEGAL_SECTIONS: &[ArchiveState] = &[ArchiveState::Archived, ArchiveState::Inbox];

pub fn is_section_legal(thread_type: ThreadType, section: ArchiveState) -> bool {
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
    pub current_section: ArchiveState,
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
    pub new_section: Option<ArchiveState>,
}

pub fn resolve_transition(
    event_type: &str,
    thread_type: ThreadType,
    current_section: ArchiveState,
    is_top_level: bool,
) -> Result<TransitionResult, LifecycleViolation> {
    let no_change = Ok(TransitionResult { new_section: None });
    let to_inbox = Ok(TransitionResult {
        new_section: Some(ArchiveState::Inbox),
    });
    let to_archived = Ok(TransitionResult {
        new_section: Some(ArchiveState::Archived),
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
        // ResponseGenerated surfaces chat threads to inbox (top-level only)
        "ResponseGenerated" => match thread_type {
            ThreadType::Chat => to_inbox,
            ThreadType::CodingAgent => no_change,
        },
        // CodingAgentIdled surfaces CC threads to inbox (top-level only)
        "CodingAgentIdled" => match thread_type {
            ThreadType::CodingAgent => to_inbox,
            ThreadType::Chat => violation("CodingAgentIdled is CC-only"),
        },
        // ChangeApplied/ChangeDiscarded: no transition — thread stays in inbox so Archive button appears
        "ChangeApplied" | "ChangeDiscarded" => no_change,
        // ThreadArchived moves thread to archived (both thread types)
        "ThreadArchived" => to_archived,
        // CC-only events illegal for Chat
        "SessionStarted"
        | "SessionEnded"
        | "CodingAgentTextStreamed"
        | "CodingAgentToolCalled"
        | "CodingAgentToolResult"
        | "CodingAgentUserMessageSent"
        | "CodingAgentPromptSent"
        | "MissingHardeningDetected"
        | "CodingAgentSettingsChanged"
        | "ContinueSignal" => match thread_type {
            ThreadType::CodingAgent => no_change,
            ThreadType::Chat => violation("CC-specific event on Chat thread"),
        },
        // User-attended terminals surface the thread in REVIEW so the user can
        // save or archive it. Without this, a cancel/abort on an already-
        // archived thread leaves it actionless: `resolve_actions` returns []
        // when stored_section != Inbox.
        "ResponseAborted" | "ResponseFailed" | "ResponseCanceled" => to_inbox,
        // ChangeProposed surfaces CC threads to inbox — proposed changes require
        // user action (apply/discard) and must surface in REVIEW. Without this,
        // CC sessions that finish without an intermediate CodingAgentIdled (or where
        // the thread is already archived) would go straight to HISTORY.
        "ChangeProposed" => match thread_type {
            ThreadType::CodingAgent => to_inbox,
            ThreadType::Chat => violation("ChangeProposed is CC-only"),
        },
        // UserQuestionAsked surfaces the thread in REVIEW so the user sees the
        // question card and the action buttons. CC-only.
        "UserQuestionAsked" => match thread_type {
            ThreadType::CodingAgent => to_inbox,
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
            ThreadType::CodingAgent => to_inbox,
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
        | "ContextCaptured"
        | "MemorySearched"
        | "ToolCalled"
        | "ToolResult"
        // ContinuationStarted opens the resume exchange after engine restart for
        // both chat (via /api/threads/<id>/continue → chat/rerun.rs) and CC
        // (via the spawn-dispatcher's --resume path). Pure boundary marker —
        // the section transition belongs to the events that follow it.
        | "ContinuationStarted"
        | "ThreadTitleGenerated"
        | "ThreadTitleRenamed"
        | "ThreadSaved"
        | "ThreadUnsaved"
        | "TriggerStarted"
        | "TriggerCompleted"
        | "ChangeReverted"
        | "ChangeApplyFailed"
        | "ChangeHardened"
        | "MergeConflictDetected"
        | "MergeResolutionStarted"
        | "MergeResolutionCleared"
        | "UserPromptInjected"
        | "CredentialRequested"
        | "McpConsentRequested"
        // Compose lifecycle — orthogonal to section/status machinery.
        | "ThreadStarted"
        | "ThreadDiscarded"
        // ImageUploaded — passive audit event for content-addressed blob
        // uploads. Same orthogonality as ThreadStarted/Discarded: no
        // section change, no status change, just a record of the attach.
        | "ImageUploaded"
        // WorktreeCleaned is a passive bookkeeping event emitted by the
        // background cleanup worker (Phase 10.2). It must NOT bump the thread
        // out of HISTORY or change status — that's the whole point of cleanup
        // running on long-idle threads.
        | "WorktreeCleaned"
        // Phase 4 fan-in events — typed callback emitted onto the parent
        // when a child thread completes, and the dismissal record the
        // `dismiss_from_context` tool produces. Both are pure resume-helper
        // input: no section transition (parent's section was already moved
        // by the child completion's other side effects), no status change.
        // Without this entry, the bus rejects the typed emit and the
        // wake-text path becomes a phantom-event reference (the C2 bug).
        | "ChildThreadCompleted"
        | "ContextDismissed"
        // Background bash lifecycle — pure audit / fallback storage for
        // the run_bash_background tools. Legal on both thread types
        // (chat is the primary caller; CC could plausibly use them too).
        // No section change — completion fires from a tokio watcher
        // outside the LLM turn, so a status flip would surface a quiet
        // thread for no user-visible reason.
        | "BackgroundBashStarted"
        | "BackgroundBashCompleted" => no_change,
        _ => violation("Unknown event type"),
    }?;

    // Chat sub-threads never go to Inbox — agentic loop children shouldn't
    // surface in REVIEW. CC threads always go to Inbox regardless of depth,
    // because every CC session needs user action (Apply/Discard/Archive).
    if !is_top_level
        && thread_type != ThreadType::CodingAgent
        && result.new_section == Some(ArchiveState::Inbox)
    {
        return Ok(TransitionResult { new_section: None });
    }

    Ok(result)
}

// ── Display Section Mapping ─────────────────────────────────────────

/// Resolution order:
///   1. is_saved                                        → Saved
///   2. status == Running OR has_active_children        → Active
///   3. has_pending_changes                             → Review
///   4. archive_state == Archived                       → Archive
///   5. otherwise                                       → Review
///
/// Saved is the strongest claim — saving is the user's "I'll manage this
/// manually" gesture and overrides every other route. Pending changes
/// outrank Archive so the user can never lose unresolved work behind the
/// archive curtain: a thread the user archived while changes are still
/// pending stays surfaced in Review until they explicitly Apply or
/// Discard each one.
pub fn display_section(
    stored: ArchiveState,
    status: ThreadStatus,
    is_saved: bool,
    has_active_children: bool,
    has_pending_changes: bool,
) -> DisplaySection {
    if is_saved {
        return DisplaySection::Saved;
    }
    if status == ThreadStatus::Running || has_active_children {
        return DisplaySection::Active;
    }
    if has_pending_changes {
        return DisplaySection::Review;
    }
    if stored == ArchiveState::Archived {
        return DisplaySection::Archive;
    }
    DisplaySection::Review
}

/// Resolve which actions are available for a thread in its current state.
/// This is the single source of truth — frontend imports the codegen'd version.
pub fn resolve_actions(
    thread_type: ThreadType,
    status: ThreadStatus,
    stored_section: ArchiveState,
    has_pending_changes: bool,
    is_saved: bool,
) -> Vec<Action> {
    // Mid-turn: nothing to dismiss; QuestionCard owns the input. Apply/Discard
    // would commit incomplete work, so this branch must run BEFORE the
    // pending-changes check below — even when archived + pending puts the
    // thread in Review, mid-turn safety wins and the action bar stays empty.
    // WaitingForUserAnswer is also mid-turn — the WaitingBanner renders a
    // Cancel button (not an Action) for that state instead.
    if status == ThreadStatus::Running || status == ThreadStatus::WaitingForUserAnswer {
        return vec![];
    }
    // Pending changes always win — display_section surfaces archived+pending
    // in Review, so the action set must follow or the user sees dots with no
    // buttons.
    if has_pending_changes && thread_type == ThreadType::CodingAgent {
        return vec![Action::Discard, Action::Apply];
    }
    // Saved threads render Archive via PromptInput.getPromptSectionButtons.
    if is_saved {
        return vec![];
    }
    if stored_section != ArchiveState::Inbox {
        return vec![];
    }
    vec![Action::Archive]
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
    // ContinueSignal — start event for a CC continuation. Bumps last_activity
    // so the thread surfaces in the recents list as soon as the recovery
    // dispatcher emits one.
    "ContinueSignal",
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
        // Projection skips empty-text payloads — see event_bus.rs.
        (
            "CodingAgentPromptSent",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        // ContinueSignal — emitted by the recovery path (Phase 5.3) when a
        // mid-turn-interrupted CC session needs to resume. The dispatcher
        // (Task 5.2) actuates the spawn, so the thread transitions back to
        // Running just like any other start event.
        (
            "ContinueSignal",
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
        // Archive → idle, clear CC flags
        (
            "ThreadArchived",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Idle),
                cc_flags: CcFlagRule::ClearAll,
            },
        ),
        // Activity events: defense in depth against premature-Idled drift.
        // `classify_result` always emits `CodingAgentIdled` after a CC Result,
        // so any subsequent activity event (the model invokes a Skill tool
        // that triggers another model turn, or back-to-back inputs CC merged
        // into one Result haven't actually finished) must re-bump status to
        // Running. This is a no-op on the common streaming path where status
        // is already Running. Must list exactly the same events as the
        // activity-event arm in `event_bus.rs::update_thread_projection`
        // (`MemorySearched` is classified Activity but stays a projection
        // no-op there, so it's also absent here).
        //
        // Caveat: the projection skips this Running-bump when the event
        // carries `meta.actor = MessageOrigin::System`. Live LLM-loop
        // activity events never set an actor (`EventMeta::NONE`); only the
        // recovery sweeps stamp System (e.g. `recover_orphan_tool_calls`
        // emits a synthetic `ToolResult` to pair an orphan `ToolCalled`).
        // Those backfills land on threads whose terminal event already
        // wrote, so resurrecting them to Running parks the row in the
        // Active section forever. The contract here describes the
        // typical live-event path; the System-actor exception lives in
        // `event_bus_projection.rs` and is covered by
        // `system_actor_activity_event_does_not_resurrect_terminated_thread`.
        (
            "TextStreamed",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "Thinking",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "ToolCalled",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "ToolResult",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "CodingAgentTextStreamed",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "CodingAgentToolCalled",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "CodingAgentToolResult",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
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
