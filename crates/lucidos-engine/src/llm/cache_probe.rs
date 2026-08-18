//! Temporary diagnostic: the Anthropic prompt-cache wire probe.
//!
//! Off unless `LUCIDOS_CACHE_PROBE` is set. When off, nothing here serializes,
//! hashes or logs. The request is untouched either way: the probe only borrows.
//!
//! It answers one question. When a first-of-turn call reads zero cache, were
//! the bytes we sent identical to the previous call's? Each cache-prefix
//! segment is hashed on its serialized JSON, so two calls diff mechanically.
//! Identical hashes with `cache_read=0` puts the miss on Anthropic's side. A
//! differing hash names the segment we changed.
//!
//! Registered in `docs/temporary-measures.md`. The method, and what the
//! evidence already ruled out, is in
//! `docs/plans/2026-08-17-prompt-cache-wire-probe.md`.

use super::anthropic_wire::{ClaudeRequest, TurnMeta};
use serde::Serialize;
use std::sync::OnceLock;
use uuid::Uuid;

const ENV_VAR: &str = "LUCIDOS_CACHE_PROBE";

/// Correlation for one LLM call, carried on the task rather than through
/// `LlmProvider::chat`. The trait is provider-agnostic and a diagnostic must
/// not widen it. Scoped by the agentic loop, so the request line and the
/// response line join on `(thread_id, turn_id, round)`.
#[derive(Clone, Copy)]
pub(crate) struct ProbeCall {
    pub thread_id: Uuid,
    /// The turn's `request_event_id`, which is what `ContextCaptured` rows
    /// carry, so the probe log joins straight onto the events table.
    pub turn_id: Uuid,
    /// Rounds elapsed this turn, 1-based. Round 1 is the first-of-turn call
    /// whose cache reads are the thing under investigation.
    pub round: usize,
}

tokio::task_local! {
    static PROBE_CALL: ProbeCall;
}

/// Run `fut` with `call` visible to the probe. Always wraps, probe on or off:
/// the adapter is a pass-through poll, and branching on the env var would need
/// two future types for no measurable gain.
pub(crate) fn scope<F>(call: ProbeCall, fut: F) -> impl std::future::Future<Output = F::Output>
where
    F: std::future::Future,
{
    PROBE_CALL.scope(call, fut)
}

fn current() -> Option<ProbeCall> {
    PROBE_CALL.try_with(|call| *call).ok()
}

/// Read once: the var cannot change under a running engine, and the emit sites
/// are on the hot path of every Claude call.
fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| probe_enabled_value(std::env::var(ENV_VAR).ok().as_deref()))
}

fn probe_enabled_value(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "true" | "yes" | "on"))
}

/// Emit the request line. Returns before any work when the probe is off.
pub(crate) fn log_request(
    request: &ClaudeRequest,
    model: &str,
    request_url: &str,
    provider_tag: &str,
) {
    if !enabled() {
        return;
    }
    crate::log!(
        "{}",
        request_line(request, model, request_url, provider_tag, current())
    );
}

/// Emit the response line. Returns before any work when the probe is off.
pub(crate) fn log_response(meta: &TurnMeta, provider_tag: &str) {
    if !enabled() {
        return;
    }
    crate::log!("{}", response_line(meta, provider_tag, current()));
}

// ===== Line builders (pure, so the tests can call them with the probe off) =====

fn request_line(
    request: &ClaudeRequest,
    model: &str,
    request_url: &str,
    provider_tag: &str,
    call: Option<ProbeCall>,
) -> String {
    let markers = census(request);
    let tools = request.tools.as_deref().unwrap_or(&[]);

    // Slice to the marker, since only the prefix up to a breakpoint is what
    // Anthropic looks up. With no marker there is no cached prefix at all, so
    // the segment is empty and every field describes that same empty prefix.
    // The `_prefix=-` field is what tells the reader the breakpoint went
    // missing; do not read the hash of `[]` as "the tools were stable".
    let tools_prefix = &tools[..markers.tools_prefix_end.map_or(0, |i| i + 1)];
    let (tools_hash, tools_bytes) = segment(tools_prefix);
    let tool_names: Vec<&str> = tools_prefix.iter().map(|t| t.name.as_str()).collect();
    let (tool_names_hash, _) = segment(&tool_names);

    let (system_hash, system_bytes) = segment(&request.system);

    let messages_end = markers.messages_prefix_end.map_or(0, |i| i + 1);
    let (msgs_hash, msgs_bytes) = segment(&request.messages[..messages_end]);

    let (_, body_bytes) = segment(request);

    let beta = request.anthropic_beta.as_ref().map(|b| b.join(","));

    format!(
        "[CacheProbe] req {} provider={} model={} host={} anthropic_version={} \
         anthropic_beta={} tools_n={} tools_last={} tools_prefix={} tools_hash={} \
         tools_bytes={} tool_names_hash={} system_hash={} system_bytes={} msgs_n={} \
         msgs_prefix={} msgs_hash={} msgs_bytes={} marker_count={} markers={} body_bytes={}",
        correlation(call),
        provider_tag,
        model,
        host_of(request_url),
        optional(request.anthropic_version.as_deref()),
        optional(beta.as_deref()),
        tools.len(),
        optional(tool_names.last().copied()),
        index(markers.tools_prefix_end),
        tools_hash,
        tools_bytes,
        tool_names_hash,
        system_hash,
        system_bytes,
        request.messages.len(),
        index(markers.messages_prefix_end),
        msgs_hash,
        msgs_bytes,
        markers.positions.len(),
        list(&markers.positions),
        body_bytes,
    )
}

