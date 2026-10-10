//! What a model call through the credentialed proxy cost.
//!
//! The proxy forwards an app's or a script's bytes to an upstream it never
//! parses. A model provider upstream still spends the user's money, so the
//! call records a `ContextCaptured` like every engine call (ADR 0242). It
//! records only what the provider reported: a reply with no usage block, such
//! as a quota read, records nothing.
//!
//! The record runs on its own task. The caller gets its response unchanged and
//! no later than the proxy would have returned it.

use axum::body::{Body, Bytes};
use axum::http::header::{ACCEPT_ENCODING, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use std::sync::Arc;
use uuid::Uuid;

use crate::engine::{AuxCapture, ContextPurpose, LucidosEngine};
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
    let streamed = parts
        .headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    if let Some((usage, model)) = reported(&bytes, streamed, request_body, path) {
        let capture =
            AuxCapture::for_thread_or_home(&engine.event_bus, cost_thread, ContextPurpose::Proxy);
        let request_chars = String::from_utf8_lossy(request_body).chars().count();
        tokio::spawn(async move {
            capture
                .record_usage(
                    &model,
                    request_chars,
                    Some(crate::engine::model_call::usage_from_provider(usage)),
                )
                .await;
        });
    }
    Response::from_parts(parts, Body::from(bytes))
}

/// The usage a reply reported and the model that served it, or `None` when
/// the reply names no prompt tokens.
fn reported(
    reply: &[u8],
    streamed: bool,
    request_body: &[u8],
    path: &str,
) -> Option<(ProviderUsage, String)> {
    let reply_json = (!streamed)
        .then(|| serde_json::from_slice::<serde_json::Value>(reply).ok())
        .flatten();
    let usage = match &reply_json {
        Some(serde_json::Value::Array(chunks)) => usage_wire::from_chunks(chunks),
        Some(json) => usage_wire::from_json(json),
        None => usage_wire::from_event_stream(&String::from_utf8_lossy(reply)),
    }?;
    Some((usage, model_of(reply_json.as_ref(), request_body, path)))
}

/// The model a reply names, else the one the request asked for, else the one
/// a Gemini path carries (`models/<model>:<method>`).
fn model_of(reply: Option<&serde_json::Value>, request_body: &[u8], path: &str) -> String {
    let named = |json: &serde_json::Value| {
        ["model", "modelVersion"]
            .iter()
            .find_map(|key| json.get(*key).and_then(|m| m.as_str()))
            .filter(|m| !m.is_empty())
            .map(str::to_string)
    };
    let reply = match reply {
        Some(serde_json::Value::Array(chunks)) => chunks.last(),
        other => other,
    };
    reply
        .and_then(named)
        .or_else(|| {
            serde_json::from_slice::<serde_json::Value>(request_body)
                .ok()
                .as_ref()
                .and_then(named)
        })
        .or_else(|| {
            let after = path.rsplit_once("models/")?.1;
            let model = after.split([':', '/']).next()?;
            (!model.is_empty()).then(|| model.to_string())
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "proxy_cost_tests.rs"]
mod tests;
