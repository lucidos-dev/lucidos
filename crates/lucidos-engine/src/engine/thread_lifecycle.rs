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
    /// Saved threads stay here regardless of any other state. Highest-priority
    /// route. The saved-section header carries an attention badge so unaddressed
    /// changes/questions/errors aren't lost.
    Saved,
    /// The live working set: everything that isn't saved or archived. Replaces
    /// the former `Active` + `Review` split — a thread no longer hops sections
    /// when it starts or stops running. It holds threads the user is actively
    /// engaged with, in either direction: running (the system's turn), awaiting
    /// the user (their turn), or recently idle. The row's status icon carries
    /// the running ("Active") signal in place, and attention-needing threads
    /// sort to the top under a header attention badge.
    Current,
    /// User-archived threads — long-term storage.
    Archive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Idle,
    Running,
    /// Vestigial — no projection arm writes this anymore. Historical CC rows
    /// where the user hasn't yet acted on a proposed change may still carry
    /// it; kept on the enum so `ThreadStatus::parse` deserializes them
    /// without falling back to Idle (the parse path's catch-all). New rows
    /// settle to Idle and surface the "review required" state via
    /// `coding_agent_proposed` (and `is_blocking` clause 3) instead — a
    /// proposed change is an artifact for the user to review, not a parked
    /// loop. For "loop is parked waiting for human input" see
    /// `WaitingForUserAnswer`.
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

    /// Parse the snake_case wire form persisted in `thread_summaries.status`.
    /// Mirrors `as_str` exactly; unknown values fall back to Idle
    /// (defensive — the column is only written by the projection itself, so a
    /// surprise value would indicate manual DB tampering, not a bug to crash on).
    pub fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "waiting" => Self::Waiting,
            "waiting_for_user_answer" => Self::WaitingForUserAnswer,
            "failed" => Self::Failed,
            _ => Self::Idle,
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
    /// Discard the focused thread's unsent compose draft (close layer 1).
    /// Serializes as `"discard_draft"`.
    DiscardDraft,
    Discard,
    Apply,
    Archive,
    /// Retention toggle — present for any focused thread (mutually exclusive
    /// with `Unsave`). Not part of the close cascade.
    Save,
    Unsave,
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
        // Queued-message removal is a pure marker over a prior MessageReceived.
        // It must not bump recency, status, section, or message count.
        "QueuedMessageRemoved" => EventClass::Metadata,
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
        "TextStreamed" | "ThoughtStreamed" | "MemorySearched" => EventClass::Activity,
        "ContextCaptured" => EventClass::Metadata,
        "ToolCalled" | "ToolResult" => EventClass::Activity,
        // Mid-turn snapshot of the Lucidos Agent's todo list — replace-whole-list
        // semantics. Classified as Activity so the thread reads as actively
        // working when the agent calls `todo_write`, matching ToolCalled.
        "TodoListWritten" => EventClass::Activity,
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
        | "CodingAgentPermissionRequest"
        | "CommandPermissionRequested"
        | "McpPermissionRequested" => EventClass::ActionRequired,
        // ContinuationRequested — a continuation request, classified as Start so
        // the recipient thread surfaces the spawn as the beginning of a
        // new exchange. Emitted by Phase 5.3 recovery paths; the spawn
        // dispatcher (Task 5.2) actuates a CC re-spawn against the same
        // session id without a fresh user message.
        "ContinuationRequested" => EventClass::Start,
        // UserQuestionAnswered is a step inside the same exchange as the
        // question — Activity, not Start. The status transition still moves
        // to Running so the resumed CC turn shows as in-progress.
        "UserQuestionAnswered"
        | "CodingAgentPermissionResolved"
        | "CommandPermissionResolved"
        | "McpPermissionResolved" => EventClass::Activity,
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
        // Engine-internal Flash enrichment of a prior MessageReceived's
        // attached images. The originating MessageReceived already moved
        // the thread into Running; this event arrives one or more
        // iterations later as a derived past-tense fact and must NOT
        // disturb the section/status machinery.
        "ImageDescribed" => EventClass::Metadata,
        _ => return None,
    })
}

