//! Foundation types, event bus, and event projections.

pub mod consumption_claim;
pub mod continuation_reason;
pub mod event_bus;
pub mod form_requests;
pub mod handoff_sql;
pub mod pending_handoff;
pub mod provider_rejection;
pub mod question_park_sql;
pub mod question_parsing;
pub mod session_scope;
pub mod standing_instruction;
pub mod switch_recovery_sql;
pub mod thread_events;
pub mod thread_family;
pub mod thread_lifecycle;
pub mod thread_state;
pub mod trigger_context;
pub mod widget_embed;

#[doc(hidden)]
pub mod event_bus_test_support;
