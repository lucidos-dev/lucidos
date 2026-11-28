use serde_json::Value;

use super::{EventMeta, ThreadEvent};

const GENERATE_IMAGE_TOOL: &str = "generate_image";

pub const KEEP_ALIVE_IDLE_REASON: &str = "cache_keep_alive";

pub const CEILING_REENTRY_IDLE_REASON: &str = "ceiling_reentry";

/// INVARIANT: The idle reasons of an engine turn: the agent did no work the user asked
/// for, so no trigger, event wait or turn gap reads its idle.
const ENGINE_TURN_IDLE_REASONS: [&str; 2] = [KEEP_ALIVE_IDLE_REASON, CEILING_REENTRY_IDLE_REASON];

pub const KEEP_ALIVE_ENDING_EVENT_TYPES: &[&str] =
    &["UserQuestionAsked", "CodingAgentPermissionRequest"];

/// INVARIANT: SQL true for every `events` row but an engine turn's idle, for each scan
/// that must agree with [`ThreadEvent::is_engine_turn_idle`]. `prefix` names
/// the table alias with its dot, or is empty.
pub fn not_an_engine_turn_idle_sql(prefix: &str) -> String {
    let reasons = ENGINE_TURN_IDLE_REASONS
        .map(|r| format!("'{r}'"))
        .join(", ");
    format!(
        "NOT ({prefix}event_type = 'CodingAgentIdled' \
         AND COALESCE({prefix}payload->>'reason', '') IN ({reasons}))"
    )
}