fn response_line(meta: &TurnMeta, provider_tag: &str, call: Option<ProbeCall>) -> String {
    // `input_tokens` is the witness that `message_start` arrived: the parser
    // sets it whenever the three usage counts sum above zero. See
    // `cache_tokens` for why the cache fields need one.
    let saw_usage = meta.input_tokens.is_some();
    format!(
        "[CacheProbe] resp {} provider={} cache_read={} cache_creation={} input={} output={}",
        correlation(call),
        provider_tag,
        cache_tokens(meta.cache_read_tokens, saw_usage),
        cache_tokens(meta.cache_creation_tokens, saw_usage),
        number(meta.input_tokens),
        number(meta.output_tokens),
    )
}

/// Render a cache count, separating a real zero from missing usage.
///
/// The parser records a cache field ONLY when it is non-zero, so a zero read
/// arrives here as `None`. That is the exact outcome the probe exists to
/// catch, and printing it as `-` would hide it behind the same glyph as "no
/// `message_start`". So an absent count reads as `0` once usage arrived, and
/// as `-` only when none did.
fn cache_tokens(value: Option<u32>, saw_usage: bool) -> String {
    match (value, saw_usage) {
        (Some(n), _) => n.to_string(),
        (None, true) => "0".to_string(),
        (None, false) => "-".to_string(),
    }
}

fn correlation(call: Option<ProbeCall>) -> String {
    match call {
        Some(call) => format!(
            "thread={} turn={} round={} first_of_turn={}",
            call.thread_id,
            call.turn_id,
            call.round,
            call.round == 1
        ),
        // A Claude call outside the agentic loop (memory extraction, web
        // search) has no turn to correlate to. It says so, rather than
        // borrowing whatever context happens to sit on the task.
        None => "thread=- turn=- round=- first_of_turn=-".to_string(),
    }
}

// ===== Segment hashing =====

/// Hash and byte length of one cache-prefix segment, computed on the
/// serialized JSON rather than the struct. A field reorder or a whitespace
/// change is invisible to the struct and fatal to the cache, so the bytes are
/// the only honest input.
fn segment<T: Serialize + ?Sized>(value: &T) -> (String, usize) {
    match serde_json::to_vec(value) {
        Ok(bytes) => (short_hash(&bytes), bytes.len()),
        // A sentinel rather than the error text, because every field on this
        // line has to stay whitespace-free for a mechanical diff. The same
        // failure hits the real request moments later, with a full message.
        Err(_) => ("unserializable".to_string(), 0),
    }
}

/// First 16 hex characters of the SHA-256. Wide enough that a collision
/// between two consecutive requests is not a practical concern, short enough
/// to eyeball two log lines side by side.
fn short_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// ===== Marker census =====

/// Every `cache_control` marker in the body, by position, plus where the tools
/// and messages prefixes end.
///
/// Deliberately scans ALL messages rather than trusting the placement code to
/// have marked only the last one. Anthropic allows 4 breakpoints per request
/// and rejects anything above that. So a marker accumulating in history across
/// a turn is a live suspect, and only a full scan can confirm or kill it.
struct Census {
    positions: Vec<String>,
    tools_prefix_end: Option<usize>,
    messages_prefix_end: Option<usize>,
}

fn census(request: &ClaudeRequest) -> Census {
    let mut positions = Vec::new();
    let mut tools_prefix_end = None;
    let mut messages_prefix_end = None;

    for (i, tool) in request.tools.as_deref().unwrap_or(&[]).iter().enumerate() {
        if tool.cache_control.is_some() {
            positions.push(format!("tools[{i}]"));
            tools_prefix_end = Some(i);
        }
    }

    for (i, block) in blocks_of(request.system.as_ref()).iter().enumerate() {
        if has_marker(block) {
            positions.push(format!("system[{i}]"));
        }
    }

    for (i, message) in request.messages.iter().enumerate() {
        for (j, block) in blocks_of(Some(&message.content)).iter().enumerate() {
            if has_marker(block) {
                positions.push(format!("messages[{i}][{j}]"));
                messages_prefix_end = Some(i);
            }
        }
    }

    Census {
        positions,
        tools_prefix_end,
        messages_prefix_end,
    }
}

/// The content blocks of a `system` or message `content` value. A bare string
/// carries no blocks, which is correct: the string form cannot hold a marker.
fn blocks_of(value: Option<&serde_json::Value>) -> &[serde_json::Value] {
    value
        .and_then(|v| v.as_array())
        .map_or(&[], |a| a.as_slice())
}

fn has_marker(block: &serde_json::Value) -> bool {
    block.get("cache_control").is_some()
}

// ===== Field formatting =====

fn optional(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

fn index(value: Option<usize>) -> String {
    value.map_or_else(|| "-".to_string(), |i| i.to_string())
}

fn number(value: Option<u32>) -> String {
    value.map_or_else(|| "-".to_string(), |n| n.to_string())
}

fn list(values: &[String]) -> String {
    if values.is_empty() {
        "-".to_string()
    } else {
        values.join(",")
    }
}

/// Host of `url`, or the whole string when it does not look like a URL. The
/// Vertex region host is a suspect in its own right: `vertex_region = eu`
/// resolves to the `aiplatform.eu.rep.googleapis.com` multi-region.
fn host_of(url: &str) -> &str {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    after_scheme.split('/').next().unwrap_or(after_scheme)
}

#[cfg(test)]
#[path = "cache_probe_tests.rs"]
mod tests;
