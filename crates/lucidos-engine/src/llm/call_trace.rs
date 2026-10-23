//! The *call trace*: transport facts one metered chat call collects on the way
//! to its answer or its failure.
//!
//! `LlmProvider::chat` is provider-agnostic and its error is a boxed string.
//! Neither can carry an HTTP status or a response header out of the send loop.
//! The trace rides the task instead, like `cache_probe::ProbeCall`. [`scoped`]
//! opens it around one provider call, and the sites that see a fact write it.
//! The metered call reads it back when the call ends, either way.
//!
//! One trace holds one call, retries and a model fallback included. The
//! per-attempt facts (status, `request-id`, first delta) describe the LAST
//! attempt, which answered or failed the call. A write outside [`scoped`] is a
//! no-op.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::llm::ProviderKind;

/// How a failed call failed, from what the trace saw. Never the message: a
/// provider's error body can echo request content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallFailure {
    /// The provider answered with a non-success status.
    Http,
    /// No response arrived: a refused or dropped connection.
    Connection,
    /// No response arrived in time.
    Timeout,
    /// A success status, then a stream that broke or ended early.
    Stream,
    /// Nothing was sent, or the trace saw no more than that.
    Other,
}

/// What one call's attempts did. Read with [`scoped`].
#[derive(Clone, Debug, Default)]
pub(crate) struct CallTrace {
    /// Every HTTP send across the call's retries and model fallback.
    pub(crate) attempts: u32,
    /// The last attempt's response status, when a response arrived.
    pub(crate) status: Option<u16>,
    /// The last attempt's `request-id` header, when it carried one.
    pub(crate) request_id: Option<String>,
    /// When the last attempt's stream delivered its first content.
    pub(crate) first_delta: Option<Instant>,
    /// The backend the router sent the call to.
    pub(crate) provider: Option<ProviderKind>,
    /// The last attempt's failure, when no response arrived.
    transport_failure: Option<CallFailure>,
}

impl CallTrace {
    /// How the call failed. `timed_out` is the caller's own deadline running
    /// out, which no attempt sees.
    pub(crate) fn failure(&self, timed_out: bool) -> CallFailure {
        if timed_out {
            return CallFailure::Timeout;
        }
        if let Some(failure) = self.transport_failure {
            return failure;
        }
        match self.status {
            Some(status) if (200..300).contains(&status) => CallFailure::Stream,
            Some(_) => CallFailure::Http,
            None => CallFailure::Other,
        }
    }
}

type Shared = Arc<Mutex<CallTrace>>;

tokio::task_local! {
    static TRACE: Shared;
}

/// Run `fut` with a fresh trace in scope, and return its output with what the
/// trace collected.
pub(crate) async fn scoped<F: std::future::Future>(fut: F) -> (F::Output, CallTrace) {
    let shared = Shared::default();
    let output = TRACE.scope(shared.clone(), fut).await;
    let trace = shared.lock().unwrap_or_else(|p| p.into_inner()).clone();
    (output, trace)
}

fn write(update: impl FnOnce(&mut CallTrace)) {
    let _ = TRACE.try_with(|shared| update(&mut shared.lock().unwrap_or_else(|p| p.into_inner())));
}

/// An HTTP send begins. It clears the last attempt's facts, a first delta
/// included, so a retry never reports the attempt before it. Each send is
/// followed by at most one stream.
pub(crate) fn attempt_began() {
    write(|trace| {
        trace.attempts += 1;
        trace.status = None;
        trace.request_id = None;
        trace.first_delta = None;
        trace.transport_failure = None;
    });
}

/// The attempt got a response head.
pub(crate) fn response_received(status: u16, request_id: Option<&str>) {
    write(|trace| {
        trace.status = Some(status);
        trace.request_id = request_id.map(str::to_string);
    });
}

/// The attempt got no response.
pub(crate) fn transport_failed(failure: CallFailure) {
    write(|trace| trace.transport_failure = Some(failure));
}

/// The stream delivered content. Only the first one counts.
pub(crate) fn content_arrived() {
    write(|trace| {
        trace.first_delta.get_or_insert_with(Instant::now);
    });
}

/// The router sent the call to `provider`.
pub(crate) fn routed_to(provider: ProviderKind) {
    write(|trace| trace.provider = Some(provider));
}

#[cfg(test)]
#[path = "call_trace_tests.rs"]
mod tests;