pub fn all_persisted_event_types() -> Vec<&'static str> {
    vec![
        "MessageReceived",
        "QueuedMessageRemoved",
        "TextStreamed",
        "ThoughtStreamed",
        "ContextCaptured",
        "MemorySearched",
        "ToolCalled",
        "ToolResult",
        "TodoListWritten",
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
        "ContinuationRequested",
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
        "CommandPermissionRequested",
        "CommandPermissionResolved",
        "McpPermissionRequested",
        "McpPermissionResolved",
        "WorktreeCleaned",
        // Phase 4 fan-in / resume bookkeeping.
        "ChildThreadCompleted",
        "ContextDismissed",
        // Background-bash lifecycle (run_bash_background trio).
        "BackgroundBashStarted",
        "BackgroundBashCompleted",
        // Engine-internal Flash enrichment for MessageReceived images.
        "ImageDescribed",
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

/// Custom error type — blessed exception to the `Box<dyn Error>` convention
/// (see `.claude/rules/rust.md` → "Error handling"). Justified by the
/// structured fields below: the EventBus rejection path fails fast on the
/// typed payload (event_type, thread_type, current_section, reason) instead
/// of parsing a string. Don't add new custom error types without the same
/// kind of structural justification.
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
        | "ContinuationRequested" => match thread_type {
            ThreadType::CodingAgent => no_change,
            ThreadType::Chat => violation("coding-agent-specific event on Chat thread"),
        },
        // User-attended terminals surface the thread in REVIEW so the user can
        // save or archive it. Without this, a cancel/abort on an already-
        // archived thread leaves it actionless: `available_thread_actions`
        // returns no close actions when stored_section != Inbox.
        "ResponseAborted" | "ResponseFailed" | "ResponseCanceled" => to_inbox,
        // ChangeProposed surfaces coding-agent threads to inbox — proposed changes require
        // user action (apply/discard) and must surface in REVIEW. Without this,
        // coding-agent sessions that finish without an intermediate CodingAgentIdled (or where
        // the thread is already archived) would go straight to ARCHIVE.
        "ChangeProposed" => match thread_type {
            ThreadType::CodingAgent => to_inbox,
            ThreadType::Chat => violation("ChangeProposed is coding-agent-only"),
        },
        // UserQuestionAsked surfaces the thread in REVIEW so the user sees the
        // question card and the action buttons. Raised by CC's
        // `AskUserQuestion` tool AND by the chat agent's `ask_user_question`
        // tool — same event, same lifecycle handling for both.
        "UserQuestionAsked" => to_inbox,
        // UserQuestionAnswered does not change section — the agent (CC or
        // chat) resumes and the thread stays in REVIEW until the next
        // terminal event.
        "UserQuestionAnswered" => no_change,
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
        // Command guard permission request (ADR 0002) — the chat mirror of
        // `CodingAgentPermissionRequest`. Surfaces the thread in REVIEW so the
        // user sees the PermissionCard. Emitted by the Lucidos Agent on a Chat
        // thread, never by CC, so the thread-type split is the inverse.
        "CommandPermissionRequested" => match thread_type {
            ThreadType::Chat => to_inbox,
            ThreadType::CodingAgent => violation("CommandPermissionRequested is chat-only"),
        },
        // CommandPermissionResolved is a step inside the same exchange — the
        // chat agent's tool call runs (or is refused) once the answer arrives.
        "CommandPermissionResolved" => match thread_type {
            ThreadType::Chat => no_change,
            ThreadType::CodingAgent => violation("CommandPermissionResolved is chat-only"),
        },
        // MCP permission request — the chat mirror of `CommandPermissionRequested`
        // for MCP server tool calls. Surfaces the thread in REVIEW so the user
        // sees the PermissionCard. Emitted by the Lucidos Agent on a Chat thread,
        // never by CC.
        "McpPermissionRequested" => match thread_type {
            ThreadType::Chat => to_inbox,
            ThreadType::CodingAgent => violation("McpPermissionRequested is chat-only"),
        },
        // McpPermissionResolved is a step inside the same exchange — the chat
        // agent's MCP tool call runs (or is refused) once the answer arrives.
        "McpPermissionResolved" => match thread_type {
            ThreadType::Chat => no_change,
            ThreadType::CodingAgent => violation("McpPermissionResolved is chat-only"),
        },
        // Events legal for both, no section change
        "MessageReceived"
        | "QueuedMessageRemoved"
        | "TextStreamed"
        | "ThoughtStreamed"
        | "ContextCaptured"
        | "MemorySearched"
        | "ToolCalled"
        | "ToolResult"
        // Lucidos Agent todo list snapshot — legal on both Chat and CC
        // threads for forward-compat, though today only the chat-agent tool
        // surface emits it. No section transition (the paired ToolCalled
        // already bumped activity for the wrapping turn).
        | "TodoListWritten"
        // ContinuationStarted opens the resume exchange after engine restart for
        // both chat (via /api/v1/threads/<id>/continue → chat/rerun.rs) and CC
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
        // out of ARCHIVE or change status — that's the whole point of cleanup
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
        | "BackgroundBashCompleted"
        // Engine-internal Flash enrichment of a prior MessageReceived's
        // attached images. Emitted one or more iterations after the
        // originating MessageReceived (which already moved the section);
        // a transition here would re-open a thread the user just settled.
        | "ImageDescribed" => no_change,
        _ => violation("Unknown event type"),
    }?;

    // Chat sub-threads and unattended trigger runs hide on terminal events —
    // agentic-loop children and background trigger executions shouldn't
    // surface in REVIEW. CC always goes to Inbox regardless of depth
    // because every Claude Code session needs user action (Apply / Discard / Archive).
    if !is_top_level
        && thread_type != ThreadType::CodingAgent
        && result.new_section == Some(ArchiveState::Inbox)
    {
        return Ok(TransitionResult {
            new_section: Some(ArchiveState::Archived),
        });
    }

    Ok(result)
}

