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

/// The wire and DB spelling of [`ThreadType::CodingAgent`].
///
/// One copy, because the string is compared against `thread_summaries.source`
/// in several places and a fourth hand-written literal is how the two spellings
/// drift. The authority is the `#[serde(rename)]` above.
pub const CODING_AGENT_SOURCE: &str = "claude_code";

impl ThreadType {
    /// Read a `thread_summaries.source` as the type it names.
    ///
    /// Everything that is not a coding agent is `Chat`, `trigger` included: a
    /// trigger thread's turns run the Lucidos Agent, so it is a chat thread in
    /// every way this enum decides. Not a silent default, since the enum has
    /// exactly two members and this is the whole of one of them.
    pub fn from_source(source: &str) -> Self {
        if source == CODING_AGENT_SOURCE {
            Self::CodingAgent
        } else {
            Self::Chat
        }
    }
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
    /// **Nothing writes this any more.** It survives so historical rows keep
    /// their meaning. `ThreadStatus::parse` deserializes `waiting` here rather
    /// than falling through to Idle. The variant also stays in `ALL`, in the
    /// CLI's status filter, and in the generated TS union.
    ///
    /// `AbortCause::status_sql()` was the last writer. Its verdict arms opened
    /// `CASE WHEN coding_agent_proposed THEN 'waiting'`, so an abort landing on
    /// a CC thread with a pending change settled here instead of at `Paused` /
    /// `Failed`. That ordering (a change to review outranks the interruption)
    /// is kept, and now lives only in the frontend's `resolveVisualStatus`. See
    /// `docs/plans/2026-08-22-a-restart-verdict-survives-a-pending-change.md`
    /// for why saying it twice cost the verdict.
    ///
    /// Every settle path writes `STATUS_FROM_PROPOSED_CHANGE`, the literal
    /// `'idle'`: a proposed change is an artifact for the user to review, not a
    /// parked loop, so it surfaces via `coding_agent_proposed` (and
    /// `is_blocking` clause 3) instead. For "loop is parked waiting for human
    /// input" see `WaitingForUserAnswer`.
    Waiting,
    /// CC paused on an `AskUserQuestion` tool call. The subprocess was killed
    /// after emitting `UserQuestionAsked`; resuming requires the user to
    /// answer (or cancel), at which point the engine respawns CC with
    /// `--resume` and feeds the answer back as a `tool_result`.
    WaitingForUserAnswer,
    /// The user's own *Switch to new version* interrupted this turn, and the
    /// engine has promised to resume it. Set by `AbortCause::status_sql()` for
    /// exactly one shape, `AbortCause::promises_auto_resume()`: an
    /// `EngineShutdown` abort stamped with the device that clicked Switch.
    ///
    /// Nothing failed here, which is the whole point of the variant: before it
    /// existed the switch teardown landed on `Failed`, so switching versions
    /// painted every in-flight thread with the red error dot for work the engine
    /// was about to resume by itself. The converse matters just as much, and is
    /// why the condition is this narrow: an interruption NOBODY is coming back
    /// for (a crash, a bare shutdown, or a boot that could not keep the resume
    /// promise) is `Failed`, so it keeps the red dot, its needs-attention slot,
    /// and its Continue button. Distinct from `WaitingForUserAnswer`, where the
    /// loop is parked on a question the user must answer: a paused turn is
    /// simply on its way back.
    ///
    /// **Written whether or not the thread has a pending change.** The change
    /// rides `coding_agent_proposed`, and the frontend's `resolveVisualStatus`
    /// ranks the two. This arm used to defer to a pending change and write
    /// `Waiting`, which is how the verdict got lost; see that variant.
    ///
    /// A verdict, not a resting state: like `Failed`, it must survive the trailing
    /// events of the dying turn (see `preserving_verdict`), or recovery's own
    /// `CodingAgentIdled` walks it straight back to `Idle`.
    Paused,
    /// Last response failed (model error, quota exceeded, etc.). Distinct from
    /// `Paused`, so an interruption the engine expects to resume isn't reported
    /// as an error. Cleared when the user sends another message (→ `Running`).
    /// Written whether or not a change is pending, for the reason `Paused`
    /// gives.
    Failed,
}

