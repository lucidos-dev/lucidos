//! Tests for the relay's prompt-cache probe. Each group names the plan
//! invariant it pins: `docs/plans/2026-10-18-claude-code-cache-probe.md`.

use super::*;
use axum::http::HeaderValue;
use std::sync::{Arc, Mutex};

const MESSAGE_START: &str = concat!(
    "event: message_start\n",
    r#"data: {"type":"message_start","message":{"id":"msg_vrtx_01","usage":{"#,
    r#""input_tokens":6,"cache_read_input_tokens":43302,"#,
    r#""cache_creation_input_tokens":12,"output_tokens":1}}}"#,
    "\n\n",
);

type Lines = Arc<Mutex<Vec<String>>>;

fn probe_of(body: &[u8]) -> CallProbe {
    CallProbe::of(
        Uuid::nil(),
        "claude-opus-5-5",
        "eu",
        "https://aiplatform.eu.rep.googleapis.com/v1/projects/p/x",
        &HeaderMap::new(),
        body,
    )
}

fn watch_of(probe: CallProbe, headers: &HeaderMap) -> (ResponseWatch<impl FnOnce(String)>, Lines) {
    let lines: Lines = Arc::default();
    let sink = lines.clone();
    let watch = probe.watch(200, headers, move |line| sink.lock().unwrap().push(line));
    (watch, lines)
}

fn chunk(text: &str) -> Result<Bytes, std::io::Error> {
    Ok(Bytes::from(text.to_string()))
}

fn field<'a>(line: &'a str, name: &str) -> &'a str {
    let prefix = format!("{name}=");
    line.split(' ')
        .find_map(|f| f.strip_prefix(prefix.as_str()))
        .unwrap_or_else(|| panic!("no {name} in {line}"))
}

// ── Invariant: a segment's hash depends only on its wire bytes ─────────────