// ── Display Section Mapping ─────────────────────────────────────────

/// Resolution order:
///   1. is_saved                                        → Saved
///   2. archive_state == Archived AND nothing still
///      demands surfacing                               → Archive
///   3. otherwise                                       → Current
///
/// Saved is the strongest claim — saving is the user's "I'll manage this
/// manually" gesture and overrides every other route.
///
/// Everything that isn't saved or archived lands in Current — the live
/// working set. The former `Active` (running / waiting-on-a-child) and
/// `Review` (your-turn / recently-idle) sections are merged: a thread no
/// longer changes section when it starts or stops running. The running
/// state is carried in place by the row's status icon, and attention-
/// needing threads sort to the top of Current under a header attention
/// badge instead of living in a separate section.
///
/// An archived thread still surfaces in Current — never silently dropped
/// to Archive — while it `demands_surface`: it is running, waiting on a
/// delegated child, carries its own pending changes, or carries an
/// attention descendant (bubbled transitively from any descendant in
/// `is_attention_needing` state — WaitingForUserAnswer or an in-workspace
/// CC thread with pending changes). This guarantees the user can never
/// lose unresolved work behind the archive curtain: a thread archived
/// while changes are still pending stays in Current until they explicitly
/// Apply or Discard each one. (See tests
/// `archived_with_pending_changes_routes_to_current`,
/// `attention_descendant_overrides_archive`.)
pub fn display_section(
    stored: ArchiveState,
    status: ThreadStatus,
    is_saved: bool,
    has_active_children: bool,
    has_pending_changes: bool,
    has_attention_descendants: bool,
) -> DisplaySection {
    if is_saved {
        return DisplaySection::Saved;
    }
    // Archived threads drop to Archive UNLESS they still demand surfacing —
    // running, waiting on a delegated child, carrying their own pending
    // changes, or carrying an attention descendant — in which case they stay
    // in Current alongside the rest of the live working set so unresolved
    // work can't hide behind the archive curtain.
    let demands_surface = status == ThreadStatus::Running
        || has_active_children
        || has_pending_changes
        || has_attention_descendants;
    if stored == ArchiveState::Archived && !demands_surface {
        return DisplaySection::Archive;
    }
    DisplaySection::Current
}

/// A thread is "blocking" iff archiving its ancestor would silently strand
/// active work in it. Three clauses, in order:
///
/// 1. Running or WaitingForUserAnswer always blocks, regardless of
///    `archive_state` — active work cannot be "already terminal", and the
///    Archived short-circuit must not mask it.
/// 2. Otherwise, `archive_state == Archived` does NOT block — the user
///    dismissed the thread and the row isn't stranding anything.
/// 3. An idle in-workspace CodingAgent thread with pending changes blocks
///    until the user Applies or Discards. External-repo CC is the carve-out:
///    `WaitingBanner.tsx` swaps `[Discard, Apply]` for `[Archive]` because
///    Apply can't merge into a different repo, and the cascade handler emits
///    `ChangeApplied` for each pending change before the `ThreadArchived`
///    emit so the change row closes cleanly instead of dangling.
///
/// Source of truth for the descendants_block_archive computation in the
/// thread_summaries projection.
///
/// **SQL mirrors** — keep in sync when the predicate changes:
/// - `event_bus_projection_propagation.rs::rebuild_blocking_descendant_count`
///   (recursive CTE recomputing the column from scratch).
/// - The most recent backfill migration applying this WHERE clause is
///   `20260518132821_blocking_count_running_overrides_archived.sql`. CTEs
///   can't share a function across migrations, so any new backfill must
///   inline the same WHERE clause.
pub fn is_blocking(
    thread_type: ThreadType,
    status: ThreadStatus,
    archive_state: ArchiveState,
    has_pending_changes: bool,
    is_external_repo: bool,
) -> bool {
    if status == ThreadStatus::Running || status == ThreadStatus::WaitingForUserAnswer {
        return true;
    }
    if archive_state == ArchiveState::Archived {
        return false;
    }
    if has_pending_changes && thread_type == ThreadType::CodingAgent && !is_external_repo {
        return true;
    }
    false
}

