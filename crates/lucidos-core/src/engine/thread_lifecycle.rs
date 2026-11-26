use serde::{Deserialize, Serialize};

use crate::engine::thread_events::UnproposedReason;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadType {
    Chat,
    /// INVARIANT: `CodingAgent` serializes as `"claude_code"` to preserve the wire format
    /// for the only current backend; the rename of the wire string is deferred
    /// until all consumers (DB, frontend, projections) are updated together.
    #[serde(rename = "claude_code")]
    CodingAgent,
}

pub const CODING_AGENT_SOURCE: &str = "claude_code";

impl ThreadType {
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
            _ => Self::Archived,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplaySection {
    Saved,
    OnHold,
    Current,
    Archive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Idle,
    Running,
    /// INVARIANT: **Nothing writes this any more.** It survives so historical rows keep
    /// their meaning. `ThreadStatus::parse` deserializes `waiting` here rather
    /// than falling through to Idle. The variant also stays in `ALL`, in the
    /// CLI's status filter, and in the generated TS union.
    ///
    /// `AbortCause::status_sql()` was the last writer. Its verdict arms opened
    /// with a pending-change check that wrote `'waiting'`, so an abort landing on
    /// a CC thread with a pending change settled here instead of at `Paused` /
    /// `Failed`. That ordering (a change to review outranks the interruption)
    /// is kept, and now lives only in the frontend's `resolveVisualStatus`. See
    /// `docs/plans/2026-08-22-a-restart-verdict-survives-a-pending-change.md`
    /// for why saying it twice cost the verdict.
    ///
    /// Every settle path writes `STATUS_FROM_PROPOSED_CHANGE`, the literal
    /// `'idle'`: a proposed change is an artifact for the user to review, not a
    /// parked loop, so it surfaces via the `proposed` change state (and
    /// `is_blocking` clause 3) instead. For "loop is parked waiting for human
    /// input" see `WaitingForUserAnswer`.
    Waiting,
    WaitingForUserAnswer,
    /// INVARIANT: The user's own *Switch to new version* interrupted this turn, and the
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
    /// rides the change state, and the frontend's `resolveVisualStatus`
    /// ranks the two. This arm used to defer to a pending change and write
    /// `Waiting`, which is how the verdict got lost; see that variant.
    ///
    /// A verdict, not a resting state: like `Failed`, it must survive the trailing
    /// events of the dying turn (see `preserving_verdict`), or recovery's own
    /// `CodingAgentIdled` walks it straight back to `Idle`.
    Paused,
    Failed,
}

impl ThreadStatus {
    /// INVARIANT: Every status, in the order a human would read them: idle, then the
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

    pub fn try_parse(s: &str) -> Option<Self> {
        let normalized = s.trim().to_ascii_lowercase().replace('-', "_");
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == normalized)
    }

    pub fn parse(s: &str) -> Self {
        Self::try_parse(s).unwrap_or(Self::Idle)
    }

    pub const fn sql_literal(self) -> &'static str {
        match self {
            Self::Idle => "'idle'",
            Self::Running => "'running'",
            Self::Waiting => "'waiting'",
            Self::WaitingForUserAnswer => "'waiting_for_user_answer'",
            Self::Paused => "'paused'",
            Self::Failed => "'failed'",
        }
    }
}

