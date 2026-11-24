//! *Gateway usage telemetry*: one OTLP log record per model call the Claude
//! gateway served, sent where the gateway's managed settings ask.
//!
//! A call that answered is Claude Code's `claude_code.api_request` event:
//! model, tokens, cost and timing. A call that failed is its
//! `claude_code.api_error` event: model, error class, status and timing. Neither
//! carries content.
//!
//! - [`switch`] decides whether to send, and where.
//! - [`record`] builds a record from a persisted call.
//! - [`otlp`] encodes a batch.
//! - [`exporter`] reads the calls and posts them.
//!
//! The exporter is a consumer of persisted `ContextCaptured` and
//! `ModelCallFailed` rows. Nothing on the request path calls it, so a slow or
//! failing collector never touches a turn. Design: ADR 0510.

pub mod exporter;
pub(crate) mod otlp;
pub(crate) mod record;
pub mod switch;
