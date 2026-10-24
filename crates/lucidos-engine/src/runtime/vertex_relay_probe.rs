//! Temporary diagnostic: the Claude Code prompt-cache probe, inside the Vertex
//! relay.
//!
//! Each relayed call logs one line. It joins what Claude Code sent with what
//! Vertex answered, so a zero cache read can be told apart. Either we sent
//! different bytes, or Vertex served the call from a cache that did not hold
//! them. Every cache-prefix segment is hashed on its raw wire bytes: the
//! `tools` array, each `system` block and the first message.
//!
//! A hash covers the raw bytes, `cache_control` markers included. A marker that
//! moves between calls moves the hash of its segment.
//!
//! The probe sees the call as forwarded, after the relay's own rewrite, so the
//! line shows what Vertex received. It only borrows. It never changes a byte the relay forwards, and an
//! input it cannot read leaves that field `-`. The line holds hashes, sizes and
//! routing headers. It never holds body bytes, the relay token or a credential.
//!
//! Registered in `docs/temporary-measures.md`. The method is in
//! `docs/plans/2026-10-18-claude-code-cache-probe.md`.

use axum::body::Bytes;
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::value::RawValue;
use uuid::Uuid;

use crate::llm::stable_prefix::short_hash;

/// How much of a response the probe reads while it looks for `message_start`.
/// That event opens every stream, so a response that outgrows this has none.
const MAX_SCAN_BYTES: usize = 64 * 1024;

/// Response headers whose values reach the log: standard and routing headers.
/// Any other header is logged by name only. A proxy can put anything in one,
/// and the routing question needs the names more than the values.
const LOGGED_HEADER_NAMES: [&str; 12] = [
    "alt-svc",
    "cache-control",
    "content-type",
    "date",
    "retry-after",
    "server",
    "server-timing",
    "vary",
    "via",
    "request-id",
    "x-request-id",
    "x-cloud-trace-context",
];

/// Prefixes of response headers whose values reach the log.
const LOGGED_HEADER_PREFIXES: [&str; 5] =
    ["anthropic-", "x-goog-", "x-envoy-", "x-ratelimit-", "cf-"];

/// What stands in for the value of a header that is logged by name only.
const WITHHELD: &str = "-";

/// Print one probe line to the engine log.
pub(super) fn log_line(line: String) {
    crate::log!("{line}");
}

/// What one call looked like on the way in, rendered as line fields.
pub(super) struct CallProbe {
    fields: String,
}

impl CallProbe {
    pub(super) fn of(
        thread_id: Uuid,
        model: &str,
        location: &str,
        upstream_url: &str,
        request_headers: &HeaderMap,
        body: &[u8],
    ) -> Self {
        let beta = request_headers
            .get("anthropic-beta")
            .and_then(|v| v.to_str().ok());
        let fields = format!(
            "thread={thread_id} model={model} location={location} host={} beta={} {}",
            host_of(upstream_url),
            beta.unwrap_or("-"),
            Fingerprint::of(body).fields(),
        );
        Self { fields }
    }

    /// Start watching the response. `emit` receives the finished line once.
    pub(super) fn watch<E: FnOnce(String)>(
        self,
        status: u16,
        response_headers: &HeaderMap,
        emit: E,
    ) -> ResponseWatch<E> {
        ResponseWatch {
            fields: format!(
                "{} status={status} headers={}",
                self.fields,
                headers_json(response_headers)
            ),
            buffer: Vec::new(),
            emit: Some(emit),
        }
    }
}

/// Reads the response chunks as they pass, until it finds `message_start`.
/// It emits the line then, or when the stream ends, errors or is dropped.
pub(super) struct ResponseWatch<E: FnOnce(String)> {
    fields: String,
    buffer: Vec<u8>,
    emit: Option<E>,
}

impl<E: FnOnce(String)> ResponseWatch<E> {
    /// Look at one chunk. Never changes it, and never fails.
    pub(super) fn see<Err>(&mut self, chunk: &Result<Bytes, Err>) {
        if self.emit.is_none() {
            return;
        }
        let Ok(bytes) = chunk else {
            return self.finish(None);
        };
        self.buffer.extend_from_slice(bytes);
        if let Some(start) = take_message_start(&mut self.buffer) {
            self.finish(Some(start));
        } else if self.buffer.len() > MAX_SCAN_BYTES {
            self.finish(None);
        }
    }

    fn finish(&mut self, start: Option<MessageStart>) {
        let Some(emit) = self.emit.take() else {
            return;
        };
        self.buffer = Vec::new();
        let start = start.unwrap_or_default();
        emit(format!(
            "[VertexRelayProbe] {} message_id={} input={} cache_read={} cache_creation={} output_at_start={}",
            self.fields,
            start.id.as_deref().unwrap_or("-"),
            number(start.usage.input_tokens),
            number(start.usage.cache_read_input_tokens),
            number(start.usage.cache_creation_input_tokens),
            number(start.usage.output_tokens),
        ));
    }
}

impl<E: FnOnce(String)> Drop for ResponseWatch<E> {
    fn drop(&mut self) {
        self.finish(None);
    }
}

// ===== Request fingerprint =====

/// The wire bytes of the cache-prefix segments, hashed.
#[derive(Debug, Default, PartialEq, Eq)]
struct Fingerprint {
    body_bytes: usize,
    tools: Option<Segment>,
    system: Option<Vec<Segment>>,
    first_message: Option<Segment>,
    messages_n: Option<usize>,
    thinking: Option<Segment>,
}