impl sqlx::Type<sqlx::Postgres> for ThreadStatus {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <&str as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <&str as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for ThreadStatus {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        Ok(Self::parse(<&str as sqlx::Decode<sqlx::Postgres>>::decode(
            value,
        )?))
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
pub enum ThreadAction {
    DiscardDraft,
    Discard,
    Apply,
    Hold,
    ApplyWhenSettled,
    Archive,
    Save,
    Unsave,
}

pub fn classify_event(event_type: &str) -> Option<EventClass> {
    Some(match event_type {
        "ThreadTitleGenerated" | "ThreadTitleRenamed" => EventClass::Metadata,
        "ThreadSaved" | "ThreadUnsaved" | "ThreadMovedToFolder" => EventClass::Metadata,
        "TriggerRunInterrupted" => EventClass::Metadata,
        "ThreadArchiveRequested" => EventClass::Metadata,
        "ThreadUnarchived" => EventClass::Metadata,
        "ThreadTriageProposed" => EventClass::Metadata,
        "WorkItemDriverTaken"
        | "RootThreadDriverTaken"
        | "WorkItemDriverReleased"
        | "RootThreadDriverReleased"
        | "WorkItemGateEscalated"
        | "RootThreadGateEscalated"
        | "WorkItemWatched"
        | "RootThreadWatched"
        | "WorkItemUnwatched"
        | "RootThreadUnwatched" => EventClass::Metadata,
        "UserOnlyMessagePosted" | "AgentPaused" | "AgentResumed" => EventClass::Metadata,
        "AutomationPaused" | "AutomationUnpaused" | "ThreadHoldPlaced" | "ThreadHoldLifted" => {
            EventClass::Metadata
        }
        "PermissionGranted"
        | "PermissionSpent"
        | "PermissionRestored"
        | "PermissionRevoked"
        | "PermissionRequestPrepared"
        | "BatchItemRan" => EventClass::Metadata,
        "ThreadReadRequested" | "ThreadReadNotRequested" | "ThreadReplySeen" => {
            EventClass::Metadata
        }
        "ParentWakeRequested" | "ParentWakeNotRequested" => EventClass::Metadata,
        "ChangeApplyRequested" | "ApplyNowStarted" | "ApplyNowEnded" => EventClass::Metadata,
        "QueuedMessageRemoved" => EventClass::Metadata,
        "MessageHeld" | "HeldMessageReleased" => EventClass::Metadata,
        // INVARIANT: A read acknowledges an input already recorded, so it opens no
        // exchange. One that started a turn after a Stop still sets running:
        // see its row in `status_transitions`.
        "CodingAgentInputRead" => EventClass::Metadata,
        "CodingAgentInputDiscarded" => EventClass::Metadata,
        "ChildReportTaken" => EventClass::Metadata,
        "ThreadStarted" | "ThreadDiscarded" | "HomeThreadCreated" => EventClass::Metadata,
        "ImageUploaded" => EventClass::Metadata,
        "MessageReceived" | "TriggerStarted" => EventClass::Start,
        "CodingAgentUserMessageSent" | "PromptInjected" => EventClass::Start,
        "SessionStarted"
        | "ContinuationStarted"
        | "CodingAgentSettingsChanged"
        | "CodingAgentBillingObserved"
        | "ChatSettingsChanged"
        | "ChatTurnSettingsResolved"
        | "CodingAgentCompacted"
        | "CodingAgentModelSubstituted"
        | "SuccessorThreadSpawned"
        | "SessionHandedOff"
        | "PlanProposed"
        | "PlanApproved"
        | "PlanStarted"
        | "HandoffCheckpointReached"
        | "SuccessorOfferDeclined"
        | "SuccessorRequested"
        | "ContextCeilingSet"
        | "ContextCeilingReached"
        | "SessionHandoffWithdrawn" => EventClass::Metadata,
        "VoiceSessionStarted" | "VoiceSessionEnded" => EventClass::Metadata,
        // INVARIANT: A spoken reply, for the same reason. The talker authors it mid-call,
        // often while the doer's own turn is still running. Classifying it
        // as a response would settle a turn it has no part in.
        "SpokenReplyGenerated" => EventClass::Metadata,
        // INVARIANT: What the caller said. Metadata, because it starts nothing: the
        // talker decides whether the doer is wanted, and Start here would
        // leave the thread waiting on a turn that never runs.
        "SpokenMessageReceived" => EventClass::Metadata,
        "SideQuestionAsked"
        | "SideQuestionAnswered"
        | "SideQuestionFailed"
        | "SideQuestionDismissed" => EventClass::Metadata,
        "WorkDelegated" => EventClass::Start,
        "MergeConflictDetected" | "MissingHardeningDetected" => EventClass::Start,
        "TextStreamed" | "ThoughtStreamed" | "MemoryRecalled" => EventClass::Activity,
        "ContextCaptured" | "ModelCallFailed" => EventClass::Metadata,
        "ToolCalled" | "ToolResult" => EventClass::Activity,
        "TodoListWritten" => EventClass::Activity,
        "BackgroundBashStarted" | "BackgroundBashCompleted" | "ConfinementRunStarted" => {
            EventClass::Metadata
        }
        "OwnerApprovalRequested" | "OwnerApprovalSpent" => EventClass::Metadata,
        "MachineMadeWorkspaceChanged" => EventClass::Metadata,
        "CommandCheckpointed" | "CommandCheckpointReverted" => EventClass::Metadata,
        "WidgetShown" | "WidgetPinned" | "WidgetUnpinned" | "InlineWidgetSized" => {
            EventClass::Metadata
        }
        "ThreadMcpServerEnabled" | "ThreadMcpServerDisabled" => EventClass::Metadata,
        "HomeBoardArranged" => EventClass::Metadata,
        "CodingAgentTextStreamed"
        | "CodingAgentThoughtStreamed"
        | "CodingAgentToolCalled"
        | "CodingAgentToolResult" => EventClass::Activity,
        "CodingAgentPromptSent" => EventClass::Activity,
        "CredentialRequested" | "McpConsentRequested" => EventClass::Activity,
        // INVARIANT: Form requests arrive inside the turn that asked, like the tool step
        // they follow. The resolution usually lands after the turn ended, so it
        // is Metadata: an answered form must not read as the thread working.
        "PluginInstallRequested"
        | "PluginUninstallRequested"
        | "EmailConfirmRequested"
        | "OAuthAuthorizationRequested" => EventClass::Activity,
        "FormRequestResolved" => EventClass::Metadata,
        "UserQuestionQueued" => EventClass::Metadata,
        "ResponseGenerated" | "ResponseCanceled" | "ResponseAborted" => EventClass::Terminal,
        "SessionEnded" | "ThreadArchived" | "TriggerCompleted" => EventClass::Terminal,
        "ChangeApplied" | "ChangeDiscarded" | "ChangeReverted" | "ChangeApplyFailed" => {
            EventClass::Terminal
        }
        "ResponseFailed"
        | "CodingAgentIdled"
        | "ChangeProposed"
        | "UserQuestionAsked"
        | "CodingAgentPermissionRequest"
        | "CommandPermissionRequested"
        | "McpPermissionRequested" => EventClass::ActionRequired,
        "ContinuationRequested" => EventClass::Start,
        // INVARIANT: UserQuestionAnswered is a step inside the same exchange as the
        // question — Activity, not Start. The status transition still moves
        // to Running so the resumed CC turn shows as in-progress.
        "UserQuestionAnswered"
        | "CodingAgentPermissionResolved"
        | "CommandPermissionResolved"
        | "McpPermissionResolved" => EventClass::Activity,
        "WorktreeCleaned" => EventClass::Metadata,
        "ChangeHardened"
        | "ChangeSummarized"
        | "ChangeSetAside"
        | "ChangeBroughtBack"
        | "ChangeWithdrawn"
        | "ProposalWithheld"
        | "ChangeQueued"
        | "ChangeHandoffStarted"
        | "ChangeUnqueued"
        | "MergeResolutionStarted"
        | "MergeResolutionCleared" => EventClass::Metadata,
        "ChildThreadCompleted" => EventClass::Start,
        "ChildThreadStopped" => EventClass::Metadata,
        "ChildReportFiled" => EventClass::Metadata,
        "ChildThreadDetached" => EventClass::Metadata,
        "ContextDismissed" | "ContextKeptOpen" => EventClass::Metadata,
        "WorkingUnderstandingWritten" => EventClass::Metadata,
        // INVARIANT: Engine-internal Flash enrichment of a prior MessageReceived's
        // attached images. The originating MessageReceived already moved
        // the thread into Running; this event arrives one or more
        // iterations later as a derived past-tense fact and must NOT
        // disturb the section/status machinery.
        "ImageDescribed" => EventClass::Metadata,
        "ConversationSummarized" => EventClass::Metadata,
        // INVARIANT: Event-wait lifecycle. Registration is Activity, not ActionRequired:
        // the thread subscribed to something the SYSTEM will deliver, so it must
        // never read as needing the user. The three resolutions are Activity for
        // the same reason `UserQuestionAnswered` is: none of them opens a fresh
        // round of work. A delivery and an expiry resume the turn that parked,
        // and the delivery's own `PromptInjected` is the Start event that
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
        "CacheKeepAliveStatusChanged" | "EventWaitTimingUpdated" => EventClass::Metadata,
        "CacheKeepAliveTurnedOff" | "CacheKeepAliveTurnedOn" => EventClass::Metadata,
        "WakeExpected" => EventClass::Metadata,
        _ => return None,
    })
}

pub fn all_persisted_event_types() -> Vec<&'static str> {
    vec![
        "MessageReceived",
        "QueuedMessageRemoved",
        "MessageHeld",
        "HeldMessageReleased",
        "TextStreamed",
        "ThoughtStreamed",
        "ContextCaptured",
        "ModelCallFailed",
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
        "CodingAgentInputRead",
        "CodingAgentInputDiscarded",
        "ChildReportTaken",
        "CodingAgentIdled",
        "ContinuationRequested",
        "MissingHardeningDetected",
        "ThreadTitleGenerated",
        "ThreadTitleRenamed",
        "ThreadSaved",
        "ThreadUnsaved",
        "ThreadMovedToFolder",
        "ThreadArchived",
        "ThreadArchiveRequested",
        "ThreadUnarchived",
        "ThreadReadRequested",
        "ThreadReadNotRequested",
        "ParentWakeRequested",
        "ParentWakeNotRequested",
        "ApplyNowStarted",
        "ChangeApplyRequested",
        "ApplyNowEnded",
        "ThreadReplySeen",
        "ThreadTriageProposed",
        "RootThreadDriverTaken",
        "RootThreadDriverReleased",
        "RootThreadGateEscalated",
        "RootThreadWatched",
        "RootThreadUnwatched",
        "UserOnlyMessagePosted",
        "AgentPaused",
        "AgentResumed",
        "AutomationPaused",
        "AutomationUnpaused",
        "ThreadHoldPlaced",
        "ThreadHoldLifted",
        "PermissionGranted",
        "PermissionSpent",
        "PermissionRestored",
        "PermissionRevoked",
        "PermissionRequestPrepared",
        "BatchItemRan",
        "ThreadStarted",
        "ThreadDiscarded",
        "HomeThreadCreated",
        "ImageUploaded",
        "TriggerStarted",
        "TriggerRunInterrupted",
        "TriggerCompleted",
        "ChangeProposed",
        "ChangeApplied",
        "ChangeDiscarded",
        "ChangeReverted",
        "ChangeSetAside",
        "ChangeBroughtBack",
        "ChangeWithdrawn",
        "ProposalWithheld",
        "ChangeQueued",
        "ChangeHandoffStarted",
        "ChangeUnqueued",
        "ChangeApplyFailed",
        "ChangeHardened",
        "ChangeSummarized",
        "MergeConflictDetected",
        "MergeResolutionStarted",
        "MergeResolutionCleared",
        "PromptInjected",
        "CredentialRequested",
        "PluginInstallRequested",
        "PluginUninstallRequested",
        "EmailConfirmRequested",
        "OAuthAuthorizationRequested",
        "FormRequestResolved",
        "McpConsentRequested",
        "CodingAgentSettingsChanged",
        "CodingAgentBillingObserved",
        "ChatSettingsChanged",
        "ChatTurnSettingsResolved",
        "CodingAgentCompacted",
        "CodingAgentModelSubstituted",
        "SuccessorThreadSpawned",
        "SessionHandedOff",
        "PlanStarted",
        "PlanProposed",
        "PlanApproved",
        "HandoffCheckpointReached",
        "SuccessorOfferDeclined",
        "SuccessorRequested",
        "ContextCeilingSet",
        "ContextCeilingReached",
        "SessionHandoffWithdrawn",
        "UserQuestionAsked",
        "UserQuestionQueued",
        "UserQuestionAnswered",
        "CodingAgentPermissionRequest",
        "CodingAgentPermissionResolved",
        "CommandPermissionRequested",
        "CommandPermissionResolved",
        "McpPermissionRequested",
        "McpPermissionResolved",
        "WorktreeCleaned",
        "ChildThreadCompleted",
        "ChildReportFiled",
        "ChildThreadStopped",
        "ChildThreadDetached",
        "ContextDismissed",
        "ContextKeptOpen",
        "BackgroundBashStarted",
        "BackgroundBashCompleted",
        "ConfinementRunStarted",
        "OwnerApprovalRequested",
        "OwnerApprovalSpent",
        "MachineMadeWorkspaceChanged",
        "ImageDescribed",
        // INVARIANT: The cached older-turn summary. Persisted because the event IS the
        // cache: there is no table, and a later turn rebuilds it from here.
        "ConversationSummarized",
        "CommandCheckpointed",
        "CommandCheckpointReverted",
        // INVARIANT: Event-wait lifecycle. Persisted because the wait IS the event: there
        // is no table, and the dispatcher's live set is rebuilt from these.
        "EventWaitStarted",
        "EventWaitTimingUpdated",
        "EventWaitDelivered",
        "EventWaitExpired",
        "EventWaitCanceled",
        // INVARIANT: The keep-alive status and the per-thread switch. Persisted because
        // their columns project from them.
        "CacheKeepAliveStatusChanged",
        "CacheKeepAliveTurnedOff",
        "CacheKeepAliveTurnedOn",
        "WakeExpected",
        // INVARIANT: Voice-session lifecycle. Persisted because the pair IS the record:
        // there is no session table, and the boot sweep finds an unpaired
        // start by reading these back.
        "VoiceSessionStarted",
        "VoiceSessionEnded",
        // INVARIANT: What the caller actually heard. Persisted because the audio is not,
        // so this text is the only record of a spoken turn.
        "SpokenReplyGenerated",
        // INVARIANT: The other half of a call's transcript: an utterance the talker
        // answered alone, and the talker's own request for the doer. Persisted
        // for the same reason, since neither has a row anywhere else.
        "SpokenMessageReceived",
        "WorkDelegated",
        "SideQuestionAsked",
        "SideQuestionAnswered",
        "SideQuestionFailed",
        "SideQuestionDismissed",
        "WidgetShown",
        "WidgetPinned",
        "WidgetUnpinned",
        "InlineWidgetSized",
        "ThreadMcpServerEnabled",
        "ThreadMcpServerDisabled",
        "HomeBoardArranged",
    ]
}

pub const CHAT_LEGAL_SECTIONS: &[ArchiveState] = &[ArchiveState::Archived, ArchiveState::Inbox];
pub const CC_LEGAL_SECTIONS: &[ArchiveState] = &[ArchiveState::Archived, ArchiveState::Inbox];

pub fn is_section_legal(thread_type: ThreadType, section: ArchiveState) -> bool {
    match thread_type {
        ThreadType::Chat => CHAT_LEGAL_SECTIONS.contains(&section),
        ThreadType::CodingAgent => CC_LEGAL_SECTIONS.contains(&section),
    }
}

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
        "ResponseGenerated" => match thread_type {
            ThreadType::Chat => to_inbox,
            ThreadType::CodingAgent => no_change,
        },
        "CodingAgentIdled" => match thread_type {
            ThreadType::CodingAgent => to_inbox,
            ThreadType::Chat => violation("CodingAgentIdled is CC-only"),
        },
        "ChangeApplied" | "ChangeDiscarded" => no_change,
        "ThreadArchived" => to_archived,
        "ThreadSaved" => to_inbox,
        "ThreadUnarchived" => to_inbox,
        "ContextCeilingSet" => no_change,
        "SessionStarted"
        | "SessionEnded"
        | "CodingAgentTextStreamed"
        | "CodingAgentThoughtStreamed"
        | "CodingAgentToolCalled"
        | "CodingAgentToolResult"
        | "CodingAgentUserMessageSent"
        | "CodingAgentPromptSent"
        | "CodingAgentInputRead"
        | "CodingAgentInputDiscarded"
        | "ChildReportTaken"
        | "MissingHardeningDetected"
        | "CodingAgentSettingsChanged"
        | "CodingAgentBillingObserved"
        | "CodingAgentCompacted"
        | "CodingAgentModelSubstituted"
        | "SuccessorThreadSpawned"
        | "SessionHandedOff"
        | "PlanProposed"
        | "PlanApproved"
        | "PlanStarted"
        | "HandoffCheckpointReached"
        | "SuccessorOfferDeclined"
        | "SuccessorRequested"
        | "ContextCeilingReached"
        | "SessionHandoffWithdrawn"
        | "ContinuationRequested" => match thread_type {
            ThreadType::CodingAgent => no_change,
            ThreadType::Chat => violation("coding-agent-specific event on Chat thread"),
        },
        // INVARIANT: User-attended terminals surface the thread in REVIEW so the user can
        // save or archive it. Without this, a cancel/abort on an already-
        // archived thread leaves it actionless: `available_thread_actions`
        // returns no close actions when stored_section != Inbox.
        "ResponseAborted" | "ResponseFailed" | "ResponseCanceled" => to_inbox,
        "TriggerRunInterrupted" => to_inbox,
        "ChangeProposed" => match thread_type {
            ThreadType::CodingAgent => to_inbox,
            ThreadType::Chat => violation("ChangeProposed is coding-agent-only"),
        },
        "ChangeBroughtBack" => match thread_type {
            ThreadType::CodingAgent => to_inbox,
            ThreadType::Chat => violation("ChangeBroughtBack is coding-agent-only"),
        },
        "ChangeWithdrawn" => match thread_type {
            ThreadType::CodingAgent => to_inbox,
            ThreadType::Chat => violation("ChangeWithdrawn is coding-agent-only"),
        },
        "ProposalWithheld" => match thread_type {
            ThreadType::CodingAgent => no_change,
            ThreadType::Chat => violation("ProposalWithheld is coding-agent-only"),
        },
        "ChangeQueued" | "ChangeHandoffStarted" | "ChangeUnqueued" => match thread_type {
            ThreadType::CodingAgent => no_change,
            ThreadType::Chat => violation("the landing lane is coding-agent-only"),
        },
        "UserQuestionAsked" => to_inbox,
        "UserQuestionAnswered" => no_change,
        "CodingAgentPermissionRequest" => match thread_type {
            ThreadType::CodingAgent => to_inbox,
            ThreadType::Chat => violation("CodingAgentPermissionRequest is CC-only"),
        },
        "CodingAgentPermissionResolved" => match thread_type {
            ThreadType::CodingAgent => no_change,
            ThreadType::Chat => violation("CodingAgentPermissionResolved is CC-only"),
        },
        "CommandPermissionRequested" => match thread_type {
            ThreadType::Chat => to_inbox,
            ThreadType::CodingAgent => violation("CommandPermissionRequested is chat-only"),
        },
        "CommandPermissionResolved" => match thread_type {
            ThreadType::Chat => no_change,
            ThreadType::CodingAgent => violation("CommandPermissionResolved is chat-only"),
        },
        "McpPermissionRequested" => match thread_type {
            ThreadType::Chat => to_inbox,
            ThreadType::CodingAgent => violation("McpPermissionRequested is chat-only"),
        },
        "McpPermissionResolved" => match thread_type {
            ThreadType::Chat => no_change,
            ThreadType::CodingAgent => violation("McpPermissionResolved is chat-only"),
        },
        "ChatSettingsChanged" | "ChatTurnSettingsResolved" => match thread_type {
            ThreadType::Chat => no_change,
            ThreadType::CodingAgent => violation("Chat settings events are chat-only"),
        },
        "MessageReceived"
        | "QueuedMessageRemoved"
        | "MessageHeld"
        | "HeldMessageReleased"
        | "TextStreamed"
        | "ThoughtStreamed"
        | "ContextCaptured"
        | "ModelCallFailed"
        | "MemoryRecalled"
        | "ToolCalled"
        | "ToolResult"
        | "TodoListWritten"
        | "WorkingUnderstandingWritten"
        | "ContinuationStarted"
        | "ThreadTitleGenerated"
        | "ThreadTitleRenamed"
        | "ThreadUnsaved"
        | "ThreadMovedToFolder"
        | "ThreadArchiveRequested"
        | "ThreadReadRequested"
        | "ThreadReadNotRequested"
        | "ParentWakeRequested"
        | "ParentWakeNotRequested"
        | "ApplyNowStarted"
        | "ChangeApplyRequested"
        | "ApplyNowEnded"
        | "ThreadReplySeen"
        | "ThreadTriageProposed"
        | "WorkItemDriverTaken"
        | "RootThreadDriverTaken"
        | "WorkItemDriverReleased"
        | "RootThreadDriverReleased"
        | "WorkItemGateEscalated"
        | "RootThreadGateEscalated"
        | "WorkItemWatched"
        | "RootThreadWatched"
        | "WorkItemUnwatched"
        | "RootThreadUnwatched"
        | "UserOnlyMessagePosted"
        | "AgentPaused"
        | "AgentResumed"
        | "AutomationPaused"
        | "AutomationUnpaused"
        | "ThreadHoldPlaced"
        | "ThreadHoldLifted"
        | "PermissionGranted"
        | "PermissionSpent"
        | "PermissionRestored"
        | "PermissionRevoked"
        | "PermissionRequestPrepared"
        | "BatchItemRan"
        | "TriggerStarted"
        | "TriggerCompleted"
        | "ChangeReverted"
        | "ChangeSetAside"
        | "ChangeApplyFailed"
        | "ChangeHardened"
        | "ChangeSummarized"
        | "MergeConflictDetected"
        | "MergeResolutionStarted"
        | "MergeResolutionCleared"
        | "PromptInjected"
        | "CredentialRequested"
        | "PluginInstallRequested"
        | "PluginUninstallRequested"
        | "EmailConfirmRequested"
        | "OAuthAuthorizationRequested"
        // INVARIANT: A form request resolves whenever the user answers, often long after
        // the turn, and in whatever section the thread sits by then.
        | "FormRequestResolved"
        | "UserQuestionQueued"
        | "McpConsentRequested"
        | "ThreadStarted"
        | "ThreadDiscarded"
        | "HomeThreadCreated"
        | "ImageUploaded"
        | "WorktreeCleaned"
        // INVARIANT: Phase 4 fan-in events — typed callback emitted onto the parent
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
        | "ChildThreadStopped"
        | "ChildReportFiled"
        | "ChildThreadDetached"
        | "ContextDismissed"
        | "ContextKeptOpen"
        | "BackgroundBashStarted"
        | "BackgroundBashCompleted"
        | "ConfinementRunStarted"
        | "OwnerApprovalRequested"
        | "OwnerApprovalSpent"
        | "MachineMadeWorkspaceChanged"
        | "ImageDescribed"
        | "ConversationSummarized"
        | "CommandCheckpointed"
        | "CommandCheckpointReverted"
        | "EventWaitStarted"
        | "EventWaitTimingUpdated"
        | "EventWaitDelivered"
        | "EventWaitExpired"
        | "EventWaitCanceled"
        // INVARIANT: The keep-alive status reports what the engine does while the thread
        // waits. It moves nothing.
        | "CacheKeepAliveStatusChanged"
        | "CacheKeepAliveTurnedOff"
        | "CacheKeepAliveTurnedOn"
        | "WakeExpected"
        | "VoiceSessionStarted"
        | "VoiceSessionEnded"
        | "SpokenReplyGenerated"
        | "SpokenMessageReceived"
        | "WorkDelegated"
        | "SideQuestionAsked"
        | "SideQuestionAnswered"
        | "SideQuestionFailed"
        | "SideQuestionDismissed"
        | "WidgetShown"
        | "WidgetPinned"
        | "WidgetUnpinned"
        | "InlineWidgetSized"
        | "ThreadMcpServerEnabled"
        | "ThreadMcpServerDisabled" => no_change,
        | "HomeBoardArranged" => no_change,
        _ => violation("Unknown event type"),
    }?;

    if is_unattended
        && thread_type != ThreadType::CodingAgent
        && result.new_section == Some(ArchiveState::Inbox)
        && !WAITING_FOR_USER_ANSWER_EVENTS.contains(&event_type)
        && event_type != "ThreadUnarchived"
        && event_type != "TriggerRunInterrupted"
    {
        return Ok(TransitionResult {
            new_section: Some(ArchiveState::Archived),
        });
    }

    Ok(result)
}