/// A thread "needs attention" iff its state requires a user action to
/// progress. Same shape as `is_blocking` but DROPS the `Running` clause:
/// a running descendant is delegated work (belongs to Active), not pending
/// attention (would belong to Review).
///
/// Concretely:
/// - `WaitingForUserAnswer` — user must answer the question / permission
///   card before the agent can resume.
/// - In-workspace CC thread with pending changes — user must Apply or
///   Discard before the thread can settle. External-repo CC is the same
///   carve-out as `is_blocking` clause 3.
///
/// Drives `thread_summaries.attention_descendant_count`, which bubbles
/// transitively up the ancestor chain so any thread with a
/// "needs-attention" descendant routes to Review via `display_section`.
/// Relationship: `is_blocking = is_attention_needing OR status == Running`.
/// `Archive`-button gating still uses `is_blocking` so a Running descendant
/// keeps the button hidden — only the section routing splits them.
///
/// **SQL mirrors** — keep in sync when the predicate changes:
/// - `event_bus_projection_propagation.rs::rebuild_blocking_descendant_count`
///   (the recursive CTE there recomputes BOTH columns in one pass — see the
///   `attention_cnt` clause).
/// - `event_bus_projection_propagation.rs::reconcile_blocking_descendant_count_for_ancestors`
///   (the per-ancestor reconcile updates BOTH columns in lockstep).
/// - The backfill in `20260522091904_add_attention_descendant_count.sql`
///   inlines the same WHERE clause. CTEs can't share a function across
///   migrations, so any new backfill must inline it again.
pub fn is_attention_needing(
    thread_type: ThreadType,
    status: ThreadStatus,
    archive_state: ArchiveState,
    has_pending_changes: bool,
    is_external_repo: bool,
) -> bool {
    if status == ThreadStatus::WaitingForUserAnswer {
        return true;
    }
    if archive_state == ArchiveState::Archived {
        return false;
    }
    if has_pending_changes && thread_type == ThreadType::CodingAgent && !is_external_repo {
        return true;
    }
    false
}

/// Every action available for a thread in its current state, in cascade
/// priority order. This is the single DB-derivable source of truth — the
/// frontend imports the codegen'd version AND the mutating HTTP handlers guard
/// on it server-side.
///
/// Returned order makes the front-most close LAYER positional:
/// `[DiscardDraft?, Discard?, Apply?, Archive?, Unsave|Save]`. The close set is
/// the prefix; the retention toggle (`Save`/`Unsave`) always appends exactly
/// one entry for a focused thread, matching the always-present prompt section
/// toggle.
///
/// Inputs and their seam:
/// - `has_pending_changes` / `descendants_block_archive` — projection facts
///   (`coding_agent_proposed`, `blocking_descendant_count > 0`).
/// - `has_unsent_draft` — `thread_summaries.compose_text`/`compose_images`
///   non-empty. DB-derivable, but the frontend feeds the live value from the
///   `composeDrafts` signal so the cascade doesn't lag the 250 ms compose
///   debounce.
/// - `is_saved` — `thread_summaries.is_saved`.
///
/// `descendants_block_archive` is true when any descendant is currently in a
/// state that prevents archive (Running, WaitingForUserAnswer, or
/// has_pending_changes && CodingAgent — see `is_blocking`).
pub fn available_thread_actions(
    thread_type: ThreadType,
    status: ThreadStatus,
    stored_section: ArchiveState,
    has_pending_changes: bool,
    descendants_block_archive: bool,
    has_unsent_draft: bool,
    is_saved: bool,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let live =
        status == ThreadStatus::Running || status == ThreadStatus::WaitingForUserAnswer;
    let coding_agent_pending = has_pending_changes && thread_type == ThreadType::CodingAgent;

    // Layer 1 — draft discard. Orthogonal to run state: an unsent draft can be
    // discarded whether or not the thread is live.
    if has_unsent_draft {
        actions.push(Action::DiscardDraft);
    }
    // Layers 2 & 3 — change resolution then archive. Both are suppressed while
    // the thread is live (mid-turn). A pending change outranks archive: the
    // user must Apply or Discard before the thread can be archived.
    if !live {
        if coding_agent_pending {
            actions.push(Action::Discard);
            actions.push(Action::Apply);
        } else if stored_section == ArchiveState::Inbox && !descendants_block_archive {
            actions.push(Action::Archive);
        }
    }
    // Retention toggle — available in any run state, exactly one of the pair.
    if is_saved {
        actions.push(Action::Unsave);
    } else {
        actions.push(Action::Save);
    }
    actions
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
    "CommandPermissionRequested",
    "CommandPermissionResolved",
    "McpPermissionRequested",
    "McpPermissionResolved",
    // ContinuationRequested — start event for a CC continuation. Bumps last_activity
    // so the thread surfaces in the recents list as soon as the recovery
    // dispatcher emits one.
    "ContinuationRequested",
    // Step events (keep timestamp current during long agentic responses).
    // Canonical names only — this constant is consulted via the
    // deserialized variant (alias collapses legacy "Thinking" rows to
    // ThoughtStreamed before the lookup).
    "ToolCalled",
    "ToolResult",
    "TextStreamed",
    "ThoughtStreamed",
    "MemorySearched",
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
    /// Status depends on coding_agent_proposed: first = with changes, second = without.
    ConditionalCc(ThreadStatus, ThreadStatus),
    /// No status change.
    NoChange,
}