#[derive(Debug, PartialEq, Eq)]
struct Segment {
    hash: String,
    bytes: usize,
}

impl Segment {
    fn of(raw: &RawValue) -> Self {
        let bytes = raw.get().as_bytes();
        Self {
            hash: short_hash(bytes),
            bytes: bytes.len(),
        }
    }
}

/// The top-level values the probe hashes, borrowed from the body.
#[derive(Deserialize)]
struct Parts<'a> {
    #[serde(borrow)]
    tools: Option<&'a RawValue>,
    #[serde(borrow)]
    system: Option<&'a RawValue>,
    #[serde(borrow)]
    messages: Option<Vec<&'a RawValue>>,
    #[serde(borrow)]
    thinking: Option<&'a RawValue>,
}

impl Fingerprint {
    fn of(body: &[u8]) -> Self {
        let Ok(parts) = serde_json::from_slice::<Parts>(body) else {
            return Self {
                body_bytes: body.len(),
                ..Self::default()
            };
        };
        Self {
            body_bytes: body.len(),
            tools: parts.tools.map(Segment::of),
            system: parts.system.map(system_segments),
            first_message: parts
                .messages
                .as_ref()
                .and_then(|m| m.first().copied())
                .map(Segment::of),
            messages_n: parts.messages.as_ref().map(Vec::len),
            thinking: parts.thinking.map(Segment::of),
        }
    }

    fn fields(&self) -> String {
        let (tools_hash, tools_bytes) = split(self.tools.as_ref());
        let (message_hash, message_bytes) = split(self.first_message.as_ref());
        let (thinking_hash, _) = split(self.thinking.as_ref());
        let system = self.system.as_deref().unwrap_or(&[]);
        format!(
            "body_bytes={} tools_hash={tools_hash} tools_bytes={tools_bytes} system_n={} \
             system_hashes={} system_bytes={} msgs_n={} msg0_hash={message_hash} \
             msg0_bytes={message_bytes} thinking_hash={thinking_hash}",
            self.body_bytes,
            self.system
                .as_ref()
                .map_or("-".into(), |s| s.len().to_string()),
            list(system.iter().map(|s| s.hash.clone())),
            list(system.iter().map(|s| s.bytes.to_string())),
            number(self.messages_n),
        )
    }
}

/// A `system` array hashes block by block. A bare string is one segment.
fn system_segments(raw: &RawValue) -> Vec<Segment> {
    if !raw.get().starts_with('[') {
        return vec![Segment::of(raw)];
    }
    match serde_json::from_str::<Vec<&RawValue>>(raw.get()) {
        Ok(blocks) => blocks.into_iter().map(Segment::of).collect(),
        Err(_) => vec![Segment::of(raw)],
    }
}

fn split(segment: Option<&Segment>) -> (&str, String) {
    segment.map_or(("-", "-".to_string()), |s| {
        (s.hash.as_str(), s.bytes.to_string())
    })
}

// ===== Response =====

#[derive(Debug, Default, PartialEq, Eq)]
struct MessageStart {
    id: Option<String>,
    usage: Usage,
}

#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    kind: String,
    message: Option<StreamMessage>,
}

#[derive(Deserialize)]
struct StreamMessage {
    id: Option<String>,
    usage: Option<Usage>,
}

/// Read complete lines off the front of `buffer` until one is the
/// `message_start` data line. Keeps the partial last line for the next chunk.
fn take_message_start(buffer: &mut Vec<u8>) -> Option<MessageStart> {
    let mut consumed = 0;
    let mut found = None;
    while let Some(len) = buffer[consumed..].iter().position(|&b| b == b'\n') {
        let line = &buffer[consumed..consumed + len];
        consumed += len + 1;
        if let Some(start) = message_start_of(line) {
            found = Some(start);
            break;
        }
    }
    buffer.drain(..consumed);
    found
}

fn message_start_of(line: &[u8]) -> Option<MessageStart> {
    let data = line.strip_prefix(b"data:")?;
    let event: StreamEvent = serde_json::from_slice(data.trim_ascii()).ok()?;
    if event.kind != "message_start" {
        return None;
    }
    let message = event.message;
    Some(MessageStart {
        id: message.as_ref().and_then(|m| m.id.clone()),
        usage: message.and_then(|m| m.usage).unwrap_or_default(),
    })
}

/// The response headers as one JSON object. A header outside the logged set
/// keeps its name and loses its value. A repeated header joins its values with
/// a comma.
fn headers_json(headers: &HeaderMap) -> String {
    let mut object = serde_json::Map::new();
    for name in headers.keys() {
        let name = name.as_str();
        let value = if value_is_logged(name) {
            headers
                .get_all(name)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .collect::<Vec<_>>()
                .join(",")
        } else {
            WITHHELD.to_string()
        };
        object.insert(name.to_string(), value.into());
    }
    serde_json::Value::Object(object).to_string()
}

fn value_is_logged(name: &str) -> bool {
    LOGGED_HEADER_NAMES.contains(&name)
        || LOGGED_HEADER_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

// ===== Field formatting =====

fn number<N: ToString>(value: Option<N>) -> String {
    value.map_or_else(|| "-".to_string(), |n| n.to_string())
}

fn list(items: impl Iterator<Item = String>) -> String {
    let joined = items.collect::<Vec<_>>().join(",");
    if joined.is_empty() {
        "-".to_string()
    } else {
        joined
    }
}

/// The host of `url`. The URL itself holds a project id and maybe credentials,
/// so only its host is logged.
fn host_of(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
#[path = "vertex_relay_probe_tests.rs"]
mod tests;