pub fn is_retention_legal(archive_state: ArchiveState, is_saved: bool) -> bool {
    !is_saved || archive_state == ArchiveState::Inbox
}

pub fn display_section(
    stored: ArchiveState,
    status: ThreadStatus,
    is_saved: bool,
    has_active_children: bool,
    has_pending_changes: bool,
    has_attention_descendants: bool,
    is_held: bool,
) -> DisplaySection {
    if is_saved {
        return DisplaySection::Saved;
    }
    if is_held {
        return DisplaySection::OnHold;
    }
    let demands_surface = status == ThreadStatus::Running
        || has_active_children
        || has_pending_changes
        || has_attention_descendants;
    if stored == ArchiveState::Archived && !demands_surface {
        return DisplaySection::Archive;
    }
    DisplaySection::Current
}

pub fn awaits_owner_answer(status: ThreadStatus, is_held: bool) -> bool {
    status == ThreadStatus::WaitingForUserAnswer && !is_held
}

/// INVARIANT: A thread is "blocking" iff archiving its ancestor would silently strand
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
/// **SQL mirrors**: keep in sync when the predicate changes.
/// - `event_bus_projection_propagation.rs::BLOCKING_DESCENDANT_FILTER`, which
///   both the boot rebuild and the per-ancestor reconcile read.
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
    own_blocker(
        thread_type,
        status,
        archive_state,
        if has_pending_changes {
            ChangeWork::Proposed
        } else {
            ChangeWork::None
        },
        is_external_repo,
    )
    .is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStateKind {
    None,
    Unproposed,
    Proposed,
}

