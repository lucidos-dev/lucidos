use serde_json::Value;

use crate::runtime::CodingAgent;

use super::{EventMeta, ThreadEvent};

impl ThreadEvent {
    /// Names of every event variant that closes a chat-mode request.
    /// Excludes CC-specific terminators (`CodingAgentIdled`, `SessionEnded`)
    /// — `chat::recovery::recover_orphaned_threads` checks both sets and
    /// keeps its own enumeration. Use this for code that operates on the
    /// chat agentic loop only.
    pub const TERMINATOR_EVENT_TYPES: &'static [&'static str] = &[
        "ResponseGenerated",
        "ResponseCanceled",
        "ResponseAborted",
        "ResponseFailed",
    ];

    /// Event names that mean a previously-emitted `UserQuestionAsked` is no
    /// longer the latest interactive point on the thread — either the
    /// surrounding turn ended (terminal), or the agent kept emitting
    /// progression events past it (CC's parallel-tool-call race: the model
    /// emitted `AskUserQuestion` alongside sibling tool_uses in one assistant
    /// message; the hook blocked the question but the siblings dispatched).
    /// Once any of these lands after a question, the next typed user text
    /// must start a fresh follow-up rather than be routed as a `FreeText`
    /// answer to the dead question. Two consumers:
    ///
    /// * `agent_question::lookup_active_question_tool_use_id` (the CC chat
    ///   FreeText fast-path in `chat::process.rs`). `lookup_pending_question_…`
    ///   is intentionally broader and ignores this list, so archive's
    ///   cancel-stamp still works on overtaken questions.
    /// * `agent_recovery::unanswered_question_exists_sql`, the restart preserve
    ///   guard, which unions this list with three extras of its own. So a name
    ///   added or removed here also moves the boundary between "this thread is
    ///   a preserved checkpoint across a restart" and "this is an ordinary
    ///   interrupted turn"; check that side too.
    ///
    /// The frontend mirrors the list a third time as
    /// `QUESTION_OVERTAKEN_STEP_TYPES` (`store/thread-events/exchange-grouping.ts`),
    /// which decides both whether the card renders struck through and whether
    /// the exchange can still read "Needs your answer".
    ///
    /// `ResponseGenerated` is omitted because `UserQuestionAsked` is currently
    /// CC-only on the production path; CC turns end with `CodingAgentIdled`,
    /// not `ResponseGenerated`. That reasoning is scoped to the consumers
    /// above; the preserve guard cannot afford the assumption, which is why it
    /// adds `ResponseGenerated` / `SessionEnded` / `UserQuestionAnswered` in its
    /// own `PARK_ENDING_EXTRA_EVENT_TYPES` rather than widening this list. The
    /// chat-agent variants below are included for symmetry: the agentic loop
    /// blocks sequentially on `ask_user_question` today, but the uniform list
    /// defends against future regressions.
    ///
    /// `UserQuestionAsked` is NOT in the set: the SQL's
    /// `ORDER BY sequence DESC LIMIT 1` already picks the latest unanswered
    /// question, so a replacement naturally takes over without an explicit
    /// orphaning entry.
    pub const QUESTION_OVERTAKEN_EVENT_TYPES: &'static [&'static str] = &[
        // Terminal (both agents)
        "ResponseAborted",
        "ResponseCanceled",
        "ResponseFailed",
        "CodingAgentIdled",
        // CC progression — the parallel-tool-call race
        "CodingAgentTextStreamed",
        "CodingAgentToolCalled",
        "CodingAgentToolResult",
        "CodingAgentPromptSent",
        // Chat-agent progression (symmetry; harmless on CC threads)
        "TextStreamed",
        "ThoughtStreamed",
        "ToolCalled",
        "ToolResult",
    ];

    /// Convert a control request into a CodingAgentSettingsChanged event,
    /// if applicable. `coding_agent` identifies which backend issued the change.
    pub fn from_control_request(
        request: &crate::runtime::ControlRequest,
        coding_agent: CodingAgent,
    ) -> Option<Self> {
        use crate::runtime::ControlRequest;
        let (model, effort, perm) = match request {
            ControlRequest::SetModel { model } => (Some(model.clone()), None, None),
            ControlRequest::SetReasoningEffort { effort } => (None, Some(effort.clone()), None),
            ControlRequest::SetPermissionMode { mode } => (None, None, Some(mode.clone())),
            _ => return None,
        };
        Some(Self::CodingAgentSettingsChanged {
            model,
            reasoning_effort: effort,
            permission_mode: perm,
            coding_agent,
            // Settings-only emit (user changed model/effort/permission mid-session).
            // The session id AND config dir are pinned by the Init-time emit; see
            // the variant doc.
            cc_session_id: None,
            claude_config_dir: None,
        })
    }

    /// Returns the variant name as a string, matching the DB `event_type` column.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::MessageReceived { .. } => "MessageReceived",
            Self::QueuedMessageRemoved { .. } => "QueuedMessageRemoved",
            Self::TextStreamed { .. } => "TextStreamed",
            Self::ThoughtStreamed { .. } => "ThoughtStreamed",
            Self::ContextCaptured { .. } => "ContextCaptured",
            Self::MemorySearched { .. } => "MemorySearched",
            Self::ToolCalled { .. } => "ToolCalled",
            Self::ToolResult { .. } => "ToolResult",
            Self::TodoListWritten { .. } => "TodoListWritten",
            Self::BackgroundBashStarted { .. } => "BackgroundBashStarted",
            Self::BackgroundBashCompleted { .. } => "BackgroundBashCompleted",
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
            Self::MissingHardeningDetected { .. } => "MissingHardeningDetected",
            Self::CodingAgentIdled { .. } => "CodingAgentIdled",
            Self::ContinuationRequested { .. } => "ContinuationRequested",
            Self::ThreadTitleGenerated { .. } => "ThreadTitleGenerated",
            Self::ThreadTitleRenamed { .. } => "ThreadTitleRenamed",
            Self::ThreadSaved => "ThreadSaved",
            Self::ThreadUnsaved => "ThreadUnsaved",
            Self::ThreadArchived => "ThreadArchived",
            Self::ThreadStarted { .. } => "ThreadStarted",
            Self::ThreadDiscarded { .. } => "ThreadDiscarded",
            Self::ImageUploaded { .. } => "ImageUploaded",
            Self::TriggerStarted { .. } => "TriggerStarted",
            Self::TriggerCompleted { .. } => "TriggerCompleted",
            Self::ChangeProposed { .. } => "ChangeProposed",
            Self::ChangeApplied { .. } => "ChangeApplied",
            Self::ChangeDiscarded { .. } => "ChangeDiscarded",
            Self::ChangeReverted { .. } => "ChangeReverted",
            Self::ChangeApplyFailed { .. } => "ChangeApplyFailed",
            Self::MergeConflictDetected { .. } => "MergeConflictDetected",
            Self::MergeResolutionStarted { .. } => "MergeResolutionStarted",
            Self::MergeResolutionCleared { .. } => "MergeResolutionCleared",
            Self::ChangeHardened { .. } => "ChangeHardened",
            Self::CodingAgentSettingsChanged { .. } => "CodingAgentSettingsChanged",
            Self::UserPromptInjected { .. } => "UserPromptInjected",
            Self::CredentialRequested { .. } => "CredentialRequested",
            Self::McpConsentRequested { .. } => "McpConsentRequested",
            Self::UserQuestionAsked { .. } => "UserQuestionAsked",
            Self::UserQuestionAnswered { .. } => "UserQuestionAnswered",
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
            Self::ContextDismissed { .. } => "ContextDismissed",
            Self::ImageDescribed { .. } => "ImageDescribed",
            Self::EventWaitStarted { .. } => "EventWaitStarted",
            Self::EventWaitDelivered { .. } => "EventWaitDelivered",
            Self::EventWaitExpired { .. } => "EventWaitExpired",
            Self::EventWaitCanceled { .. } => "EventWaitCanceled",
            // Transient
            Self::CumulativeTextUpdated { .. } => "CumulativeTextUpdated",
            Self::LlmCallRetried { .. } => "LlmCallRetried",
            Self::PreambleCompleted => "PreambleCompleted",
            Self::CredentialPromptRequested { .. } => "CredentialPromptRequested",
            Self::PluginInstallRequested { .. } => "PluginInstallRequested",
            Self::PluginUninstallRequested { .. } => "PluginUninstallRequested",
            Self::EmailConfirmRequested { .. } => "EmailConfirmRequested",
            Self::PushNotificationRequested => "PushNotificationRequested",
            Self::AppUiRefreshRequested { .. } => "AppUiRefreshRequested",
            Self::AppUiCaptureRequested { .. } => "AppUiCaptureRequested",
            Self::NavigationRequested { .. } => "NavigationRequested",
            Self::CodingAgentThreadSpawned { .. } => "CodingAgentThreadSpawned",
            Self::CodingAgentDiffChanged { .. } => "CodingAgentDiffChanged",
            Self::ChildrenCountChanged { .. } => "ChildrenCountChanged",
        }
    }

    /// Whether this variant fires once per streamed text chunk (many fires per
    /// turn). Used by the scheduler's trigger gate to short-circuit the
    /// matcher for the per-token firehose — see
    /// `crates/lucidos-engine/src/scheduler/mod.rs`. Adding a new per-token
    /// streaming variant to the enum? Add it here too.
    pub fn is_per_token_streaming(&self) -> bool {
        matches!(
            self,
            Self::TextStreamed { .. }
                | Self::ThoughtStreamed { .. }
                | Self::CodingAgentTextStreamed { .. }
                | Self::CodingAgentThoughtStreamed { .. }
        )
    }

    /// Whether this event should be persisted to the DB.
    /// All variants are past-tense (events-only model); persistence is
    /// orthogonal to tense. Transient variants live on SSE only.
    pub fn is_persisted(&self) -> bool {
        !matches!(
            self,
            Self::CumulativeTextUpdated { .. }
                | Self::LlmCallRetried { .. }
                | Self::PreambleCompleted
                | Self::CredentialPromptRequested { .. }
                | Self::PluginInstallRequested { .. }
                | Self::PluginUninstallRequested { .. }
                | Self::EmailConfirmRequested { .. }
                | Self::PushNotificationRequested
                | Self::AppUiRefreshRequested { .. }
                | Self::AppUiCaptureRequested { .. }
                | Self::NavigationRequested { .. }
                | Self::CodingAgentThreadSpawned { .. }
                | Self::CodingAgentDiffChanged { .. }
                | Self::ChildrenCountChanged { .. }
        )
    }

    /// Returns the text content to index into memory, if this event type is indexable.
    /// Used by both the live memory consumer and the rebuild path.
    pub fn indexable_text(&self) -> Option<&str> {
        match self {
            Self::MessageReceived { text, .. } => Some(text),
            // A `UserPromptInjected` carrying `injected_message_id` is an
            // ACKNOWLEDGEMENT of a `MessageReceived` that is already persisted
            // and already indexed, with the same text copied verbatim. Indexing
            // it again files the user's sentence into memory twice. The engine
            // mode (a resume note, a legacy child-thread callback) carries no
            // such id and is the only original content here, so it stays.
            Self::UserPromptInjected {
                text,
                injected_message_id: None,
                ..
            } => Some(text),
            Self::UserPromptInjected { .. } => None,
            Self::ResponseGenerated { text, .. } => Some(text),
            Self::ResponseCanceled { text, .. } => Some(text),
            Self::ResponseAborted { text, .. } => Some(text),
            Self::ChildThreadCompleted { summary, .. } => Some(summary),
            // Image descriptions carry real shared content (screenshots,
            // tickets, photos). Index them so an image-only turn isn't a
            // memory black hole — the title path already folds them in.
            Self::ImageDescribed { description, .. } => Some(description),
            _ => None,
        }
    }

    /// Serializes to JSON payload for DB storage, stripping the "type" tag.
    pub fn to_payload(&self, meta: &EventMeta) -> Value {
        let mut v = serde_json::to_value(self).expect("ThreadEvent serialization cannot fail");
        if let Some(obj) = v.as_object_mut() {
            obj.remove("type");
            meta.apply(obj);
        }
        v
    }
}
