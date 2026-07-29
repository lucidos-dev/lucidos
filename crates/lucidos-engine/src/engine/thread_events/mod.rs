//! Persisted thread events and their cross-cutting metadata.
//!
//! Re-export barrel: every public item keeps its original
//! `crate::engine::thread_events::*` path. The `ThreadEvent` enum lives in
//! `event.rs` (its inherent `impl` in `event_impl.rs`); supporting types and
//! the emit helpers are grouped by responsibility seam in the sibling modules.

mod actor;
mod cause;
mod channel;
mod emit;
mod event;
mod event_impl;
mod meta;
mod question;
mod session;
mod todo;

pub use actor::{ActorMode, EngineReason, MessageOrigin, ThreadDirection};
pub use cause::{AbortCause, CancelCause};
pub use channel::{EventChannel, TriggerInvocation};
pub use event::ThreadEvent;
pub use meta::EventMeta;
pub use question::{AnswerKind, QuestionOption};
pub use session::{ChildCompletionStatus, SessionEndReason};
pub use todo::{TodoItem, TodoStatus};

pub(crate) use emit::{
    emit_continuation_requested_or_log, emit_response_aborted, emit_response_canceled,
    has_terminator_for,
};

#[cfg(test)]
#[path = "../thread_events_tests/message_origin.rs"]
mod message_origin_tests;
#[cfg(test)]
#[path = "../thread_events_tests/event_type.rs"]
mod event_type_tests;
#[cfg(test)]
#[path = "../thread_events_tests/event_payload.rs"]
mod event_payload_tests;
#[cfg(test)]
#[path = "../thread_events_tests/event_meta.rs"]
mod event_meta_tests;
#[cfg(test)]
#[path = "../thread_events_tests/question.rs"]
mod question_tests;
#[cfg(test)]
#[path = "../thread_events_tests/cause_session.rs"]
mod cause_session_tests;
#[cfg(test)]
#[path = "../thread_events_tests/origin_fields.rs"]
mod origin_fields_tests;
#[cfg(test)]
#[path = "../thread_events_tests/variants.rs"]
mod variant_tests;
#[cfg(test)]
#[path = "../thread_events_tests/todo.rs"]
mod todo_tests;