impl ChangeStateKind {
    pub const ALL: [Self; 3] = [Self::None, Self::Unproposed, Self::Proposed];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Unproposed => "unproposed",
            Self::Proposed => "proposed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == value)
    }
}

impl sqlx::Type<sqlx::Postgres> for ChangeStateKind {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <&str as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <&str as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for ChangeStateKind {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let raw = <&str as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        Self::parse(raw).ok_or_else(|| format!("unknown change state {raw:?}").into())
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for ChangeStateKind {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.as_str(), buf)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CodingAgentChangeState {
    None,
    Unproposed { reason: Option<UnproposedReason> },
    Proposed { requires_restart: bool },
}

impl CodingAgentChangeState {
    pub fn kind(self) -> ChangeStateKind {
        match self {
            Self::None => ChangeStateKind::None,
            Self::Unproposed { .. } => ChangeStateKind::Unproposed,
            Self::Proposed { .. } => ChangeStateKind::Proposed,
        }
    }

    pub fn is_proposed(self) -> bool {
        self.kind() == ChangeStateKind::Proposed
    }

    /// INVARIANT: From the three `thread_summaries` columns. Their `CHECK` constraints
    /// admit only legal rows, so an unreadable kind reads as unproposed work:
    /// the direction that never hides a branch's commits.
    pub fn from_columns(kind: &str, reason: Option<&str>, requires_restart: bool) -> Self {
        match ChangeStateKind::parse(kind) {
            Some(ChangeStateKind::None) => Self::None,
            Some(ChangeStateKind::Proposed) => Self::Proposed { requires_restart },
            Some(ChangeStateKind::Unproposed) => Self::Unproposed {
                reason: reason.and_then(UnproposedReason::parse),
            },
            None => {
                crate::log_legacy!("[ThreadLifecycle] Unknown coding_agent_change_state {kind:?}");
                Self::Unproposed { reason: None }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeWork {
    None,
    Proposed,
    Held,
}

impl ChangeWork {
    pub const ALL: [Self; 3] = [Self::None, Self::Proposed, Self::Held];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Proposed => "proposed",
            Self::Held => "held",
        }
    }

    pub fn of(change: ChangeStateKind, holds_live_wait: bool) -> Self {
        match change {
            ChangeStateKind::Proposed => Self::Proposed,
            ChangeStateKind::Unproposed if holds_live_wait => Self::Held,
            _ => Self::None,
        }
    }
}

/// INVARIANT: Which of `is_blocking`'s clauses holds for one thread. Declaration order is
/// priority, so `min()` over a family picks the reason to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OwnBlocker {
    Running,
    Question,
    PendingChange,
    HeldProposal,
}

impl OwnBlocker {
    pub const ALL: [Self; 4] = [
        Self::Running,
        Self::Question,
        Self::PendingChange,
        Self::HeldProposal,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Question => "question",
            Self::PendingChange => "pending_change",
            Self::HeldProposal => "held_proposal",
        }
    }
}

pub fn own_blocker(
    thread_type: ThreadType,
    status: ThreadStatus,
    archive_state: ArchiveState,
    change: ChangeWork,
    is_external_repo: bool,
) -> Option<OwnBlocker> {
    match status {
        ThreadStatus::Running => return Some(OwnBlocker::Running),
        ThreadStatus::WaitingForUserAnswer => return Some(OwnBlocker::Question),
        _ => {}
    }
    if archive_state == ArchiveState::Archived
        || thread_type != ThreadType::CodingAgent
        || is_external_repo
    {
        return None;
    }
    match change {
        ChangeWork::None => None,
        ChangeWork::Proposed => Some(OwnBlocker::PendingChange),
        ChangeWork::Held => Some(OwnBlocker::HeldProposal),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocker {
    None,
    Home,
    Running,
    Question,
    PendingChange,
    HeldProposal,
    DescendantRunning,
    DescendantQuestion,
    DescendantPendingChange,
    DescendantHeldProposal,
}

impl Blocker {
    pub const ALL: [Self; 10] = [
        Self::None,
        Self::Home,
        Self::Running,
        Self::Question,
        Self::PendingChange,
        Self::HeldProposal,
        Self::DescendantRunning,
        Self::DescendantQuestion,
        Self::DescendantPendingChange,
        Self::DescendantHeldProposal,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Home => "home",
            Self::Running => "running",
            Self::Question => "question",
            Self::PendingChange => "pending_change",
            Self::HeldProposal => "held_proposal",
            Self::DescendantRunning => "descendant_running",
            Self::DescendantQuestion => "descendant_question",
            Self::DescendantPendingChange => "descendant_pending_change",
            Self::DescendantHeldProposal => "descendant_held_proposal",
        }
    }
}

pub fn action_blocker(
    own: Option<OwnBlocker>,
    is_home: bool,
    descendant: Option<OwnBlocker>,
) -> Blocker {
    if is_home {
        return Blocker::Home;
    }
    match (own, descendant) {
        (Some(OwnBlocker::Running), _) => Blocker::Running,
        (Some(OwnBlocker::Question), _) => Blocker::Question,
        (Some(OwnBlocker::PendingChange), _) => Blocker::PendingChange,
        (Some(OwnBlocker::HeldProposal), _) => Blocker::HeldProposal,
        (None, Some(OwnBlocker::Running)) => Blocker::DescendantRunning,
        (None, Some(OwnBlocker::Question)) => Blocker::DescendantQuestion,
        (None, Some(OwnBlocker::PendingChange)) => Blocker::DescendantPendingChange,
        (None, Some(OwnBlocker::HeldProposal)) => Blocker::DescendantHeldProposal,
        (None, None) => Blocker::None,
    }
}

pub fn check_archive_allowed(
    thread_type: ThreadType,
    current_section: ArchiveState,
    status: ThreadStatus,
    is_home: bool,
) -> Result<(), LifecycleViolation> {
    let reason = if is_home {
        "the home thread is never archived"
    } else if status == ThreadStatus::WaitingForUserAnswer {
        "the thread is waiting on the user; answer or stop it first"
    } else {
        return Ok(());
    };
    Err(LifecycleViolation {
        event_type: "ThreadArchived".to_string(),
        thread_type,
        current_section,
        reason: reason.to_string(),
    })
}

pub fn check_title_generated_allowed(
    thread_type: ThreadType,
    current_section: ArchiveState,
    is_home: bool,
) -> Result<(), LifecycleViolation> {
    if !is_home {
        return Ok(());
    }
    Err(LifecycleViolation {
        event_type: "ThreadTitleGenerated".to_string(),
        thread_type,
        current_section,
        reason: "the home thread is named only by the user".to_string(),
    })
}

pub fn is_attention_needing(
    thread_type: ThreadType,
    status: ThreadStatus,
    archive_state: ArchiveState,
    has_pending_changes: bool,
    is_external_repo: bool,
    is_stopped_child: bool,
    is_held: bool,
) -> bool {
    if awaits_owner_answer(status, is_held) {
        return true;
    }
    if archive_state == ArchiveState::Archived {
        return false;
    }
    if has_pending_changes && thread_type == ThreadType::CodingAgent && !is_external_repo {
        return true;
    }
    is_stopped_child
}

#[allow(clippy::too_many_arguments)]
pub fn available_thread_actions(
    thread_type: ThreadType,
    status: ThreadStatus,
    stored_section: ArchiveState,
    has_pending_changes: bool,
    descendants_block_archive: bool,
    has_live_event_waits: bool,
    has_unsent_draft: bool,
    is_saved: bool,
) -> Vec<ThreadAction> {
    let mut actions = Vec::new();
    let live = status == ThreadStatus::Running || status == ThreadStatus::WaitingForUserAnswer;
    let coding_agent_pending = has_pending_changes && thread_type == ThreadType::CodingAgent;

    if has_unsent_draft {
        actions.push(ThreadAction::DiscardDraft);
    }
    if !live {
        if coding_agent_pending {
            if !has_live_event_waits {
                actions.push(ThreadAction::Discard);
                actions.push(ThreadAction::Apply);
                actions.push(ThreadAction::Hold);
            }
        } else if stored_section == ArchiveState::Inbox && !descendants_block_archive {
            actions.push(ThreadAction::Archive);
        }
    }
    let watching = has_live_event_waits
        && status != ThreadStatus::WaitingForUserAnswer
        && status != ThreadStatus::Failed;
    if thread_type == ThreadType::CodingAgent
        && (status == ThreadStatus::Running || status == ThreadStatus::Paused || watching)
    {
        actions.push(ThreadAction::ApplyWhenSettled);
    }
    if is_saved {
        actions.push(ThreadAction::Unsave);
    } else {
        actions.push(ThreadAction::Save);
    }
    actions
}

/// INVARIANT: Event types whose projection sets `last_activity = NOW()` in
/// `event_bus_projection_thread.rs`. Must match exactly the match arms in
/// `EventBus::update_thread_projection()`.
/// The frontend uses this to keep thread.meta.updatedAt in sync with the backend.
pub const LAST_ACTIVITY_EVENTS: &[&str] = &[
    "MessageReceived",
    "TriggerStarted",
    "ResponseGenerated",
    "ResponseAborted",
    "CodingAgentIdled",
    "ChangeApplied",
    "ChangeDiscarded",
    "CodingAgentUserMessageSent",
    "PromptInjected",
    "TriggerCompleted",
    "UserQuestionAsked",
    "UserQuestionAnswered",
    "CodingAgentPermissionRequest",
    "CodingAgentPermissionResolved",
    "CommandPermissionRequested",
    "CommandPermissionResolved",
    "McpPermissionRequested",
    "McpPermissionResolved",
    "ContinuationRequested",
    "ToolCalled",
    "ToolResult",
    "TextStreamed",
    "ThoughtStreamed",
    "MemoryRecalled",
    "CodingAgentTextStreamed",
    "CodingAgentThoughtStreamed",
    "CodingAgentToolCalled",
    "CodingAgentToolResult",
    "VoiceSessionStarted",
    "SpokenMessageReceived",
    "SpokenReplyGenerated",
    "UserOnlyMessagePosted",
];

/// INVARIANT: Event types that increment message_count in the thread_summaries projection.
/// Must match the events that do `message_count + 1` in
/// `event_bus_projection_thread.rs`.
pub const MESSAGE_COUNT_EVENTS: &[&str] = &[
    "MessageReceived",
    "TriggerStarted",
    "CodingAgentUserMessageSent",
    "PromptInjected",
];

pub const WAITING_FOR_USER_ANSWER_EVENTS: &[&str] = &[
    "UserQuestionAsked",
    "CodingAgentPermissionRequest",
    "CommandPermissionRequested",
    "McpPermissionRequested",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusRule {
    Set(ThreadStatus),
    NoChange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CcFlagRule {
    ClearAll,
    SetChanges,
    FromPayload,
    None,
}

pub struct StatusTransition {
    pub status: StatusRule,
    pub cc_flags: CcFlagRule,
}

pub fn status_transitions() -> Vec<(&'static str, StatusTransition)> {
    vec![
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
        // INVARIANT: The projection keeps `WaitingForUserAnswer` instead: an injection
        // under an open question waits for the answer. The coarse rule model
        // cannot express that, as with `preserving_verdict`.
        (
            "PromptInjected",
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
        // INVARIANT: Only a read with `started_turn` moves status; every other read
        // moves nothing. See event_bus_projection_thread.rs.
        (
            "CodingAgentInputRead",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        (
            "ContinuationRequested",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        // INVARIANT: Completed work → idle. Projection-only exception: eligible automatic
        // successors keep running across ResponseGenerated, CodingAgentIdled
        // and SessionEnded; the coarse table has no durable handoff axis.
        // A pending change shows through change state, never through status.
        (
            "ResponseGenerated",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Idle),
                cc_flags: CcFlagRule::None,
            },
        ),
        // INVARIANT: Projection-only exception: a redirect cancel (`CancelCause::is_redirect`)
        // leaves the status alone, and so do the `CodingAgentIdled` and
        // `SessionEnded` after it, until the next start event
        // (`keeping_status_across_redirect`). The coarse rule model has no
        // cause axis.
        (
            "ResponseCanceled",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Idle),
                cc_flags: CcFlagRule::None,
            },
        ),
        // INVARIANT: Exception: reason=StaleResume skips this transition (handled imperatively
        // in event_bus_projection_thread.rs and thread-events.ts) because the
        // user's message is still being processed in a fresh session.
        (
            "SessionEnded",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Idle),
                cc_flags: CcFlagRule::None,
            },
        ),
        // INVARIANT: System interruption. Approximate on purpose: this table has no cause
        // or actor axis, and `AbortCause::status_sql()` splits three ways.
        // `StaleSettle` maps to 'idle' (engine cleanup of a row whose process was
        // already gone), and the user's own *Switch to new version* (an
        // `EngineShutdown` abort carrying a device actor) maps to 'paused' rather
        // than 'failed', because the engine resumes that turn itself. The row
        // below states the remaining case, which is every interruption nobody
        // promised to undo and IS genuinely failed; the split lives next to the
        // cause enum, on `AbortCause::promises_auto_resume()`.
        //
        // A pending change does not enter into it. The change surfaces through
        // the change state, and the frontend ranks it against the verdict.
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
        (
            "TriggerCompleted",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Idle),
                cc_flags: CcFlagRule::None,
            },
        ),
        // INVARIANT: CC lifecycle. The projection SQL also preserves a pre-existing
        // 'failed' status (`CASE WHEN status='failed' THEN 'failed' ELSE …`):
        // a failed CC turn emits `ResponseFailed` then this idle in the same
        // turn, and the idle must not downgrade the red error dot. The coarse
        // `StatusRule` model can't express verdict preservation or a pending
        // automatic successor that keeps running; see the
        // CodingAgentIdled arm in `event_bus_projection_thread.rs`.
        (
            "CodingAgentIdled",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Idle),
                cc_flags: CcFlagRule::FromPayload,
            },
        ),
        // INVARIANT: ChangeProposed only sets CC flags, not status. Status is already correct:
        // - If CC just idled → CodingAgentIdled already set 'idle'
        // - If CC is still running (mid-session commit) → stays 'running' (no premature buttons)
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
            "UserQuestionAsked",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::WaitingForUserAnswer),
                cc_flags: CcFlagRule::None,
            },
        ),
        // INVARIANT: A quiet answer (`quiet: true`) sets Idle instead: it wakes nothing.
        // The table keys on event type alone, so the projection owns that case.
        (
            "UserQuestionAnswered",
            StatusTransition {
                status: StatusRule::Set(ThreadStatus::Running),
                cc_flags: CcFlagRule::None,
            },
        ),
        // INVARIANT: CC permission prompt — pauses CC's tool call, surfaces a PermissionCard.
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
        // INVARIANT: **No event-wait row sets a status, and that absence is the rule, not
        // an omission.** A subscription does not hold its thread's turn:
        // registration happens mid-turn and the turn's own terminator decides
        // the status, while a resolution lands on a thread that is idle,
        // running something unrelated, or parked on a question. The delivery's
        // wake sets Running through its own `PromptInjected`, which is
        // where that transition belongs, and it leaves a parked question alone.
        // A cancel leaves the thread exactly as it found it.
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
        // INVARIANT: Activity events: defense in depth against premature-Idled drift.
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

#[path = "thread_lifecycle_codegen.rs"]
pub mod codegen;

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