impl ThreadEvent {
    pub const TERMINATOR_EVENT_TYPES: &'static [&'static str] = &[
        "ResponseGenerated",
        "ResponseCanceled",
        "ResponseAborted",
        "ResponseFailed",
    ];

    pub const QUESTION_OVERTAKEN_EVENT_TYPES: &'static [&'static str] = &[
        "ResponseAborted",
        "ResponseCanceled",
        "ResponseFailed",
        "CodingAgentIdled",
        "CodingAgentTextStreamed",
        "CodingAgentToolCalled",
        "CodingAgentPromptSent",
        "TextStreamed",
        "ThoughtStreamed",
        "ToolCalled",
        "ToolResult",
    ];

    pub fn question_own_result_sql(question: &str, later: &str) -> String {
        format!(
            "({later}.event_type = 'CodingAgentToolResult' AND {})",
            Self::question_owns_result_id_sql(
                question,
                &format!("{later}.payload->>'tool_use_id'")
            )
        )
    }

    pub fn question_owns_result_id_sql(question: &str, result_id: &str) -> String {
        format!("({question}.payload->>'tool_use_id' = {result_id})")
    }

    pub fn strip_result_text(payload: &mut serde_json::Map<String, Value>) {
        payload.remove("result");
        payload.insert("result_stripped".to_string(), Value::Bool(true));
    }

    /// INVARIANT: Drop a tool call's `args` from its serialized payload and mark it, so the
    /// step modal fetches them from `GET /api/v1/events/:event_id/tool-args`.
    /// The snapshot and the live stream both call this, which keeps a stripped
    /// call one shape everywhere. A no-op for any other event type.
    ///
    /// `args` is the heaviest thing a call carries: a `Write`'s whole file, an
    /// `Edit`'s two hunks, a `bash` call's script. Nothing inline renders it.
    ///
    /// **`description` is filled first, and that ordering is the whole trick.**
    /// The inline label reads `description || describe(name, args)`, so a row
    /// with no description would otherwise keep only its bare tool name.
    pub fn strip_tool_call_args(event_type: &str, payload: &mut serde_json::Map<String, Value>) {
        let describe: fn(&str, &Value) -> String = match event_type {
            "ToolCalled" => crate::core::describe_tool,
            "CodingAgentToolCalled" => crate::core::describe_cc_tool,
            _ => return,
        };
        let name = payload.get("name").and_then(Value::as_str).unwrap_or("");
        // INVARIANT: The generated image takes its alt text from the call's prompt, so
        // this one tool's args are read inline and stay.
        if name == GENERATE_IMAGE_TOOL {
            return;
        }
        let described = payload
            .get("description")
            .and_then(Value::as_str)
            .is_some_and(|d| !d.is_empty());
        if !described {
            let args = payload.get("args").cloned().unwrap_or(Value::Null);
            let description = describe(name, &args);
            payload.insert("description".to_string(), description.into());
        }
        payload.remove("args");
        payload.insert("args_stripped".to_string(), Value::Bool(true));
    }

    pub fn event_type(&self) -> &'static str {
        match self {
            Self::MessageReceived { .. } => "MessageReceived",
            Self::QueuedMessageRemoved { .. } => "QueuedMessageRemoved",
            Self::MessageHeld { .. } => "MessageHeld",
            Self::HeldMessageReleased { .. } => "HeldMessageReleased",
            Self::TextStreamed { .. } => "TextStreamed",
            Self::ThoughtStreamed { .. } => "ThoughtStreamed",
            Self::ContextCaptured { .. } => "ContextCaptured",
            Self::CodingAgentBillingObserved { .. } => "CodingAgentBillingObserved",
            Self::ModelCallFailed { .. } => "ModelCallFailed",
            Self::MemoryRecalled { .. } => "MemoryRecalled",
            Self::ToolCalled { .. } => "ToolCalled",
            Self::ToolResult { .. } => "ToolResult",
            Self::TodoListWritten { .. } => "TodoListWritten",
            Self::WorkingUnderstandingWritten { .. } => "WorkingUnderstandingWritten",
            Self::BackgroundBashStarted { .. } => "BackgroundBashStarted",
            Self::BackgroundBashCompleted { .. } => "BackgroundBashCompleted",
            Self::ConfinementRunStarted { .. } => "ConfinementRunStarted",
            Self::ResponseGenerated { .. } => "ResponseGenerated",
            Self::ResponseCanceled { .. } => "ResponseCanceled",
            Self::ResponseAborted { .. } => "ResponseAborted",
            Self::ResponseFailed { .. } => "ResponseFailed",
            Self::ContinuationStarted { .. } => "ContinuationStarted",
            Self::SessionStarted { .. } => "SessionStarted",
            Self::SessionEnded { .. } => "SessionEnded",
            Self::CodingAgentTextStreamed { .. } => "CodingAgentTextStreamed",
            Self::CodingAgentThoughtStreamed { .. } => "CodingAgentThoughtStreamed",
            Self::CodingAgentToolCalled { .. } => "CodingAgentToolCalled",
            Self::CodingAgentToolResult { .. } => "CodingAgentToolResult",
            Self::CodingAgentUserMessageSent { .. } => "CodingAgentUserMessageSent",
            Self::CodingAgentPromptSent { .. } => "CodingAgentPromptSent",
            Self::CodingAgentInputRead { .. } => "CodingAgentInputRead",
            Self::CodingAgentInputDiscarded { .. } => "CodingAgentInputDiscarded",
            Self::ChildReportTaken { .. } => "ChildReportTaken",
            Self::MissingHardeningDetected { .. } => "MissingHardeningDetected",
            Self::CodingAgentIdled { .. } => "CodingAgentIdled",
            Self::ContinuationRequested { .. } => "ContinuationRequested",
            Self::ThreadTitleGenerated { .. } => "ThreadTitleGenerated",
            Self::ThreadTitleRenamed { .. } => "ThreadTitleRenamed",
            Self::ThreadSaved => "ThreadSaved",
            Self::ThreadUnsaved => "ThreadUnsaved",
            Self::ThreadMovedToFolder { .. } => "ThreadMovedToFolder",
            Self::ThreadArchived => "ThreadArchived",
            Self::ThreadArchiveRequested => "ThreadArchiveRequested",
            Self::ThreadUnarchived => "ThreadUnarchived",
            Self::ThreadReadRequested { .. } => "ThreadReadRequested",
            Self::ThreadReadNotRequested { .. } => "ThreadReadNotRequested",
            Self::ParentWakeRequested => "ParentWakeRequested",
            Self::ParentWakeNotRequested => "ParentWakeNotRequested",
            Self::ThreadReplySeen { .. } => "ThreadReplySeen",
            Self::ThreadTriageProposed { .. } => "ThreadTriageProposed",
            Self::RootThreadDriverTaken => "RootThreadDriverTaken",
            Self::RootThreadDriverReleased => "RootThreadDriverReleased",
            Self::RootThreadGateEscalated { .. } => "RootThreadGateEscalated",
            Self::RootThreadWatched { .. } => "RootThreadWatched",
            Self::RootThreadUnwatched => "RootThreadUnwatched",
            Self::UserOnlyMessagePosted { .. } => "UserOnlyMessagePosted",
            Self::AgentPaused { .. } => "AgentPaused",
            Self::AgentResumed { .. } => "AgentResumed",
            Self::AutomationPaused { .. } => "AutomationPaused",
            Self::AutomationUnpaused { .. } => "AutomationUnpaused",
            Self::ThreadHoldPlaced { .. } => "ThreadHoldPlaced",
            Self::ThreadHoldLifted { .. } => "ThreadHoldLifted",
            Self::PermissionGranted { .. } => "PermissionGranted",
            Self::PermissionSpent { .. } => "PermissionSpent",
            Self::PermissionRestored { .. } => "PermissionRestored",
            Self::PermissionRevoked { .. } => "PermissionRevoked",
            Self::PermissionRequestPrepared { .. } => "PermissionRequestPrepared",
            Self::BatchItemRan { .. } => "BatchItemRan",
            Self::ThreadStarted { .. } => "ThreadStarted",
            Self::ThreadDiscarded { .. } => "ThreadDiscarded",
            Self::HomeThreadCreated => "HomeThreadCreated",
            Self::ImageUploaded { .. } => "ImageUploaded",
            Self::TriggerStarted { .. } => "TriggerStarted",
            Self::TriggerRunInterrupted { .. } => "TriggerRunInterrupted",
            Self::TriggerCompleted { .. } => "TriggerCompleted",
            Self::PlanStarted { .. } => "PlanStarted",
            Self::PlanProposed { .. } => "PlanProposed",
            Self::PlanApproved { .. } => "PlanApproved",
            Self::ChangeApplyRequested { .. } => "ChangeApplyRequested",
            Self::ChangeProposed { .. } => "ChangeProposed",
            Self::ChangeApplied { .. } => "ChangeApplied",
            Self::ChangeDiscarded { .. } => "ChangeDiscarded",
            Self::ChangeReverted { .. } => "ChangeReverted",
            Self::ChangeSetAside { .. } => "ChangeSetAside",
            Self::ChangeBroughtBack { .. } => "ChangeBroughtBack",
            Self::ChangeWithdrawn { .. } => "ChangeWithdrawn",
            Self::ProposalWithheld { .. } => "ProposalWithheld",
            Self::ChangeQueued { .. } => "ChangeQueued",
            Self::ChangeHandoffStarted { .. } => "ChangeHandoffStarted",
            Self::ChangeUnqueued { .. } => "ChangeUnqueued",
            Self::ChangeApplyFailed { .. } => "ChangeApplyFailed",
            Self::ApplyNowStarted => "ApplyNowStarted",
            Self::ApplyNowEnded { .. } => "ApplyNowEnded",
            Self::MergeConflictDetected { .. } => "MergeConflictDetected",
            Self::MergeResolutionStarted { .. } => "MergeResolutionStarted",
            Self::MergeResolutionCleared { .. } => "MergeResolutionCleared",
            Self::ChangeHardened { .. } => "ChangeHardened",
            Self::ChangeSummarized { .. } => "ChangeSummarized",
            Self::CodingAgentSettingsChanged { .. } => "CodingAgentSettingsChanged",
            Self::ChatSettingsChanged { .. } => "ChatSettingsChanged",
            Self::ChatTurnSettingsResolved { .. } => "ChatTurnSettingsResolved",
            Self::CodingAgentCompacted { .. } => "CodingAgentCompacted",
            Self::CodingAgentModelSubstituted { .. } => "CodingAgentModelSubstituted",
            Self::SuccessorThreadSpawned { .. } => "SuccessorThreadSpawned",
            Self::SessionHandedOff { .. } => "SessionHandedOff",
            Self::HandoffCheckpointReached { .. } => "HandoffCheckpointReached",
            Self::SuccessorOfferDeclined { .. } => "SuccessorOfferDeclined",
            Self::ContextCeilingSet { .. } => "ContextCeilingSet",
            Self::ContextCeilingReached { .. } => "ContextCeilingReached",
            Self::SessionHandoffWithdrawn => "SessionHandoffWithdrawn",
            Self::SuccessorRequested { .. } => "SuccessorRequested",
            Self::PromptInjected { .. } => "PromptInjected",
            Self::CredentialRequested { .. } => "CredentialRequested",
            Self::PluginInstallRequested { .. } => "PluginInstallRequested",
            Self::PluginUninstallRequested { .. } => "PluginUninstallRequested",
            Self::EmailConfirmRequested { .. } => "EmailConfirmRequested",
            Self::CodingAgentCommandReceived { .. } => "CodingAgentCommandReceived",
            Self::CodingAgentCommandAnswered { .. } => "CodingAgentCommandAnswered",
            Self::CodingAgentMcpStatusesReported { .. } => "CodingAgentMcpStatusesReported",
            Self::AgentSignInNeeded { .. } => "AgentSignInNeeded",
            Self::TerminalOpened { .. } => "TerminalOpened",
            Self::TerminalExited { .. } => "TerminalExited",
            Self::TerminalCommandSuggested { .. } => "TerminalCommandSuggested",
            Self::AgentSignInRequested { .. } => "AgentSignInRequested",
            Self::OAuthAuthorizationRequested { .. } => "OAuthAuthorizationRequested",
            Self::FormRequestResolved { .. } => "FormRequestResolved",
            Self::McpConsentRequested { .. } => "McpConsentRequested",
            Self::UserQuestionAsked { .. } => "UserQuestionAsked",
            Self::UserQuestionQueued { .. } => "UserQuestionQueued",
            Self::UserQuestionAnswered { .. } => "UserQuestionAnswered",
            Self::OwnerApprovalRequested { .. } => "OwnerApprovalRequested",
            Self::OwnerApprovalSpent { .. } => "OwnerApprovalSpent",
            Self::MachineMadeWorkspaceChanged { .. } => "MachineMadeWorkspaceChanged",
            Self::CodingAgentPermissionRequest { .. } => "CodingAgentPermissionRequest",
            Self::CodingAgentPermissionResolved { .. } => "CodingAgentPermissionResolved",
            Self::CommandPermissionRequested { .. } => "CommandPermissionRequested",
            Self::CommandPermissionResolved { .. } => "CommandPermissionResolved",
            Self::McpPermissionRequested { .. } => "McpPermissionRequested",
            Self::McpPermissionResolved { .. } => "McpPermissionResolved",
            Self::CommandCheckpointed { .. } => "CommandCheckpointed",
            Self::CommandCheckpointReverted { .. } => "CommandCheckpointReverted",
            Self::WorktreeCleaned { .. } => "WorktreeCleaned",
            Self::ChildThreadCompleted { .. } => "ChildThreadCompleted",
            Self::ChildReportFiled { .. } => "ChildReportFiled",
            Self::ChildThreadStopped { .. } => "ChildThreadStopped",
            Self::ChildThreadDetached { .. } => "ChildThreadDetached",
            Self::ContextDismissed { .. } => "ContextDismissed",
            Self::ContextKeptOpen { .. } => "ContextKeptOpen",
            Self::ImageDescribed { .. } => "ImageDescribed",
            Self::ConversationSummarized { .. } => "ConversationSummarized",
            Self::EventWaitStarted { .. } => "EventWaitStarted",
            Self::EventWaitTimingUpdated { .. } => "EventWaitTimingUpdated",
            Self::EventWaitDelivered { .. } => "EventWaitDelivered",
            Self::EventWaitExpired { .. } => "EventWaitExpired",
            Self::EventWaitCanceled { .. } => "EventWaitCanceled",
            Self::CacheKeepAliveStatusChanged { .. } => "CacheKeepAliveStatusChanged",
            Self::CacheKeepAliveTurnedOff => "CacheKeepAliveTurnedOff",
            Self::CacheKeepAliveTurnedOn => "CacheKeepAliveTurnedOn",
            Self::WakeExpected { .. } => "WakeExpected",
            Self::VoiceSessionStarted { .. } => "VoiceSessionStarted",
            Self::VoiceSessionEnded { .. } => "VoiceSessionEnded",
            Self::SpokenReplyGenerated { .. } => "SpokenReplyGenerated",
            Self::SpokenMessageReceived { .. } => "SpokenMessageReceived",
            Self::WorkDelegated { .. } => "WorkDelegated",
            Self::SideQuestionAsked { .. } => "SideQuestionAsked",
            Self::SideQuestionAnswered { .. } => "SideQuestionAnswered",
            Self::SideQuestionFailed { .. } => "SideQuestionFailed",
            Self::SideQuestionDismissed { .. } => "SideQuestionDismissed",
            Self::WidgetShown { .. } => "WidgetShown",
            Self::WidgetPinned { .. } => "WidgetPinned",
            Self::WidgetUnpinned { .. } => "WidgetUnpinned",
            Self::InlineWidgetSized { .. } => "InlineWidgetSized",
            Self::ThreadMcpServerEnabled { .. } => "ThreadMcpServerEnabled",
            Self::ThreadMcpServerDisabled { .. } => "ThreadMcpServerDisabled",
            Self::HomeBoardArranged { .. } => "HomeBoardArranged",
            Self::CumulativeTextUpdated { .. } => "CumulativeTextUpdated",
            Self::LlmCallRetried { .. } => "LlmCallRetried",
            Self::PreambleCompleted => "PreambleCompleted",
            Self::PushNotificationRequested => "PushNotificationRequested",
            Self::AppUiRefreshRequested { .. } => "AppUiRefreshRequested",
            Self::AppUiCaptureRequested { .. } => "AppUiCaptureRequested",
            Self::WidgetFitRequested { .. } => "WidgetFitRequested",
            Self::NavigationRequested { .. } => "NavigationRequested",
            Self::CodingAgentThreadSpawned { .. } => "CodingAgentThreadSpawned",
            Self::CodingAgentDiffChanged { .. } => "CodingAgentDiffChanged",
            Self::ChildrenCountChanged { .. } => "ChildrenCountChanged",
        }
    }

    pub const RESERVED_TYPE_NAMES: &'static [&'static str] = &[
        "MessageReceived",
        "QueuedMessageRemoved",
        "MessageHeld",
        "HeldMessageReleased",
        "TextStreamed",
        "ThoughtStreamed",
        "ContextCaptured",
        "CodingAgentBillingObserved",
        "ModelCallFailed",
        "MemoryRecalled",
        "ToolCalled",
        "ToolResult",
        "TodoListWritten",
        "WorkingUnderstandingWritten",
        "BackgroundBashStarted",
        "BackgroundBashCompleted",
        "ConfinementRunStarted",
        "ResponseGenerated",
        "ResponseCanceled",
        "ResponseAborted",
        "ResponseFailed",
        "ContinuationStarted",
        "SessionStarted",
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
        "MissingHardeningDetected",
        "CodingAgentIdled",
        "ContinuationRequested",
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
        "PlanStarted",
        "PlanProposed",
        "PlanApproved",
        "ChangeApplyRequested",
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
        "ApplyNowStarted",
        "ApplyNowEnded",
        "MergeConflictDetected",
        "MergeResolutionStarted",
        "MergeResolutionCleared",
        "ChangeHardened",
        "ChangeSummarized",
        "CodingAgentSettingsChanged",
        "ChatSettingsChanged",
        "ChatTurnSettingsResolved",
        "CodingAgentCompacted",
        "CodingAgentModelSubstituted",
        "SuccessorThreadSpawned",
        "SessionHandedOff",
        "HandoffCheckpointReached",
        "SuccessorOfferDeclined",
        "SuccessorRequested",
        "ContextCeilingSet",
        "ContextCeilingReached",
        "SessionHandoffWithdrawn",
        "PromptInjected",
        "CredentialRequested",
        "PluginInstallRequested",
        "PluginUninstallRequested",
        "EmailConfirmRequested",
        "CodingAgentCommandReceived",
        "CodingAgentCommandAnswered",
        "CodingAgentMcpStatusesReported",
        "AgentSignInNeeded",
        "TerminalOpened",
        "TerminalExited",
        "TerminalCommandSuggested",
        "AgentSignInRequested",
        "OAuthAuthorizationRequested",
        "FormRequestResolved",
        "McpConsentRequested",
        "UserQuestionAsked",
        "UserQuestionQueued",
        "UserQuestionAnswered",
        "OwnerApprovalRequested",
        "OwnerApprovalSpent",
        "MachineMadeWorkspaceChanged",
        "CodingAgentPermissionRequest",
        "CodingAgentPermissionResolved",
        "CommandPermissionRequested",
        "CommandPermissionResolved",
        "McpPermissionRequested",
        "McpPermissionResolved",
        "CommandCheckpointed",
        "CommandCheckpointReverted",
        "WorktreeCleaned",
        "ChildThreadCompleted",
        "ChildReportFiled",
        "ChildThreadStopped",
        "ChildThreadDetached",
        "ContextDismissed",
        "ContextKeptOpen",
        "ImageDescribed",
        "ConversationSummarized",
        "EventWaitStarted",
        "EventWaitTimingUpdated",
        "EventWaitDelivered",
        "EventWaitExpired",
        "EventWaitCanceled",
        "CacheKeepAliveStatusChanged",
        "CacheKeepAliveTurnedOff",
        "CacheKeepAliveTurnedOn",
        "WakeExpected",
        "CumulativeTextUpdated",
        "LlmCallRetried",
        "PreambleCompleted",
        "PushNotificationRequested",
        "AppUiRefreshRequested",
        "AppUiCaptureRequested",
        "WidgetFitRequested",
        "NavigationRequested",
        "CodingAgentThreadSpawned",
        "CodingAgentDiffChanged",
        "ChildrenCountChanged",
        "VoiceSessionStarted",
        "VoiceSessionEnded",
        "SpokenMessageReceived",
        "WorkDelegated",
        "SpokenReplyGenerated",
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
    ];

    /// INVARIANT: The `#[serde(alias = ...)]` spellings, kept for rows written before a
    /// rename.
    ///
    /// Denied at the emit boundary alongside the current names. An alias
    /// deserializes INTO its variant, so a domain row named `Thinking` reads
    /// back as a `ThoughtStreamed` thread event to every consumer that parses
    /// one. Refusing the new name and allowing the old one would leave the
    /// forgery intact under its former spelling.
    pub const LEGACY_TYPE_NAME_ALIASES: &'static [&'static str] = &[
        "WorkItemDriverTaken",
        "WorkItemDriverReleased",
        "WorkItemGateEscalated",
        "WorkItemWatched",
        "WorkItemUnwatched",
        "CCSettingsChanged",
        "CaptureAppUI",
        "CcThreadSpawned",
        "ClaudeCodeIdled",
        "ClaudeCodePromptSent",
        "ClaudeCodeTextStreamed",
        "ClaudeCodeThoughtStreamed",
        "ClaudeCodeToolCalled",
        "ClaudeCodeToolResult",
        "ClaudeCodeUserMessageSent",
        "ContinueSignal",
        "CredentialPromptRequested",
        "CredentialRequest",
        "EmailConfirmRequest",
        "MemorySearched",
        "PluginInstallRequest",
        "PluginUninstallRequest",
        "PreambleCompleting",
        "PushNotificationRequest",
        "RefreshAppUI",
        "Retrying",
        "SessionRecovered",
        "SessionResumed",
        "TextStreaming",
        "Thinking",
        "UserPromptInjected",
        "WidgetHidden",
        "WidgetRestored",
    ];

    pub fn is_reserved_type_name(name: &str) -> bool {
        Self::RESERVED_TYPE_NAMES.contains(&name) || Self::LEGACY_TYPE_NAME_ALIASES.contains(&name)
    }

    pub const SIDE_QUESTION_EVENT_TYPES: &'static [&'static str] = &[
        "SideQuestionAsked",
        "SideQuestionAnswered",
        "SideQuestionFailed",
        "SideQuestionDismissed",
    ];

    pub const PARENT_NOTE_EVENT_TYPES: &'static [&'static str] = &[
        "ChildThreadStopped",
        "ChildThreadDetached",
        "ChildReportFiled",
    ];

    pub fn is_engine_turn_idle(&self) -> bool {
        matches!(
            self,
            Self::CodingAgentIdled { reason: Some(r), .. }
                if ENGINE_TURN_IDLE_REASONS.contains(&r.as_str())
        )
    }

    pub fn is_side_question_event(&self) -> bool {
        matches!(
            self,
            Self::SideQuestionAsked { .. }
                | Self::SideQuestionAnswered { .. }
                | Self::SideQuestionFailed { .. }
                | Self::SideQuestionDismissed { .. }
        )
    }

    pub fn is_per_token_streaming(&self) -> bool {
        matches!(
            self,
            Self::TextStreamed { .. }
                | Self::ThoughtStreamed { .. }
                | Self::CodingAgentTextStreamed { .. }
                | Self::CodingAgentThoughtStreamed { .. }
        )
    }

    pub fn moves_archive_input_off_summary(&self) -> bool {
        matches!(
            self,
            Self::UserQuestionAsked { .. }
                | Self::UserQuestionAnswered { .. }
                | Self::ChangeProposed { .. }
                | Self::ChangeApplied { .. }
                | Self::ChangeDiscarded { .. }
                | Self::ChangeSetAside { .. }
                | Self::ChangeBroughtBack { .. }
                | Self::ChangeWithdrawn { .. }
                | Self::ChangeReverted { .. }
        )
    }

    pub fn is_persisted(&self) -> bool {
        !matches!(
            self,
            Self::CumulativeTextUpdated { .. }
                | Self::LlmCallRetried { .. }
                | Self::PreambleCompleted
                | Self::PushNotificationRequested
                | Self::AppUiRefreshRequested { .. }
                | Self::AppUiCaptureRequested { .. }
                | Self::WidgetFitRequested { .. }
                | Self::NavigationRequested { .. }
                | Self::CodingAgentThreadSpawned { .. }
                | Self::CodingAgentDiffChanged { .. }
                | Self::ChildrenCountChanged { .. }
        )
    }

    /// INVARIANT: The only `event_type` names [`Self::indexable_text`] returns text for.
    /// It checks this list first, so a memory rebuild that filters on it in
    /// SQL indexes exactly what the live consumer does. A new arm there needs
    /// its name here too, or it never fires.
    pub const INDEXABLE_EVENT_TYPES: &'static [&'static str] = &[
        "MessageReceived",
        "SpokenMessageReceived",
        "UserOnlyMessagePosted",
        "PromptInjected",
        "ResponseGenerated",
        "ResponseCanceled",
        "ResponseAborted",
        "ChildThreadCompleted",
        "ChildReportFiled",
        "ImageDescribed",
    ];

    pub fn indexable_text(&self) -> Option<&str> {
        if !Self::INDEXABLE_EVENT_TYPES.contains(&self.event_type()) {
            return None;
        }
        match self {
            Self::MessageReceived { text, .. } => Some(text),
            Self::SpokenMessageReceived { text, .. } => Some(text),
            Self::UserOnlyMessagePosted { text, .. } => Some(text),
            // INVARIANT: A `PromptInjected` carrying `injected_message_id` is an
            // ACKNOWLEDGEMENT of a `MessageReceived` that is already persisted
            // and already indexed, with the same text copied verbatim. Indexing
            // it again files the user's sentence into memory twice. The engine
            // mode (a resume note, a legacy child-thread callback) carries no
            // such id and is the only original content here, so it stays.
            Self::PromptInjected {
                text,
                injected_message_id: None,
                ..
            } => Some(text),
            Self::PromptInjected { .. } => None,
            Self::ResponseGenerated { text, .. } => Some(text),
            Self::ResponseCanceled { text, .. } => Some(text),
            Self::ResponseAborted { text, .. } => Some(text),
            Self::ChildThreadCompleted { summary, .. } | Self::ChildReportFiled { summary, .. } => {
                Some(summary)
            }
            Self::ImageDescribed { description, .. } => Some(description),
            _ => None,
        }
    }

    pub fn from_stored(
        event_type: &str,
        mut payload: Value,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        payload
            .as_object_mut()
            .ok_or("the stored payload is not an object")?
            .insert("type".into(), Value::String(event_type.into()));
        Ok(serde_json::from_value(payload)?)
    }

    pub fn into_queued(self) -> Self {
        match self {
            Self::UserQuestionAsked {
                tool_use_id,
                cc_session_id,
                question,
                options,
                worktree_path,
                multi_select,
                owner_approval,
                permission_request,
            } => Self::UserQuestionQueued {
                tool_use_id,
                cc_session_id,
                question,
                options,
                worktree_path,
                multi_select,
                owner_approval,
                permission_request,
            },
            other => other,
        }
    }

    pub fn queued_into_asked(self) -> Self {
        match self {
            Self::UserQuestionQueued {
                tool_use_id,
                cc_session_id,
                question,
                options,
                worktree_path,
                multi_select,
                owner_approval,
                permission_request,
            } => Self::UserQuestionAsked {
                tool_use_id,
                cc_session_id,
                question,
                options,
                worktree_path,
                multi_select,
                owner_approval,
                permission_request,
            },
            other => other,
        }
    }

    pub fn to_payload(&self, meta: &EventMeta) -> Value {
        let mut v = serde_json::to_value(self).expect("ThreadEvent serialization cannot fail");
        if let Some(obj) = v.as_object_mut() {
            obj.remove("type");
            meta.apply(obj);
        }
        v
    }
}

