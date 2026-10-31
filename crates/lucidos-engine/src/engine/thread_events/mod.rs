//! Persisted thread events and their cross-cutting metadata.
//!
//! Re-export barrel: every public item keeps its original
//! `crate::engine::thread_events::*` path. The `ThreadEvent` enum lives in
//! `event.rs` (its inherent `impl` in `event_impl.rs`); supporting types and
//! the emit helpers are grouped by responsibility seam in the sibling modules.

mod actor;
mod agent_exit;
mod apply_now;
mod capture;
mod cause;
mod channel;
mod dashboard;
mod emit;
mod event;
mod event_impl;
mod expected_wake;
mod form_request;
mod keep_alive_status;
mod mcp;
mod memory;
mod meta;
mod question;
mod session;
mod todo;
mod triage;

pub use actor::{
    ActorMode, AgentParticipant, EngineReason, EventWaitOutcome, MessageOrigin, PluginSetupCause,
    PluginSetupOccasion, ThreadDirection, CACHE_KEEP_ALIVE_REASON_KIND,
};
pub use agent_exit::{CodingAgentExit, CodingAgentExitStage};
pub use apply_now::ApplyNowOutcome;
pub use capture::CaptureFormat;
pub use cause::{AbortCause, CancelCause, EventWaitCancelCause, UnproposedReason, UnqueueReason};
pub use channel::{EventChannel, FireRetry, RetryOf, RetrySkipReason, TriggerInvocation};
pub use dashboard::{ArrangedTile, Dashboard, LegacyArrangedTile};
pub use event::ThreadEvent;
pub use event_impl::{
    not_a_keep_alive_idle_sql, KEEP_ALIVE_ENDING_EVENT_TYPES, KEEP_ALIVE_IDLE_REASON,
};
pub use expected_wake::{PlannedWake, WakeBucket, WakeSource};
pub use form_request::FormRequestOutcome;
pub use keep_alive_status::{CacheKeepAliveNotKeptReason, CacheKeepAliveStatus, CapOdds};
pub use mcp::McpEnablementSource;
pub use memory::RecalledMemory;
pub use meta::EventMeta;
pub(crate) use question::with_image_note;
pub use question::{AnswerKind, OwnerApproval, QuestionOption, WidgetEmbed};
pub use session::{
    ChildCompletionStatus, FreshSessionReason, HandoffConsent, HandoffOpening, IntegratedBranch,
    SessionContinuity, SessionEndReason, SubThreadPendingChange, VoiceSessionEndReason, WakeMode,
};
pub use todo::{TodoItem, TodoStatus};
pub use triage::TriageProposalEntry;

pub(crate) use emit::{
    emit_continuation_requested_or_log, emit_response_aborted, emit_response_canceled,
    has_terminator_for,
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
pub(crate) mod ts_codegen;

#[cfg(test)]
#[path = "../thread_events_tests/variants.rs"]
mod variant_tests;

/// A human follow-up makes a trigger thread attended. Both projection and
/// recovery bind `$2` from `ActorMode::Human`, including legacy missing modes.
pub(crate) const HUMAN_MESSAGE_SQL: &str =
    "event_type = 'MessageReceived' AND COALESCE(payload->>'mode', $2) = $2";
