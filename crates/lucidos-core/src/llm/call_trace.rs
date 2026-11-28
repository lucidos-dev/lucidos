use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::llm::ProviderKind;

/// INVARIANT: How a failed call failed, from what the trace saw. Never the message: a
/// provider's error body can echo request content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallFailure {
    Http,
    Connection,
    Timeout,
    Stream,
    Other,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CallTrace {
    pub(crate) attempts: u32,
    pub(crate) status: Option<u16>,
    pub(crate) request_id: Option<String>,
    pub(crate) first_delta: Option<Instant>,
    pub(crate) provider: Option<ProviderKind>,
    transport_failure: Option<CallFailure>,
}

impl CallTrace {
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

pub(crate) async fn scoped<F: std::future::Future>(fut: F) -> (F::Output, CallTrace) {
    let shared = Shared::default();
    let output = TRACE.scope(shared.clone(), fut).await;
    let trace = shared.lock().unwrap_or_else(|p| p.into_inner()).clone();
    (output, trace)
}

fn write(update: impl FnOnce(&mut CallTrace)) {
    let _ = TRACE.try_with(|shared| update(&mut shared.lock().unwrap_or_else(|p| p.into_inner())));
}

pub(crate) fn backend_began() {
    write(|trace| {
        *trace = CallTrace {
            attempts: trace.attempts,
            ..Default::default()
        };
    });
}

pub fn attempt_began() {
    write(|trace| {
        trace.attempts += 1;
        trace.status = None;
        trace.request_id = None;
        trace.first_delta = None;
        trace.transport_failure = None;
    });
}

pub fn request_id_of(headers: &reqwest::header::HeaderMap) -> Option<String> {
    ["request-id", "x-request-id"]
        .iter()
        .find_map(|name| headers.get(*name)?.to_str().ok())
        .map(str::to_string)
}

pub fn response_received(status: u16, request_id: Option<&str>) {
    write(|trace| {
        trace.status = Some(status);
        trace.request_id = request_id.map(str::to_string);
    });
}

pub(crate) fn transport_failed(failure: CallFailure) {
    write(|trace| trace.transport_failure = Some(failure));
}

pub(crate) fn content_arrived() {
    write(|trace| {
        trace.first_delta.get_or_insert_with(Instant::now);
    });
}

pub fn routed_to(provider: ProviderKind) {
    write(|trace| trace.provider = Some(provider));
}

#[cfg(test)]
#[path = "call_trace_tests.rs"]
mod tests;
