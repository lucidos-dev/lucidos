use serde_json::Value;

use super::{EventMeta, ThreadEvent};

/// The one tool whose args the inline render reads, so the args strip spares it.
const GENERATE_IMAGE_TOOL: &str = "generate_image";

/// The `reason` on a *cache keep-alive* turn's `CodingAgentIdled`.
pub const KEEP_ALIVE_IDLE_REASON: &str = "cache_keep_alive";

/// The events that end a *cache keep-alive* turn: a card the agent raised
/// anyway. The thread now waits on a person, so the card and the turn after
/// it show.
pub const KEEP_ALIVE_ENDING_EVENT_TYPES: &[&str] =
    &["UserQuestionAsked", "CodingAgentPermissionRequest"];

/// SQL true for every `events` row but a keep-alive idle, for each scan that
/// must agree with [`ThreadEvent::is_cache_keep_alive_idle`]. `prefix` names
/// the table alias with its dot, or is empty.
pub fn not_a_keep_alive_idle_sql(prefix: &str) -> String {
    format!(
        "NOT ({prefix}event_type = 'CodingAgentIdled' \
         AND COALESCE({prefix}payload->>'reason', '') = '{KEEP_ALIVE_IDLE_REASON}')"
    )
}

impl ThreadEvent {
    /// Names of the four `Response*` events that close a request. A chat turn
    /// ends on one, and so does a coding-agent turn, which stamps its channel.
    /// Excludes the coding-agent lifecycle events (`CodingAgentIdled`,
    /// `SessionEnded`), which `chat::recovery::recover_orphaned_threads`
    /// enumerates itself. A caller that wants only one agent's turns filters
    /// on the channel, as `agent_session::turn_gap` does.
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

    /// Drop a tool result's text from its serialized payload and mark it, so a
    /// reader fetches it by event id. The snapshot and the live stream both
    /// call this, which keeps a stripped result one shape everywhere.
    pub fn strip_result_text(payload: &mut serde_json::Map<String, Value>) {
        payload.remove("result");
        payload.insert("result_stripped".to_string(), Value::Bool(true));
    }

    /// Drop a tool call's `args` from its serialized payload and mark it, so the
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
        // The generated image takes its alt text from the call's prompt, so
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