impl ThreadStatus {
    /// Every status, in the order a human would read them: idle, then the
    /// three ways a turn can still be open, then the two verdicts.
    ///
    /// The single source for five surfaces: the `status` filter's accepted
    /// values on `threads list` / `count`, the value list its error messages
    /// and the CLI help print, the `status` enum in the `threads` LLM tool
    /// schema, and (through `thread_lifecycle_tests::contract`) the
    /// cross-validation fixture's status dimension and the generated TS union.
    ///
    /// **A variant missing from here is missing from all five**, and no test
    /// can catch that, because every enumeration a test could use IS this
    /// array. The array's own length is a compile-time constant, so it fails
    /// nothing either. `as_str` below is the one place the compiler stops you,
    /// which is why the instruction is repeated there.
    pub const ALL: [Self; 6] = [
        Self::Idle,
        Self::Running,
        Self::Waiting,
        Self::WaitingForUserAnswer,
        Self::Paused,
        Self::Failed,
    ];

    /// Adding a variant makes this match non-exhaustive, which is the compile
    /// error that brings you here. Add it to [`Self::ALL`] in the same edit
    /// (and widen the array): nothing downstream will tell you if you don't.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::WaitingForUserAnswer => "waiting_for_user_answer",
            Self::Paused => "paused",
            Self::Failed => "failed",
        }
    }

    /// Strict inverse of `as_str`: `None` for anything that is not one of the
    /// six current statuses. This is the parser for *caller-supplied* values
    /// (the `status` filter on threads list / count), where an unrecognized
    /// value must become a visible error rather than quietly selecting
    /// something. Contrast [`Self::parse`], which reads the projection's own
    /// column and is deliberately lenient.
    ///
    /// Accepts the kebab spelling of each value as well as the snake_case one
    /// the column stores, so `waiting-for-user-answer` (the repo's kebab-case
    /// convention for public parameter values) and `waiting_for_user_answer`
    /// (what every returned row's `status` field prints) select the same rows.
    /// Only that one value has a separator at all.
    pub fn try_parse(s: &str) -> Option<Self> {
        let normalized = s.trim().to_ascii_lowercase().replace('-', "_");
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == normalized)
    }

    /// Parse the snake_case wire form persisted in `thread_summaries.status`.
    /// Mirrors `as_str` exactly; unknown values fall back to Idle
    /// (defensive: the column is only written by the projection itself, so a
    /// surprise value would indicate manual DB tampering, not a bug to crash on).
    ///
    /// `waiting_for_event` is the one value that reaches this from real data
    /// without being tampering: it was a status until 2026-08-06, when a
    /// subscription stopped holding the turn (see
    /// `docs/plans/2026-08-06-every-event-wait-is-detached.md`). The migration
    /// rewrites the stored rows; the fallback is what makes a row written by an
    /// older engine against a shared database still read as what it now means,
    /// which is `Idle`.
    pub fn parse(s: &str) -> Self {
        Self::try_parse(s).unwrap_or(Self::Idle)
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
    /// Arm a *standing apply*: the change applies once the thread settles (ADR
    /// 0168 clause 5). Offered exactly where `Apply` is withheld because the
    /// thread is still working, so a control that cannot act is replaced by the
    /// one that can. Serializes as `"apply_when_settled"`.
    ApplyWhenSettled,
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
        // Voice-session lifecycle. Metadata on BOTH halves, which is the
        // asymmetry against the coding-agent pair above, where the end is
        // Terminal. A live microphone is not a turn: the thread's status
        // belongs to the agent's turn, and voice is a mode of the thread
        // (ADR 0148). An end that settled status would terminate a turn the
        // doer is still running.
        "VoiceSessionStarted" | "VoiceSessionEnded" => EventClass::Metadata,
        // A spoken reply, for the same reason. The talker authors it mid-call,
        // often while the doer's own turn is still running. Classifying it
        // as a response would settle a turn it has no part in.
        "SpokenReplyGenerated" => EventClass::Metadata,
        // An utterance the talker handled alone, and the delegation that
        // asked for the doer. Metadata on both, for opposite reasons.
        //
        // SpokenMessageReceived starts nothing, so Start would leave the
        // thread waiting on a turn that never runs. WorkDelegated sits BESIDE
        // a turn the `MessageReceived` next to it already started, so Start
        // would count one utterance twice.
        "SpokenMessageReceived" | "WorkDelegated" => EventClass::Metadata,
        "MergeConflictDetected" | "MissingHardeningDetected" => EventClass::Start,
        // Activity
        "TextStreamed" | "ThoughtStreamed" | "MemoryRecalled" => EventClass::Activity,
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
        // Command-guard checkpoint lifecycle: a pure card / audit projection,
        // so neither bumps status or activity (ADR 0002, Phase 4).
        "CommandCheckpointed" | "CommandCheckpointReverted" => EventClass::Metadata,
        "CodingAgentTextStreamed"
        | "CodingAgentThoughtStreamed"
        | "CodingAgentToolCalled"
        | "CodingAgentToolResult" => EventClass::Activity,
        "CodingAgentPromptSent" => EventClass::Activity,
        "CredentialRequested" | "McpConsentRequested" => EventClass::Activity,
        // Terminal
        "ResponseGenerated" | "ResponseCanceled" | "ResponseAborted" => EventClass::Terminal,
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
        // Resume-helper input from the retired `dismiss_from_context` tool,
        // and the record of a keep. Pure bookkeeping, no UI surface and no
        // activity bump.
        "ContextDismissed" | "ContextKeptOpen" => EventClass::Metadata,
        // The model's picture of the job, written in its own reply. The
        // round that carried it already bumped activity, so this is
        // bookkeeping the next turn reads back.
        "WorkingUnderstandingWritten" => EventClass::Metadata,
        // Engine-internal Flash enrichment of a prior MessageReceived's
        // attached images. The originating MessageReceived already moved
        // the thread into Running; this event arrives one or more
        // iterations later as a derived past-tense fact and must NOT
        // disturb the section/status machinery.
        "ImageDescribed" => EventClass::Metadata,
        // The cached older-turn summary (ADR 0102). Written during turn setup,
        // before the starter event on some paths, so it must not touch the
        // section or status machinery. It is derived content, not a decision.
        "ConversationSummarized" => EventClass::Metadata,
        // Event-wait lifecycle. Registration is Activity, not ActionRequired:
        // the thread subscribed to something the SYSTEM will deliver, so it must
        // never read as needing the user. The three resolutions are Activity for
        // the same reason `UserQuestionAnswered` is: none of them opens a fresh
        // round of work. A delivery and an expiry resume the turn that parked,
        // and the delivery's own `UserPromptInjected` is the Start event that
        // opens the woken turn; a cancel resumes nothing at all.
        //
        // Not the same axis as how the transcript GROUPS them. This class drives
        // the section and status machinery; the frontend decides its own exchange
        // boundaries, and it opens one for a `user_stop` cancel so the person who
        // pressed Stop waiting sees their own action where they took it
        // (`isExchangeStartEvent`). Activity is still right here: a stop starts no
        // work and moves the thread nowhere.
        "EventWaitStarted" | "EventWaitDelivered" | "EventWaitExpired" | "EventWaitCanceled" => {
            EventClass::Activity
        }
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
        "MemoryRecalled",
        "ToolCalled",
        "ToolResult",
        "TodoListWritten",
        "WorkingUnderstandingWritten",
        "ResponseGenerated",
        "ResponseCanceled",
        "ResponseAborted",
        "ResponseFailed",
        "SessionStarted",
        "ContinuationStarted",
        "SessionEnded",
        "CodingAgentTextStreamed",
        "CodingAgentThoughtStreamed",
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
        "ContextKeptOpen",
        // Background-bash lifecycle (run_bash_background trio).
        "BackgroundBashStarted",
        "BackgroundBashCompleted",
        // Engine-internal Flash enrichment for MessageReceived images.
        "ImageDescribed",
        // The cached older-turn summary. Persisted because the event IS the
        // cache: there is no table, and a later turn rebuilds it from here.
        "ConversationSummarized",
        // Command-guard checkpoint lifecycle (ADR 0002, Phase 4).
        "CommandCheckpointed",
        "CommandCheckpointReverted",
        // Event-wait lifecycle. Persisted because the wait IS the event: there
        // is no table, and the dispatcher's live set is rebuilt from these.
        "EventWaitStarted",
        "EventWaitDelivered",
        "EventWaitExpired",
        "EventWaitCanceled",
        // Voice-session lifecycle. Persisted because the pair IS the record:
        // there is no session table, and the boot sweep finds an unpaired
        // start by reading these back.
        "VoiceSessionStarted",
        "VoiceSessionEnded",
        // What the caller actually heard. Persisted because the audio is not,
        // so this text is the only record of a spoken turn.
        "SpokenReplyGenerated",
        // The other half of a call's transcript: an utterance the talker
        // answered alone, and the talker's own request for the doer. Persisted
        // for the same reason, since neither has a row anywhere else.
        "SpokenMessageReceived",
        "WorkDelegated",
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

/// Resolve where an event moves a thread's `archive_state`.
///
/// `is_unattended` is the caller's verdict that nobody is watching this run.
/// `event_bus_projection_thread.rs` computes it, and only a trigger execution
/// can qualify: one the user neither opted into reviewing nor followed up on.
/// It suppresses the inbox surfacing an event would otherwise cause, so the
/// bottom guard below is the only thing that reads it.
pub fn resolve_transition(
    event_type: &str,
    thread_type: ThreadType,
    current_section: ArchiveState,
    is_unattended: bool,
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
        | "CodingAgentThoughtStreamed"
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
        | "MemoryRecalled"
        | "ToolCalled"
        | "ToolResult"
        // Lucidos Agent todo list snapshot — legal on both Chat and CC
        // threads for forward-compat, though today only the chat-agent tool
        // surface emits it. No section transition (the paired ToolCalled
        // already bumped activity for the wrapping turn).
        | "TodoListWritten"
        // The model's working understanding, same shape as the todo list
        // and legal on the same threads.
        | "WorkingUnderstandingWritten"
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
        // when a child thread completes, plus the two curation records: the
        // retired `dismiss_from_context` tool's, and the one a `[KEEP OPEN]`
        // heading in the working understanding produces. No
        // section transition here:
        // the parent's section was already moved by the child completion's
        // other side effects. Without this entry, the bus rejects the typed
        // emit and the wake-text path becomes a phantom-event reference (the
        // C2 bug).
        //
        // A completion that wakes its parent DOES move the status to
        // `running`. The parent's exchange then opens the way a user
        // message's does, rather than reading "Done" through the turn setup.
        // That write is `update_parent_after_child_terminal` in
        // `event_bus/parent_callback.rs`, under the same `should_callback`
        // gate that decides a card is owed. It is absent from
        // `status_transitions()` on purpose: that table mirrors
        // `update_thread_projection`, and this write is not in it.
        | "ChildThreadCompleted"
        | "ContextDismissed"
        | "ContextKeptOpen"
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
        | "ImageDescribed"
        // The cached older-turn summary, written while the prompt is being
        // assembled. A transition here would flip a thread to running before
        // its own starter event says so.
        | "ConversationSummarized"
        // Command-guard checkpoint lifecycle (ADR 0002, Phase 4). Both are
        // mid-turn bookkeeping over an Undo card: the checkpoint is taken while
        // the turn is already running, and the revert is a button on that card.
        // Neither moves the section or the status. Without these entries the bus
        // rejected the typed emit, so the events never persisted, the Undo card
        // never rendered, and every guarded command leaked its git checkpoint
        // ref because only the undo path deletes it.
        | "CommandCheckpointed"
        | "CommandCheckpointReverted"
        // Event-wait lifecycle. No section transition in either direction: the
        // park keeps the thread exactly where it was (it is mid-turn, and the
        // status carries the parked signal in place), and the resolutions
        // resume that same turn. Surfacing to Inbox on the park would read as
        // "your turn", which is the one thing a system-side wait is not.
        //
        // Legal on both thread types for forward-compat, though only the chat
        // agent has `await_event` today (a coding-agent thread parks through
        // its own mechanisms).
        | "EventWaitStarted"
        | "EventWaitDelivered"
        | "EventWaitExpired"
        | "EventWaitCanceled"
        // Voice-session lifecycle. Voice is a MODE of a thread (ADR 0148), so
        // neither half moves it: the section and the status belong to the
        // agent's turn, and a live microphone is not a turn. An end that
        // settled the thread would terminate work the agent is still doing.
        //
        // Legal on both thread types, matching the event-wait pair above.
        // Nothing may offer voice on a coding-agent thread (ADR 0165), so this
        // arm is reachable on one only through a row written before that rule.
        // It stays legal because the question here is what a session TOUCHES,
        // and the answer is nothing either way.
        | "VoiceSessionStarted"
        | "VoiceSessionEnded"
        // A spoken reply moves nothing either. The caller is on the call, so
        // surfacing the thread to the Inbox would ask for attention they are
        // already giving.
        //
        // Nor does an utterance the talker handled alone, nor the talker's
        // request for the doer. Same caller, same call, same reason.
        | "SpokenReplyGenerated"
        | "SpokenMessageReceived"
        | "WorkDelegated" => no_change,
        _ => violation("Unknown event type"),
    }?;

    // An unattended run hides on its terminal event. Nobody is watching, so
    // surfacing it would ask for attention on work the user never started.
    //
    // Depth is deliberately NOT part of this. A finished sub-thread keeps the
    // inbox state it ran with. Archiving it here writes a state no
    // `ThreadArchived` event backs, and the drawer then dims a row nobody
    // archived. See
    // `docs/plans/2026-08-17-a-finished-sub-thread-stays-in-the-inbox.md`.
    //
    // A coding-agent thread is exempt at every depth, because every session
    // ends needing Apply, Discard or Archive.
    if is_unattended
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
/// 1. Running or WaitingForUserAnswer always blocks, whatever the
///    `archive_state`: active work cannot be "already terminal", and the
///    Archived short-circuit must not mask it. A thread merely holding an
///    *event wait* is deliberately NOT here: a subscription does not hold its
///    thread's turn, so such a thread is idle and archiving it is a legitimate
///    thing to do (the archive cancels the subscription through
///    `EventWaitCancelCause::ThreadArchived`).
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
/// a running descendant is delegated work, not pending attention.
///
/// Concretely:
/// - `WaitingForUserAnswer` — user must answer the question / permission
///   card before the agent can resume.
/// - In-workspace CC thread with pending changes — user must Apply or
///   Discard before the thread can settle. External-repo CC is the same
///   carve-out as `is_blocking` clause 3.
///
/// Drives `thread_summaries.attention_descendant_count`, which bubbles
/// transitively up the ancestor chain, so a thread with a "needs-attention"
/// descendant is kept in `DisplaySection::Current` by `display_section` even
/// once archived. Running and attention-needing threads share that one
/// section; what separates them in the UI is the per-row status icon versus
/// the attention badge and its drawer filter, both fed by this count.
///
/// A thread merely holding an *event wait* is absent from this and from
/// `is_blocking` alike: a subscription does not hold its thread's turn, so such
/// a thread is plain idle. Its subscriptions surface in the per-thread waiting
/// indicator instead.
///
/// `available_thread_actions` DOES read the wait (ADR 0106), and the asymmetry
/// is deliberate. Relaxing `is_blocking` would let an ancestor cascade-archive
/// the thread, and that cascade emits `ChangeApplied` per pending change. So it
/// would reach the outcome the action gate exists to prevent. The cost is a
/// parked thread wearing an attention badge with no action to take, which is
/// premature rather than wrong: it will need the user once it wakes.
///
/// Relationship: `is_blocking = is_attention_needing OR status == Running`.
/// `Archive`-button gating still uses `is_blocking` so a Running descendant
/// keeps the button hidden.
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
/// - `has_live_event_waits` / `has_active_children`: projection facts
///   (`live_event_wait_count > 0`, `active_children_count > 0`). Either one
///   means the thread is *parked*, so something will wake it.
///
/// `descendants_block_archive` is true when any descendant is currently in a
/// state that prevents archive (Running, WaitingForUserAnswer, or
/// has_pending_changes && CodingAgent — see `is_blocking`).
// Six of the nine parameters are `bool`, so the argument swap this lint guards
// against is a live hazard rather than an impossible one. Two things carry it
// instead of a facts struct, which would have to be mirrored through the TS
// emitter and the fixture to buy anything. The cross-validation fixture
// exhausts the cross product and compares Rust against TS case by case, so a
// swap between the two languages cannot survive it. And each side has exactly
// ONE production caller, both covered: `available_thread_actions_for` and
// `resolveThreadActions`.
#[allow(clippy::too_many_arguments)]
pub fn available_thread_actions(
    thread_type: ThreadType,
    status: ThreadStatus,
    stored_section: ArchiveState,
    has_pending_changes: bool,
    descendants_block_archive: bool,
    has_live_event_waits: bool,
    has_active_children: bool,
    has_unsent_draft: bool,
    is_saved: bool,
) -> Vec<Action> {
    let mut actions = Vec::new();
    // A thread holding an *event wait* is NOT live: the subscription does not
    // hold its turn, so it settles at `idle` and keeps Archive (ADR 0049).
    let live = status == ThreadStatus::Running || status == ThreadStatus::WaitingForUserAnswer;
    // Parked: the turn ended, but something will wake this thread and it may
    // commit again on the same branch. Its change is therefore not final.
    //
    // A gap survives between the delivery clearing the fact and the wake
    // reaching `running`, so Apply reappears for it. ADR 0106 records why that
    // is accepted: the flag needed to close it strands TRUE on a wake lost to a
    // restart, and withholds Apply for good.
    let will_resume = has_live_event_waits || has_active_children;
    let coding_agent_pending = has_pending_changes && thread_type == ThreadType::CodingAgent;

    // Layer 1 — draft discard. Orthogonal to run state: an unsent draft can be
    // discarded whether or not the thread is live.
    if has_unsent_draft {
        actions.push(Action::DiscardDraft);
    }
    // Layers 2 & 3 — change resolution then archive. Both are suppressed while
    // the thread is live (mid-turn). A pending change outranks archive: the
    // user must Apply or Discard before the thread can be archived.
    //
    // A parked thread loses Apply and Discard too, because both resolve a change
    // the thread has not finished producing: it wakes on its delivery and commits
    // on to the same branch. That leaves it exactly what a Running thread offers.
    // Archive is unaffected, because a pending change already outranked it here.
    // Archiving a parked thread with no change still cancels its waits rather than
    // stranding them (`EventWaitCancelCause::ThreadArchived`). The way out before
    // the 24 h ceiling is Stop waiting, which clears the wait and restores both.
    if !live {
        if coding_agent_pending {
            if !will_resume {
                actions.push(Action::Discard);
                actions.push(Action::Apply);
            }
        } else if stored_section == ArchiveState::Inbox && !descendants_block_archive {
            actions.push(Action::Archive);
        }
    }
    // The standing apply, offered while the thread is still working. Running
    // and Paused are the two states a standing apply can wait through: every
    // other resting state resolves it at once, so offering it there would arm
    // something that drops on its first look. See `engine::standing_apply`.
    if thread_type == ThreadType::CodingAgent
        && (status == ThreadStatus::Running || status == ThreadStatus::Paused)
    {
        actions.push(Action::ApplyWhenSettled);
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

/// Event types whose projection sets `last_activity = NOW()` in
/// `event_bus_projection_thread.rs`. Must match exactly the match arms in
/// `EventBus::update_thread_projection()`.
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
    // Canonical names only: this constant is consulted via the deserialized
    // variant, so the serde aliases have already collapsed legacy rows
    // ("Thinking" to ThoughtStreamed, "MemorySearched" to MemoryRecalled)
    // before the lookup.
    "ToolCalled",
    "ToolResult",
    "TextStreamed",
    "ThoughtStreamed",
    "MemoryRecalled",
    "CodingAgentTextStreamed",
    "CodingAgentThoughtStreamed",
    "CodingAgentToolCalled",
    "CodingAgentToolResult",
    // A call is activity on the thread it runs on, whichever half speaks.
    // Placing one is a user action, and the two spoken rows keep the timestamp
    // current through a call nobody delegates from (ADR 0167).
    "VoiceSessionStarted",
    "SpokenMessageReceived",
    "SpokenReplyGenerated",
];

/// Event types that increment message_count in the thread_summaries projection.
/// Must match the events that do `message_count + 1` in
/// `event_bus_projection_thread.rs`.
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

/// Status transitions for each event type, mirroring
/// `event_bus_projection_thread.rs`'s `update_thread_projection()`.
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
        // Projection skips empty-text payloads: see event_bus_projection_thread.rs.
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
        // in event_bus_projection_thread.rs and thread-events.ts) because the
        // user's message is still being processed in a fresh session.
        (
            "SessionEnded",
            StatusTransition {
                status: StatusRule::ConditionalCc(ThreadStatus::Waiting, ThreadStatus::Idle),
                cc_flags: CcFlagRule::None,
            },
        ),
        // System interruption. Approximate on purpose: this table has no cause
        // or actor axis, and `AbortCause::status_sql()` splits three ways.
        // `StaleSettle` maps to 'idle' (engine cleanup of a row whose process was
        // already gone), and the user's own *Switch to new version* (an
        // `EngineShutdown` abort carrying a device actor) maps to 'paused' rather
        // than 'failed', because the engine resumes that turn itself. The row
        // below states the remaining case, which is every interruption nobody
        // promised to undo and IS genuinely failed; the split lives next to the
        // cause enum, on `AbortCause::promises_auto_resume()`.
        //
        // A pending change no longer enters into it, which is why this is
        // `Set` rather than the `ConditionalCc(Waiting, Failed)` it was. The
        // change surfaces through `coding_agent_proposed`, and the frontend
        // ranks it against the verdict.
        (
            "ResponseAborted",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Failed),
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
        // **No event-wait row sets a status, and that absence is the rule, not
        // an omission.** A subscription does not hold its thread's turn:
        // registration happens mid-turn and the turn's own terminator decides
        // the status, while a resolution lands on a thread that is either idle
        // or running something unrelated. The delivery's wake sets Running
        // through its own `UserPromptInjected`, which is where that transition
        // belongs, and a cancel leaves the thread exactly as it found it.
        //
        // Writing one here is the specific bug to avoid: it would report a
        // running thread as revived, or an idle one as running with no turn
        // behind it. All four `EventWait*` types are deliberately absent from
        // this table. See
        // `docs/plans/2026-08-06-every-event-wait-is-detached.md`.
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
        // activity-event arm in
        // `event_bus_projection_thread.rs::update_thread_projection`.
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
        // `event_bus_projection_thread.rs` and is covered by
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
            "MemoryRecalled",
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
            "CodingAgentThoughtStreamed",
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
