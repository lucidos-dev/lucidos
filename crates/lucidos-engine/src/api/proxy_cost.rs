//! What a model call through the credentialed proxy cost.
//!
//! The proxy forwards an app's or a script's bytes to an upstream it never
//! parses. A model provider upstream still spends the user's money, so the
//! call records a `ContextCaptured` like every engine call (ADR 0242). It
//! records only what the provider reported: a reply with no usage block, such
//! as a quota read, records nothing.
//!
//! OpenAI speech reports usage only on a stream, or not at all for `tts-1`.
//! `api::proxy_speech` rewrites, records or refuses those requests.
//!
//! The record runs on its own task. The caller gets its response unchanged and
//! no later than the proxy would have returned it.

use axum::body::{Body, Bytes};
use axum::http::header::{ACCEPT_ENCODING, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use std::sync::Arc;
use uuid::Uuid;

use crate::engine::model_call::CallDetail;
use crate::engine::{AuxCapture, ContextPurpose, LucidosEngine};
use crate::llm::served_model::served_model_of;
use crate::llm::usage_wire::{self, ProviderUsage};

/// The Gemini API's host. Vertex hosts are recognised by `llm::vertex`.
const GEMINI_API_HOST: &str = "generativelanguage.googleapis.com";

/// Where a proxied request went.
pub(crate) enum Upstream<'a> {
    /// A builtin provider proxy. Every one is a model provider.
    Builtin,
    /// An `apis.json` entry, known only by its base URL.
    Configured { base_url: &'a str },
}

impl Upstream<'_> {
    fn is_model_provider(&self) -> bool {
        match self {
            Self::Builtin => true,
            Self::Configured { base_url } => is_model_host(base_url),
        }
    }
}

/// Ask a model provider for a reply the cost parse can read.
///
/// The engine's client decodes no content encoding, and the proxy hands the
/// upstream's encoding to the caller as it came. So a caller's
/// `Accept-Encoding` (every browser sends one) would bring back a gzip or
/// brotli body no parse can read. Without it the reply is plain, which every
/// caller reads the same way.
pub(crate) fn prepare_request(upstream: &Upstream<'_>, headers: &mut HeaderMap) {
    if upstream.is_model_provider() {
        headers.remove(ACCEPT_ENCODING);
    }
}

/// Whether `base_url` points at a model provider's API.
fn is_model_host(base_url: &str) -> bool {
    use crate::engine::tools::credentials::host_of;
    let Some(host) = host_of(base_url) else {
        return false;
    };
    let provider_bases = [
        crate::llm::openai::OPENAI_DEFAULT_BASE_URL,
        crate::llm::ANTHROPIC_API_BASE_URL,
        crate::llm::OPENROUTER_BASE_URL,
        crate::llm::XAI_BASE_URL,
        crate::llm::judgment::TYPESAFE_API_BASE_URL,
    ];
    provider_bases
        .iter()
        .filter_map(|base| host_of(base))
        .any(|known| known == host)
        || host == GEMINI_API_HOST
        || crate::llm::vertex::is_vertex_host(&host)
}

/// The thread a proxied call's cost belongs to: the one the origin token
/// proves made it. Anything else records on the home thread.
pub(crate) fn cost_thread(headers: &axum::http::HeaderMap) -> Option<Uuid> {
    match crate::api::actor::subprocess_origin(headers) {
        crate::api::actor::SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } => source_thread_id,
        crate::api::actor::SubprocessOrigin::NotSubprocess => None,
    }
}

/// Record what a proxied model call cost, and hand back its response.
pub(crate) async fn record_model_call(
    engine: &Arc<LucidosEngine>,
    cost_thread: Option<Uuid>,
    upstream: Upstream<'_>,
    path: &str,
    request_body: &Bytes,
    response: Response,
) -> Response {
    if !upstream.is_model_provider() {
        return response;
    }
    let (parts, body) = response.into_parts();
    // `forward_request` buffers every reply whole, so this reads memory.
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(e) => {
            crate::log!("[Proxy] Could not read a model reply: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                format!("could not read the reply: {e}"),
            )
                .into_response();
        }
    };
    let streamed = is_event_stream(&parts.headers);
    if let Some(call) = reported(&bytes, streamed, request_body, path) {
        record(engine, cost_thread, call, request_body);
    }
    Response::from_parts(parts, Body::from(bytes))
}

/// Record one proxied call's cost, on a task of its own.
pub(crate) fn record(
    engine: &Arc<LucidosEngine>,
    cost_thread: Option<Uuid>,
    call: ReportedCall,
    request_body: &[u8],
) {
    let capture =
        AuxCapture::for_thread_or_home(&engine.event_bus, cost_thread, ContextPurpose::Proxy);
    let request_chars = String::from_utf8_lossy(request_body).chars().count();
    tokio::spawn(async move {
        capture
            .record_usage_detailed(
                &call.model,
                request_chars,
                Some(crate::engine::model_call::usage_from_provider(call.usage)),
                CallDetail {
                    served_model: call.served_model,
                    ..CallDetail::default()
                },
            )
            .await;
    });
}

/// Whether a reply is a server-sent event stream.
pub(crate) fn is_event_stream(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"))
}

/// What one forwarded call reported.
#[derive(Debug, PartialEq)]
pub(crate) struct ReportedCall {
    pub(crate) usage: ProviderUsage,
    /// The model the request asked for, else the one the reply named.
    pub(crate) model: String,
    /// The model the reply named. A streamed reply is not read for it.
    pub(crate) served_model: Option<String>,
}

/// What a reply reported, or `None` when it names no prompt tokens.
fn reported(reply: &[u8], streamed: bool, request_body: &[u8], path: &str) -> Option<ReportedCall> {
    let reply_json = (!streamed)
        .then(|| serde_json::from_slice::<serde_json::Value>(reply).ok())
        .flatten();
    let usage = match &reply_json {
        Some(serde_json::Value::Array(chunks)) => usage_wire::from_chunks(chunks),
        Some(json) => usage_wire::from_json(json),
        None => usage_wire::from_event_stream(&String::from_utf8_lossy(reply)),
    }?;
    let served_model = match &reply_json {
        Some(serde_json::Value::Array(chunks)) => chunks.last(),
        other => other.as_ref(),
    }
    .and_then(served_model_of);
    let model = requested_model_of(request_body, path)
        .or_else(|| served_model.clone())
        .unwrap_or_default();
    Some(ReportedCall {
        usage,
        model,
        served_model,
    })
}

/// The model the request asked for: the body's, else the one a Gemini path
/// carries (`models/<model>:<method>`).
fn requested_model_of(request_body: &[u8], path: &str) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(request_body)
        .ok()
        .and_then(|body| Some(body.get("model")?.as_str()?.to_string()))
        .filter(|model| !model.is_empty())
        .or_else(|| {
            let after = path.rsplit_once("models/")?.1;
            let model = after.split([':', '/']).next()?;
            (!model.is_empty()).then(|| model.to_string())
        })
}

#[cfg(test)]
#[path = "proxy_cost_tests.rs"]
mod tests;
