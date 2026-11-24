//! Persisted thread events and their cross-cutting metadata.
//!
//! Re-export barrel: every public item keeps its original
//! `crate::engine::thread_events::*` path. The `ThreadEvent` enum lives in
//! `event.rs` (its inherent `impl` in `event_impl.rs`); supporting types and
//! the emit helpers are grouped by responsibility seam in the sibling modules.

mod actor;
pub mod affected_trigger;
mod agent_exit;
pub mod apply;
mod apply_now;
pub mod auto_compact;
pub mod aux_capture;
pub mod billing_provider;
mod capture;
mod cause;
pub mod change_hold;
mod channel;
pub mod checkpoint;
mod coding_agent_kind;
mod context_capture;
mod context_ceiling;
mod dashboard;
mod emit;
mod event;
mod event_impl;
mod id_argument;
pub mod input_order;
pub mod landing_lane;
pub mod liveness;
pub mod main_check;
pub mod tree_backfill;
pub use id_argument::resolve_thread_id_arg;
mod expected_wake;
mod form_request;
mod handoff_request;
pub mod keep_alive_status;
mod mcp;
mod memory;
mod meta;
pub mod notification;
pub mod permission;
mod plan;
mod provider_error;
mod question;
pub mod queue;
mod session;
mod todo;
mod tool_grant;
mod triage;
mod turn_end_facts;
mod turn_origin;

/// This event ages loaded knowhow when a turn finishes.
pub const TURN_FINISHED_EVENT: &str = "ResponseGenerated";

pub use crate::core::coding_agent::CodingAgent;
pub use coding_agent_kind::CodingAgentKind;
pub(crate) use context_capture::AUXILIARY_CAPTURE_SQL;
pub use context_capture::{ContextProducer, ContextPurpose, ContextRole, ContextSection};
pub use context_ceiling::{
    CeilingCrossing, CeilingSource, ContextCeiling, SessionReading, HANDOFF_FLOOR_TOKENS,
};
pub use handoff_request::HandoffRequest;
pub use tool_grant::{AllowScope, GrantOffer, OfferWithheld, UNPROVEN_WORKSPACE_DESTRUCTION_NOTE};
pub use {turn_origin::CC_ORIGINATING_EVENT_TYPES, turn_origin::CHAT_ORIGINATING_EVENT_TYPES};

pub use actor::{
    ActorMode, AgentParticipant, EngineReason, EventWaitOutcome, MessageOrigin, PluginSetupCause,
    PluginSetupOccasion, RemoteStartAsk, SpawnVerdict, ThreadDirection,
    CACHE_KEEP_ALIVE_REASON_KIND, EVENT_WAIT_REASON_KIND,
};
pub use agent_exit::{CodingAgentExit, CodingAgentExitStage};
pub use apply_now::ApplyNowOutcome;
pub use capture::CaptureFormat;
pub use cause::{
    AbortCause, AgentPauseCause, AutomationUnpauseCause, CancelCause, EventWaitCancelCause,
    HoldLiftCause, UnproposedReason, UnqueueReason,
};
pub use channel::{EventChannel, FireRetry, RetryOf, RetrySkipReason, TriggerInvocation};
pub use dashboard::{ArrangedTile, Dashboard, LegacyArrangedTile, WidgetParams};
pub use event::{AgentDraft, BackgroundWaitTiming, RecoveredCompose, ThreadEvent};
pub use event_impl::{
    not_an_engine_turn_idle_sql, CEILING_REENTRY_IDLE_REASON, KEEP_ALIVE_ENDING_EVENT_TYPES,
    KEEP_ALIVE_IDLE_REASON,
};
pub use expected_wake::{PlannedWake, WakeBucket, WakeSource};
pub use form_request::FormRequestOutcome;
pub use keep_alive_status::{
    CacheKeepAliveNotKeptReason, CacheKeepAliveStatus, CapOdds, CapSource,
};
pub use mcp::{McpEnablementSource, McpTool};
pub use memory::{
    CorrectedMemory, EmbeddingModelLoadState, EmbeddingModelStatus, MemoryEntry, MemorySource,
    RecalledMemory,
};
pub use meta::EventMeta;
pub use plan::{ChangeContent, PlanApprovalKind, PlanKind, PlanScope};
pub use provider_error::{ProviderError, ProviderErrorCode};
pub use question::composer_submission;
pub use question::with_image_note;
pub use question::{AnswerKind, OwnerApproval, QuestionOption, WidgetEmbed};
pub use session::{
    ChildCompletionStatus, FreshSessionReason, HandoffCause, HandoffConsent, HandoffOpening,
    IntegratedBranch, SandboxMode, SessionContinuity, SessionEndReason, SessionStart,
    SubThreadPendingChange, VoiceSessionEndReason, WakeMode,
};
pub use todo::{TodoItem, TodoStatus};
pub use triage::TriageProposalEntry;
pub use turn_end_facts::{TurnEndFacts, TurnEndStatus};
pub use {actor::agent_thread_actor, actor::agent_tool_actor, actor::device_actor};

pub use {
    emit::emit_change_apply_requested, emit::emit_continuation_requested_or_log,
    emit::emit_response_aborted, emit::emit_response_canceled, emit::has_terminator_for,
};

#[cfg(test)]
#[path = "../thread_events_tests/cause_session.rs"]
mod cause_session_tests;
#[cfg(test)]
#[path = "../thread_events_tests/event_meta.rs"]
mod event_meta_tests;
#[cfg(test)]
#[path = "../thread_events_tests/event_payload.rs"]
mod event_payload_tests;
#[cfg(test)]
#[path = "../thread_events_tests/event_type.rs"]
mod event_type_tests;
#[cfg(test)]
#[path = "../thread_events_tests/message_origin.rs"]
mod message_origin_tests;
#[cfg(test)]
#[path = "../thread_events_tests/origin_fields.rs"]
mod origin_fields_tests;
#[cfg(test)]
#[path = "../thread_events_tests/question.rs"]
mod question_tests;
#[cfg(test)]
#[path = "../thread_events_tests/todo.rs"]
mod todo_tests;

#[cfg(test)]
#[path = "../thread_events_tests/variants.rs"]
mod variant_tests;

/// A human follow-up makes a trigger thread attended. Both projection and
/// recovery bind `$2` from `ActorMode::Human`, including legacy missing modes.
pub const HUMAN_MESSAGE_SQL: &str =
    "event_type = 'MessageReceived' AND COALESCE(payload->>'mode', $2) = $2";

/// What the home thread is called when it is created. The user may rename it,
/// and nothing else does.
pub const HOME_THREAD_TITLE: &str = "Home";

#[cfg(any(test, feature = "test-support"))]
pub use context_capture::ALL_PURPOSES;