/// How an event changes CC flags in thread_summaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CcFlagRule {
    /// Clear all CC flags (coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo, coding_agent_applying).
    ClearAll,
    /// Set coding_agent_proposed = true.
    SetChanges,
    /// Set coding_agent_applying = true.
    SetApplying,
    /// Clear coding_agent_applying only.
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
        // ContinuationRequested — emitted by the recovery path (Phase 5.3) when a
        // mid-turn-interrupted Claude Code session needs to resume. The dispatcher
        // (Task 5.2) actuates the spawn, so the thread transitions back to
        // Running just like any other start event.
        (
            "ContinuationRequested",
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
        // indicator as ResponseFailed, unless a Claude Code session left pending
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
        // CC lifecycle. The projection SQL also preserves a pre-existing
        // 'failed' status (`CASE WHEN status='failed' THEN 'failed' ELSE …`):
        // a failed CC turn emits `ResponseFailed` then this idle in the same
        // turn, and the idle must not downgrade the red error dot. The coarse
        // `ConditionalCc` model can't express "preserve failed", but the
        // `terminal_events_never_set_running` invariant still holds — see the
        // CodingAgentIdled arm in `event_bus_projection_thread.rs`.
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
        // Question card raised — pauses the agent (CC or chat), surfaces
        // the card. UserQuestionAnswered transitions back to Running so
        // the channel-specific resume path
        // (POST /api/v1/threads/{thread_id}/answer-question → CC
        // `resume_cc_with_tool_result` for CC, in-process tool wake for
        // chat) can take over.
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
        // Mirrors UserQuestionAsked/Answered, but the Claude Code subprocess is NOT
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
        // Command guard permission prompt (ADR 0002) — pauses the chat agent's
        // bash/python tool call, surfaces a PermissionCard. Mirrors
        // CodingAgentPermission*: the agent loop blocks in-process, so the
        // resolution transitions back to Running to resume the loop.
        (
            "CommandPermissionRequested",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::WaitingForUserAnswer),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "CommandPermissionResolved",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        // MCP permission prompt (chat) — pauses the chat agent's MCP server tool
        // call, surfaces a PermissionCard. Mirrors CommandPermission*: the agent
        // loop blocks in-process, so the resolution transitions back to Running.
        (
            "McpPermissionRequested",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::WaitingForUserAnswer),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "McpPermissionResolved",
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
        // activity-event arm in `event_bus.rs::update_thread_projection`.
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
            "ThoughtStreamed",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "MemorySearched",
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
#[path = "thread_lifecycle_tests/contract.rs"]
mod contract_tests;

#[cfg(test)]
#[path = "thread_lifecycle_tests/classification.rs"]
mod classification_tests;

#[cfg(test)]
#[path = "thread_lifecycle_tests/display_and_actions.rs"]
mod display_and_actions_tests;

#[cfg(test)]
#[path = "thread_lifecycle_tests/events.rs"]
mod events_tests;

#[cfg(test)]
#[path = "thread_lifecycle_tests/invariants.rs"]
mod invariants_tests;

#[cfg(test)]
#[path = "thread_lifecycle_tests/scenario_tests.rs"]
mod scenario_tests;