#[test]
fn one_byte_of_tools_moves_only_the_tools_hash() {
    let a =
        Fingerprint::of(br#"{"tools":[{"name":"Bash"}],"system":[{"a":1}],"messages":[{"m":1}]}"#);
    let b =
        Fingerprint::of(br#"{"tools":[{"name":"Bush"}],"system":[{"a":1}],"messages":[{"m":1}]}"#);
    assert_ne!(a.tools, b.tools);
    assert_eq!(a.system, b.system);
    assert_eq!(a.first_message, b.first_message);
}

#[test]
fn equal_bytes_hash_equal_whatever_surrounds_them() {
    let a = Fingerprint::of(br#"{"model":"x","tools":[1, 2],"messages":[]}"#);
    let b = Fingerprint::of(br#"{"messages":[],  "tools":[1, 2]}"#);
    assert_eq!(a.tools, b.tools);
}

/// A re-serialization would hide this, and the cache would still miss.
#[test]
fn spacing_inside_a_segment_moves_its_hash() {
    let a = Fingerprint::of(br#"{"tools":[1,2]}"#);
    let b = Fingerprint::of(br#"{"tools":[1, 2]}"#);
    assert_ne!(a.tools, b.tools);
}

#[test]
fn the_thinking_value_has_its_own_hash() {
    let a = Fingerprint::of(br#"{"thinking":{"type":"adaptive"}}"#);
    let b = Fingerprint::of(br#"{"thinking":{"type":"adaptive","display":"updates"}}"#);
    assert_ne!(a.thinking, b.thinking);
    assert_eq!(a.tools, b.tools);
}

#[test]
fn each_system_block_gets_its_own_hash() {
    let a = Fingerprint::of(br#"{"system":[{"t":"one"},{"t":"two"}]}"#);
    let b = Fingerprint::of(br#"{"system":[{"t":"one"},{"t":"TWO"}]}"#);
    let (a, b) = (a.system.unwrap(), b.system.unwrap());
    assert_eq!(a.len(), 2);
    assert_eq!(a[0], b[0]);
    assert_ne!(a[1], b[1]);
}

#[test]
fn a_bare_system_string_is_one_segment() {
    let fingerprint = Fingerprint::of(br#"{"system":"you are helpful"}"#);
    assert_eq!(fingerprint.system.unwrap().len(), 1);
}

#[test]
fn only_the_first_message_is_hashed() {
    let a = Fingerprint::of(br#"{"messages":[{"m":1},{"m":2}]}"#);
    let b = Fingerprint::of(br#"{"messages":[{"m":1},{"m":3},{"m":4}]}"#);
    assert_eq!(a.first_message, b.first_message);
    assert_eq!((a.messages_n, b.messages_n), (Some(2), Some(3)));
}

// ── Invariant: the probe never fails a call ────────────────────────────────

#[test]
fn a_body_that_is_not_json_leaves_every_segment_unknown() {
    let line = probe_of(b"not json").fields;
    assert_eq!(field(&line, "body_bytes"), "8");
    for unknown in [
        "tools_hash",
        "system_hashes",
        "msg0_hash",
        "msgs_n",
        "thinking_hash",
    ] {
        assert_eq!(field(&line, unknown), "-", "{unknown}");
    }
}

#[test]
fn a_body_with_no_prefix_segments_leaves_them_unknown() {
    let line = probe_of(br#"{"max_tokens":1}"#).fields;
    assert_eq!(field(&line, "tools_hash"), "-");
    assert_eq!(field(&line, "system_n"), "-");
}

#[test]
fn message_start_gives_the_id_and_usage() {
    let (mut watch, lines) = watch_of(probe_of(b"{}"), &HeaderMap::new());
    watch.see(&chunk(MESSAGE_START));
    let lines = lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    assert_eq!(field(line, "message_id"), "msg_vrtx_01");
    assert_eq!(field(line, "input"), "6");
    assert_eq!(field(line, "cache_read"), "43302");
    assert_eq!(field(line, "cache_creation"), "12");
    assert_eq!(field(line, "output_at_start"), "1");
    assert_eq!(field(line, "status"), "200");
}

/// The event arrives split mid-line, as a real network chunk can split it.
#[test]
fn message_start_split_across_chunks_is_still_found() {
    let (mut watch, lines) = watch_of(probe_of(b"{}"), &HeaderMap::new());
    let (head, tail) = MESSAGE_START.split_at(40);
    watch.see(&chunk("event: ping\ndata: {\"type\":\"ping\"}\n\n"));
    watch.see(&chunk(head));
    assert!(lines.lock().unwrap().is_empty());
    watch.see(&chunk(tail));
    assert_eq!(
        field(&lines.lock().unwrap()[0], "message_id"),
        "msg_vrtx_01"
    );
}

#[test]
fn a_json_response_leaves_the_usage_unknown() {
    let (mut watch, lines) = watch_of(probe_of(b"{}"), &HeaderMap::new());
    watch.see(&chunk(r#"{"id":"msg_1","usage":{"input_tokens":5}}"#));
    drop(watch);
    let line = lines.lock().unwrap()[0].clone();
    assert_eq!(field(&line, "message_id"), "-");
    assert_eq!(field(&line, "cache_read"), "-");
}

#[test]
fn a_stream_error_emits_one_line_with_the_usage_unknown() {
    let (mut watch, lines) = watch_of(probe_of(b"{}"), &HeaderMap::new());
    watch.see::<std::io::Error>(&Err(std::io::Error::other("reset")));
    watch.see(&chunk(MESSAGE_START));
    drop(watch);
    let lines = lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(field(&lines[0], "message_id"), "-");
}

#[test]
fn a_dropped_stream_still_emits_its_line_once() {
    let (watch, lines) = watch_of(probe_of(b"{}"), &HeaderMap::new());
    drop(watch);
    assert_eq!(lines.lock().unwrap().len(), 1);
}

#[test]
fn the_probe_stops_reading_once_it_has_the_event_or_the_cap() {
    let (mut watch, lines) = watch_of(probe_of(b"{}"), &HeaderMap::new());
    watch.see(&chunk(MESSAGE_START));
    watch.see(&chunk(&"x".repeat(MAX_SCAN_BYTES * 2)));
    assert!(watch.buffer.is_empty());
    drop(watch);
    assert_eq!(lines.lock().unwrap().len(), 1);

    let (mut watch, lines) = watch_of(probe_of(b"{}"), &HeaderMap::new());
    watch.see(&chunk(&"x".repeat(MAX_SCAN_BYTES + 1)));
    assert_eq!(lines.lock().unwrap().len(), 1);
    assert!(watch.buffer.is_empty());
}

// ── Invariant: no credential, token or user content reaches the log ────────

#[test]
fn the_line_holds_no_credential_and_no_body_content() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(
        "authorization",
        HeaderValue::from_static("Bearer ya29.SECRET"),
    );
    request_headers.insert("cookie", HeaderValue::from_static("sid=COOKIE"));
    request_headers.insert(
        "anthropic-beta",
        HeaderValue::from_static("claude-code-20250219"),
    );
    let body = br#"{"tools":[],"system":[{"text":"MARKER-system"}],"messages":[{"content":"MARKER-user"}]}"#;
    let probe = CallProbe::of(
        Uuid::nil(),
        "claude-opus-5-5",
        "eu",
        "https://user:URLPASS@aiplatform.eu.rep.googleapis.com/v1/projects/MARKER-project/x",
        &request_headers,
        body,
    );
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        "authorization",
        HeaderValue::from_static("Bearer ya29.RESPONSE"),
    );
    response_headers.insert("set-cookie", HeaderValue::from_static("a=SETCOOKIE"));
    response_headers.append("cookie", HeaderValue::from_static("c=COOKIE2"));
    response_headers.insert(
        "content-type",
        HeaderValue::from_static("text/event-stream"),
    );
    let (mut watch, lines) = watch_of(probe, &response_headers);
    watch.see(&chunk(MESSAGE_START));
    let line = lines.lock().unwrap()[0].clone();
    for leaked in [
        "ya29",
        "SECRET",
        "COOKIE",
        "SETCOOKIE",
        "URLPASS",
        "MARKER",
        "RESPONSE",
    ] {
        assert!(!line.contains(leaked), "{leaked} leaked into {line}");
    }
    assert!(line.contains(r#""content-type":"text/event-stream""#));
    assert_eq!(field(&line, "host"), "aiplatform.eu.rep.googleapis.com");
    assert_eq!(field(&line, "beta"), "claude-code-20250219");
}

#[test]
fn a_repeated_header_joins_its_values() {
    let mut headers = HeaderMap::new();
    headers.append("x-request-id", HeaderValue::from_static("a"));
    headers.append("x-request-id", HeaderValue::from_static("b"));
    assert_eq!(headers_json(&headers), r#"{"x-request-id":"a,b"}"#);
}

/// A proxy can put anything in a header it names itself, so an unknown header
/// keeps its name and loses its value.
#[test]
fn an_unknown_header_is_logged_by_name_only() {
    let mut headers = HeaderMap::new();
    headers.insert("x-auth-token", HeaderValue::from_static("TOKEN-VALUE"));
    headers.insert("x-goog-request-id", HeaderValue::from_static("g-1"));
    let json = headers_json(&headers);
    assert!(json.contains(r#""x-auth-token":"-""#), "{json}");
    assert!(json.contains(r#""x-goog-request-id":"g-1""#), "{json}");
    assert!(!json.contains("TOKEN-VALUE"));
}