    /// Returns the variant name as a string, matching the DB `event_type` column.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::MessageReceived { .. } => "MessageReceived",
            Self::QueuedMessageRemoved { .. } => "QueuedMessageRemoved",
            Self::MessageHeld { .. } => "MessageHeld",
            Self::HeldMessageReleased { .. } => "HeldMessageReleased",
            Self::TextStreamed { .. } => "TextStreamed",
            Self::ThoughtStreamed { .. } => "ThoughtStreamed",
            Self::ContextCaptured { .. } => "ContextCaptured",
            Self::ModelCallFailed { .. } => "ModelCallFailed",
            Self::MemoryRecalled { .. } => "MemoryRecalled",
            Self::ToolCalled { .. } => "ToolCalled",
            Self::ToolResult { .. } => "ToolResult",
            Self::TodoListWritten { .. } => "TodoListWritten",
            Self::WorkingUnderstandingWritten { .. } => "WorkingUnderstandingWritten",
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
            Self::CodingAgentInputRead { .. } => "CodingAgentInputRead",
            Self::CodingAgentInputDiscarded { .. } => "CodingAgentInputDiscarded",
            Self::MissingHardeningDetected { .. } => "MissingHardeningDetected",
            Self::CodingAgentIdled { .. } => "CodingAgentIdled",
            Self::ContinuationRequested { .. } => "ContinuationRequested",
            Self::ThreadTitleGenerated { .. } => "ThreadTitleGenerated",
            Self::ThreadTitleRenamed { .. } => "ThreadTitleRenamed",
            Self::ThreadSaved => "ThreadSaved",
            Self::ThreadUnsaved => "ThreadUnsaved",
            Self::ThreadArchived => "ThreadArchived",
            Self::ThreadArchiveRequested => "ThreadArchiveRequested",
            Self::ThreadUnarchived => "ThreadUnarchived",
            Self::ThreadReadRequested => "ThreadReadRequested",
            Self::ThreadReadNotRequested => "ThreadReadNotRequested",
            Self::ParentWakeRequested => "ParentWakeRequested",
            Self::ParentWakeNotRequested => "ParentWakeNotRequested",
            Self::ThreadReplySeen { .. } => "ThreadReplySeen",
            Self::ThreadTriageProposed { .. } => "ThreadTriageProposed",
            Self::ThreadStarted { .. } => "ThreadStarted",
            Self::ThreadDiscarded { .. } => "ThreadDiscarded",
            Self::HomeThreadCreated => "HomeThreadCreated",
            Self::ImageUploaded { .. } => "ImageUploaded",
            Self::TriggerStarted { .. } => "TriggerStarted",
            Self::TriggerCompleted { .. } => "TriggerCompleted",
            Self::ChangeProposed { .. } => "ChangeProposed",
            Self::ChangeApplied { .. } => "ChangeApplied",
            Self::ChangeDiscarded { .. } => "ChangeDiscarded",
            Self::ChangeReverted { .. } => "ChangeReverted",
            Self::ChangeSetAside { .. } => "ChangeSetAside",
            Self::ChangeBroughtBack { .. } => "ChangeBroughtBack",
            Self::ChangeWithdrawn { .. } => "ChangeWithdrawn",
            Self::ProposalWithheld { .. } => "ProposalWithheld",
            Self::ChangeQueued { .. } => "ChangeQueued",
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
            Self::CodingAgentCompacted { .. } => "CodingAgentCompacted",
            Self::SuccessorThreadSpawned { .. } => "SuccessorThreadSpawned",
            Self::HandoffCheckpointReached { .. } => "HandoffCheckpointReached",
            Self::SuccessorOfferDeclined { .. } => "SuccessorOfferDeclined",
            Self::SuccessorRequested { .. } => "SuccessorRequested",
            Self::PromptInjected { .. } => "PromptInjected",
            Self::CredentialRequested { .. } => "CredentialRequested",
            Self::PluginInstallRequested { .. } => "PluginInstallRequested",
            Self::PluginUninstallRequested { .. } => "PluginUninstallRequested",
            Self::EmailConfirmRequested { .. } => "EmailConfirmRequested",
            Self::OAuthAuthorizationRequested { .. } => "OAuthAuthorizationRequested",
            Self::FormRequestResolved { .. } => "FormRequestResolved",
            Self::McpConsentRequested { .. } => "McpConsentRequested",
            Self::UserQuestionAsked { .. } => "UserQuestionAsked",
            Self::UserQuestionAnswered { .. } => "UserQuestionAnswered",
            Self::OwnerApprovalRequested { .. } => "OwnerApprovalRequested",
            Self::OwnerApprovalSpent { .. } => "OwnerApprovalSpent",
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
            Self::EventWaitDelivered { .. } => "EventWaitDelivered",
            Self::EventWaitExpired { .. } => "EventWaitExpired",
            Self::EventWaitCanceled { .. } => "EventWaitCanceled",
            Self::CacheKeepAliveStatusChanged { .. } => "CacheKeepAliveStatusChanged",
            Self::CacheKeepAliveTurnedOff => "CacheKeepAliveTurnedOff",
            Self::CacheKeepAliveTurnedOn => "CacheKeepAliveTurnedOn",
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
            Self::ThreadMcpServerEnabled { .. } => "ThreadMcpServerEnabled",
            Self::ThreadMcpServerDisabled { .. } => "ThreadMcpServerDisabled",
            Self::HomeBoardArranged { .. } => "HomeBoardArranged",
            // Transient
            Self::CumulativeTextUpdated { .. } => "CumulativeTextUpdated",
            Self::LlmCallRetried { .. } => "LlmCallRetried",
            Self::PreambleCompleted => "PreambleCompleted",
            Self::PushNotificationRequested => "PushNotificationRequested",
            Self::AppUiRefreshRequested { .. } => "AppUiRefreshRequested",
            Self::AppUiCaptureRequested { .. } => "AppUiCaptureRequested",
            Self::NavigationRequested { .. } => "NavigationRequested",
            Self::CodingAgentThreadSpawned { .. } => "CodingAgentThreadSpawned",
            Self::CodingAgentDiffChanged { .. } => "CodingAgentDiffChanged",
            Self::ChildrenCountChanged { .. } => "ChildrenCountChanged",
        }
    }

    /// Every wire `type` name this enum can produce.
    ///
    /// `validate_emittable_event_type` refuses all of them, so an app UI cannot
    /// write a *domain* event under a thread-event name. Such a row is
    /// permanent, and its `aggregate_id` is the event-type STRING rather than a
    /// thread uuid. One `emit_event("EventWaitStarted", ...)` therefore breaks
    /// every later query that casts `aggregate_id::uuid` on that name.
    ///
    /// The transient variants are here too. They are never persisted as thread
    /// rows, but the deny list is about the NAME. A domain row carrying one
    /// poisons a future query just as well.
    ///
    /// `reserved_type_names_cover_every_variant` recovers the real variant list
    /// from serde and fails if this one has drifted.
    pub const RESERVED_TYPE_NAMES: &'static [&'static str] = &[
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
        "BackgroundBashStarted",
        "BackgroundBashCompleted",
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
        "MissingHardeningDetected",
        "CodingAgentIdled",
        "ContinuationRequested",
        "ThreadTitleGenerated",
        "ThreadTitleRenamed",
        "ThreadSaved",
        "ThreadUnsaved",
        "ThreadArchived",
        "ThreadArchiveRequested",
        "ThreadUnarchived",
        "ThreadReadRequested",
        "ThreadReadNotRequested",
        "ParentWakeRequested",
        "ParentWakeNotRequested",
        "ThreadReplySeen",
        "ThreadTriageProposed",
        "ThreadStarted",
        "ThreadDiscarded",
        "HomeThreadCreated",
        "ImageUploaded",
        "TriggerStarted",
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
        "CodingAgentCompacted",
        "SuccessorThreadSpawned",
        "HandoffCheckpointReached",
        "SuccessorOfferDeclined",
        "SuccessorRequested",
        "PromptInjected",
        "CredentialRequested",
        "PluginInstallRequested",
        "PluginUninstallRequested",
        "EmailConfirmRequested",
        "OAuthAuthorizationRequested",
        "FormRequestResolved",
        "McpConsentRequested",
        "UserQuestionAsked",
        "UserQuestionAnswered",
        "OwnerApprovalRequested",
        "OwnerApprovalSpent",
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
        "EventWaitDelivered",
        "EventWaitExpired",
        "EventWaitCanceled",
        "CacheKeepAliveStatusChanged",
        "CacheKeepAliveTurnedOff",
        "CacheKeepAliveTurnedOn",
        "CumulativeTextUpdated",
        "LlmCallRetried",
        "PreambleCompleted",
        "PushNotificationRequested",
        "AppUiRefreshRequested",
        "AppUiCaptureRequested",
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
        "ThreadMcpServerEnabled",
        "ThreadMcpServerDisabled",
        "HomeBoardArranged",
    ];

    /// The `#[serde(alias = ...)]` spellings, kept for rows written before a
    /// rename.
    ///
    /// Denied at the emit boundary alongside the current names. An alias
    /// deserializes INTO its variant, so a domain row named `Thinking` reads
    /// back as a `ThoughtStreamed` thread event to every consumer that parses
    /// one. Refusing the new name and allowing the old one would leave the
    /// forgery intact under its former spelling.
    pub const LEGACY_TYPE_NAME_ALIASES: &'static [&'static str] = &[
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

    /// Whether `name` is a `ThreadEvent` wire name, current or legacy.
    pub fn is_reserved_type_name(name: &str) -> bool {
        Self::RESERVED_TYPE_NAMES.contains(&name) || Self::LEGACY_TYPE_NAME_ALIASES.contains(&name)
    }

    /// The `event_type` names of the four side-question events, for readers
    /// that filter in SQL. `is_side_question_event` is the typed twin.
    ///
    /// No agent may see these rows, and none may count as thread activity.
    /// Every reader that walks a thread's events generically excludes them,
    /// each pinned by a test that feeds it these rows.
    pub const SIDE_QUESTION_EVENT_TYPES: &'static [&'static str] = &[
        "SideQuestionAsked",
        "SideQuestionAnswered",
        "SideQuestionFailed",
        "SideQuestionDismissed",
    ];

    /// The notes a fan-in leaves on a parent that wake nothing. A note after
    /// an unprocessed `ChildThreadCompleted` is no reaction to it, so readers
    /// asking "is that card the parent's last word" skip them.
    pub const PARENT_NOTE_EVENT_TYPES: &'static [&'static str] = &[
        "ChildThreadStopped",
        "ChildThreadDetached",
        "ChildReportFiled",
    ];

    /// Whether this is the idle a *cache keep-alive* turn ends with. No agent,
    /// trigger or event wait may read it as the agent finishing.
    pub fn is_cache_keep_alive_idle(&self) -> bool {
        matches!(
            self,
            Self::CodingAgentIdled { reason: Some(r), .. } if r == KEEP_ALIVE_IDLE_REASON
        )
    }

    /// Whether this is one of the four side-question events, which no agent,
    /// trigger or event wait may ever see.
    pub fn is_side_question_event(&self) -> bool {
        matches!(
            self,
            Self::SideQuestionAsked { .. }
                | Self::SideQuestionAnswered { .. }
                | Self::SideQuestionFailed { .. }
                | Self::SideQuestionDismissed { .. }
        )
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
                | Self::PushNotificationRequested
                | Self::AppUiRefreshRequested { .. }
                | Self::AppUiCaptureRequested { .. }
                | Self::NavigationRequested { .. }
                | Self::CodingAgentThreadSpawned { .. }
                | Self::CodingAgentDiffChanged { .. }
                | Self::ChildrenCountChanged { .. }
        )
    }

    /// The only `event_type` names [`Self::indexable_text`] returns text for.
    /// It checks this list first, so a memory rebuild that filters on it in
    /// SQL indexes exactly what the live consumer does. A new arm there needs
    /// its name here too, or it never fires.
    pub const INDEXABLE_EVENT_TYPES: &'static [&'static str] = &[
        "MessageReceived",
        "SpokenMessageReceived",
        "PromptInjected",
        "ResponseGenerated",
        "ResponseCanceled",
        "ResponseAborted",
        "ChildThreadCompleted",
        "ChildReportFiled",
        "ImageDescribed",
    ];

    /// Returns the text content to index into memory, if this event type is indexable.
    /// Used by both the live memory consumer and the rebuild path.
    pub fn indexable_text(&self) -> Option<&str> {
        if !Self::INDEXABLE_EVENT_TYPES.contains(&self.event_type()) {
            return None;
        }
        match self {
            Self::MessageReceived { text, .. } => Some(text),
            // What the caller said on a CALL, which is the same fact in the
            // other input mode. A delegated utterance was indexed through the
            // `MessageReceived` beside it until ADR 0201 stopped writing one.
            // Without this arm nothing spoken reaches memory at all.
            Self::SpokenMessageReceived { text, .. } => Some(text),
            // A `PromptInjected` carrying `injected_message_id` is an
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
            // Image descriptions carry real shared content (screenshots,
            // tickets, photos). Index them so an image-only turn isn't a
            // memory black hole — the title path already folds them in.
            Self::ImageDescribed { description, .. } => Some(description),
            _ => None,
        }
    }

    /// A stored row back as its typed event: the inverse of [`Self::to_payload`].
    /// The row keeps its type in the `event_type` column, not the payload.
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