#[cfg(test)]
mod renamed_event_tests {
    use super::*;

    #[test]
    fn stored_role_events_parse_under_their_legacy_names() {
        for (suffix, data) in [
            ("DriverTaken", serde_json::json!({})),
            ("DriverReleased", serde_json::json!({})),
            (
                "GateEscalated",
                serde_json::json!({
                    "tool_use_id": "gate",
                    "gate_thread_id": uuid::Uuid::nil(),
                }),
            ),
            ("Watched", serde_json::json!({})),
            ("Unwatched", serde_json::json!({})),
        ] {
            let legacy = format!("WorkItem{suffix}");
            let event = ThreadEvent::from_stored(&legacy, data).unwrap();
            assert_eq!(event.event_type(), format!("RootThread{suffix}"));
            assert!(ThreadEvent::is_reserved_type_name(&legacy));
        }
    }

    #[test]
    fn stored_plan_approval_without_branch_still_parses() {
        let payload = serde_json::json!({
            "kind": "implementation",
            "scope": "implement",
            "plan_path": "docs/plans/plan.md",
            "plan_sha": "plan",
        });
        let event = ThreadEvent::from_stored("PlanApproved", payload).unwrap();
        assert!(matches!(
            event,
            ThreadEvent::PlanApproved {
                branch_name: None,
                ..
            }
        ));
    }
}
