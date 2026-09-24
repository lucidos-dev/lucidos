//! Tests for the Vertex relay. Each group names the plan invariant it pins:
//! `docs/plans/2026-09-23-coding-agent-notes-hidden-on-opus-5-5.md`.

use super::*;
use std::sync::Mutex;

const OPUS_PATH: &str =
    "projects/p-1/locations/eu/publishers/anthropic/models/claude-opus-5-5:streamRawPredict";

fn install_secret() {
    crate::api::actor::init_agent_origin_secret("vertex-relay-test-secret".to_string());
}

fn raw_fields(body: &[u8]) -> std::collections::BTreeMap<String, String> {
    let fields: std::collections::BTreeMap<String, Box<RawValue>> =
        serde_json::from_slice(body).expect("a JSON object");
    fields
        .into_iter()
        .map(|(k, v)| (k, v.get().to_string()))
        .collect()
}

// ── Invariant 1 and 7: the rewrite ─────────────────────────────────────────

/// Odd spacing and key order inside `messages` and `tools` must survive, since
/// prompt caching and tool rendering read those bytes.
#[test]
fn only_the_thinking_value_changes_and_every_other_value_keeps_its_bytes() {
    let body = br#"{"messages": [ {"role" :"user","content":"hi"} ],
        "tools":[{"name":"Bash","input_schema":{"z":1,"a":2}}],
        "thinking": {"type":"adaptive"}, "max_tokens": 128000}"#;
    let rewritten = ask_for_progress_notes(body, "claude-opus-5-5").expect("rewritten");
    let before = raw_fields(body);
    let after = raw_fields(&rewritten);
    assert_eq!(before.len(), after.len());
    for (key, value) in &before {
        if key != "thinking" {
            assert_eq!(&after[key], value, "{key} must forward byte for byte");
        }
    }
    let thinking: serde_json::Value = serde_json::from_str(&after["thinking"]).unwrap();
    assert_eq!(thinking["type"], "adaptive");
    assert_eq!(thinking["display"], "updates");
}

#[test]
fn only_always_thinking_models_are_rewritten() {
    let body = br#"{"thinking":{"type":"adaptive"}}"#;
    for (model, rewritten) in [
        ("claude-opus-5-5", true),
        ("claude-opus-5-5@20260901", true),
        ("claude-fable-5-1", true),
        ("claude-fable-5", true),
        ("claude-opus-5", false),
        ("claude-sonnet-5", false),
        ("claude-haiku-4-5@20251001", false),
        ("count-tokens", false),
    ] {
        assert_eq!(
            ask_for_progress_notes(body, model).is_some(),
            rewritten,
            "{model}"
        );
    }
}

#[test]
fn a_body_that_needs_no_change_forwards_untouched() {
    for body in [
        &br#"{"thinking":{"type":"disabled"}}"#[..],
        br#"{"thinking":{"type":"adaptive","display":"updates"}}"#,
        br#"{"messages":[]}"#,
        br#"{"thinking":"adaptive"}"#,
        b"not json",
    ] {
        assert_eq!(
            ask_for_progress_notes(body, "claude-opus-5-5"),
            None,
            "{}",
            String::from_utf8_lossy(body)
        );
    }
}

/// An explicit `omitted` hides the notes exactly as the default does.
#[test]
fn an_explicit_hiding_display_is_replaced() {
    let body = br#"{"thinking":{"type":"adaptive","display":"omitted"}}"#;
    let after = raw_fields(&ask_for_progress_notes(body, "claude-opus-5-5").unwrap());
    let thinking: serde_json::Value = serde_json::from_str(&after["thinking"]).unwrap();
    assert_eq!(thinking["display"], "updates");
}

// ── Invariant 2: headers ───────────────────────────────────────────────────

#[test]
fn credentials_pass_through_and_hop_headers_do_not() {
    let mut incoming = HeaderMap::new();
    incoming.insert("authorization", HeaderValue::from_static("Bearer ya29.x"));
    incoming.insert("x-goog-user-project", HeaderValue::from_static("p-1"));
    incoming.insert("host", HeaderValue::from_static("127.0.0.1:1"));
    incoming.insert("content-length", HeaderValue::from_static("12"));
    incoming.insert("accept-encoding", HeaderValue::from_static("gzip"));
    incoming.insert("anthropic-beta", HeaderValue::from_static("a-1, b-2"));

    let forwarded = forwarded_request_headers(&incoming, true);
    assert_eq!(forwarded["authorization"], "Bearer ya29.x");
    assert_eq!(forwarded["x-goog-user-project"], "p-1");
    for dropped in ["host", "content-length", "accept-encoding"] {
        assert!(forwarded.get(dropped).is_none(), "{dropped} must not cross");
    }
    assert_eq!(
        forwarded["anthropic-beta"],
        format!("a-1,b-2,{ANTHROPIC_BETA_THINKING_DISPLAY_UPDATES}")
    );
}

#[test]
fn the_beta_is_added_once_and_only_with_a_rewrite() {
    let mut incoming = HeaderMap::new();
    incoming.insert(
        "anthropic-beta",
        HeaderValue::from_static(ANTHROPIC_BETA_THINKING_DISPLAY_UPDATES),
    );
    assert_eq!(
        forwarded_request_headers(&incoming, true)["anthropic-beta"],
        ANTHROPIC_BETA_THINKING_DISPLAY_UPDATES
    );
    let untouched = HeaderMap::new();
    assert!(forwarded_request_headers(&untouched, false)
        .get("anthropic-beta")
        .is_none());
}

// ── Invariant 3: the token ─────────────────────────────────────────────────

#[test]
fn a_token_round_trips_its_thread_and_override() {
    install_secret();
    let thread = Uuid::new_v4();
    let token = mint_token(thread, Some("https://proxy.example/v1")).unwrap();
    assert_eq!(
        verify_token(&token),
        Some(Grant {
            thread_id: thread,
            upstream_override: Some("https://proxy.example/v1".to_string()),
        })
    );
    let plain = mint_token(thread, None).unwrap();
    assert_eq!(verify_token(&plain).unwrap().upstream_override, None);
    assert!(plain
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-.".contains(&b)));
}

/// The override names the host, so editing it must break the MAC.
#[test]
fn an_edited_token_does_not_verify() {
    install_secret();
    let token = mint_token(Uuid::new_v4(), None).unwrap();
    let (payload, mac) = token.rsplit_once('.').unwrap();
    let (thread, _) = payload.split_once('.').unwrap();
    let evil = crate::api::hex::hex_lower(b"https://evil.test");
    assert_eq!(verify_token(&format!("{thread}.{evil}.{mac}")), None);
    let other = Uuid::new_v4();
    assert_eq!(verify_token(&format!("{other}.-.{mac}")), None);
    assert_eq!(verify_token("garbage"), None);
}

/// The two credentials share a secret, so each must fail as the other.
#[test]
fn a_relay_token_and_an_origin_token_never_stand_in_for_each_other() {
    use crate::api::actor::{
        mint_agent_origin_token, subprocess_origin, SubprocessOrigin, HEADER_AGENT_ORIGIN_TOKEN,
    };
    install_secret();
    let thread = Uuid::new_v4();
    let origin = mint_agent_origin_token(Some(thread), 0, None).unwrap();
    assert_eq!(verify_token(&origin), None);

    let relay_token = mint_token(thread, None).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        HEADER_AGENT_ORIGIN_TOKEN,
        HeaderValue::from_str(&relay_token).unwrap(),
    );
    assert_eq!(subprocess_origin(&headers), SubprocessOrigin::NotSubprocess);
}

#[test]
fn a_relay_url_is_never_taken_as_an_override() {
    assert_eq!(
        usable_override("https://proxy.example/v1/"),
        Some("https://proxy.example/v1")
    );
    assert_eq!(
        usable_override("http://127.0.0.1:5/api/v1/vertex-relay/abc"),
        None
    );
    assert_eq!(usable_override("file:///etc/passwd"), None);
    assert_eq!(usable_override(""), None);
}

// ── Invariant 4: the path and the host ─────────────────────────────────────

#[test]
fn only_a_vertex_model_call_is_accepted() {
    assert_eq!(
        parse_vertex_call(OPUS_PATH),
        Some(VertexCall {
            location: "eu",
            model: "claude-opus-5-5"
        })
    );
    let count =
        "projects/p/locations/europe-west1/publishers/anthropic/models/count-tokens:rawPredict";
    assert!(parse_vertex_call(count).is_some());
    for refused in [
        "projects/p/locations/evil.test#/publishers/anthropic/models/m:rawPredict",
        "projects/p/locations/EU/publishers/anthropic/models/m:rawPredict",
        "projects/p/locations/eu/publishers/google/models/m:rawPredict",
        "projects/p/locations/eu/publishers/anthropic/models/m:generateContent",
        "projects/p/locations/eu/publishers/anthropic/models/m",
        "projects/p/locations/eu/publishers/anthropic/models/m:rawPredict/extra",
        "projects/../locations/eu/publishers/anthropic/models/m:rawPredict",
        "private/tmp/somewhere",
    ] {
        assert_eq!(parse_vertex_call(refused), None, "{refused}");
    }
}

#[test]
fn the_host_comes_from_the_location_or_the_override_only() {
    let call = parse_vertex_call(OPUS_PATH).unwrap();
    let google = Grant {
        thread_id: Uuid::nil(),
        upstream_override: None,
    };
    assert_eq!(
        upstream_url(&google, &call, OPUS_PATH, Some("alt=sse")),
        format!("https://aiplatform.eu.rep.googleapis.com/v1/{OPUS_PATH}?alt=sse")
    );
    let proxy = Grant {
        thread_id: Uuid::nil(),
        upstream_override: Some("https://proxy.example/v1".to_string()),
    };
    assert_eq!(
        upstream_url(&proxy, &call, OPUS_PATH, None),
        format!("https://proxy.example/v1/{OPUS_PATH}")
    );
}

// ── Invariants 3, 5 and 6: the relay over the wire ─────────────────────────

/// One request the fake upstream received.
#[derive(Clone, Debug)]
struct Seen {
    path_and_query: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

/// A fake Vertex on loopback that records every request and answers with
/// `status` and `body`.
async fn fake_upstream(status: StatusCode, body: &'static str) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    let app = Router::new().fallback(move |req: axum::extract::Request| {
        let record = record.clone();
        async move {
            let (parts, body_in) = req.into_parts();
            let bytes = axum::body::to_bytes(body_in, usize::MAX).await.unwrap();
            record.lock().unwrap().push(Seen {
                path_and_query: parts.uri.path_and_query().unwrap().to_string(),
                headers: parts.headers,
                body: bytes.to_vec(),
            });
            (status, body)
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, seen)
}

async fn test_relay() -> u16 {
    bind_and_serve(reqwest::Client::builder().no_proxy())
        .await
        .expect("relay binds")
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

#[tokio::test]
async fn a_bad_token_is_refused_before_the_upstream_sees_anything() {
    install_secret();
    let (_base, seen) = fake_upstream(StatusCode::OK, "{}").await;
    let port = test_relay().await;
    let response = client()
        .post(format!(
            "http://127.0.0.1:{port}{ROUTE_PREFIX}/forged.-.00/{OPUS_PATH}"
        ))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_foreign_path_is_refused_before_the_upstream_sees_anything() {
    install_secret();
    let (base, seen) = fake_upstream(StatusCode::OK, "{}").await;
    let port = test_relay().await;
    let url = relay_base_url(port, Uuid::new_v4(), Some(&base)).unwrap();
    let response = client()
        .post(format!("{url}/private/tmp/somewhere"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_call_is_forwarded_with_notes_asked_for_and_credentials_intact() {
    install_secret();
    let (base, seen) = fake_upstream(StatusCode::OK, r#"{"ok":true}"#).await;
    let port = test_relay().await;
    let url = relay_base_url(port, Uuid::new_v4(), Some(&base)).unwrap();
    let response = client()
        .post(format!("{url}/{OPUS_PATH}?alt=sse"))
        .header("authorization", "Bearer ya29.x")
        .header("anthropic-beta", "claude-code-20250219")
        .body(r#"{"thinking":{"type":"adaptive"},"messages":[ 1 ]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text().await.unwrap(), r#"{"ok":true}"#);

    let seen = seen.lock().unwrap();
    let request = &seen[0];
    assert_eq!(request.path_and_query, format!("/{OPUS_PATH}?alt=sse"));
    assert_eq!(request.headers["authorization"], "Bearer ya29.x");
    assert_eq!(
        request.headers["anthropic-beta"],
        format!("claude-code-20250219,{ANTHROPIC_BETA_THINKING_DISPLAY_UPDATES}")
    );
    let fields = raw_fields(&request.body);
    assert_eq!(fields["messages"], "[ 1 ]");
    assert!(fields["thinking"].contains(r#""display":"updates""#));
}

#[tokio::test]
async fn an_upstream_error_reaches_the_caller_unchanged() {
    install_secret();
    let (base, _seen) = fake_upstream(StatusCode::TOO_MANY_REQUESTS, r#"{"error":"quota"}"#).await;
    let port = test_relay().await;
    let url = relay_base_url(port, Uuid::new_v4(), Some(&base)).unwrap();
    let response = client()
        .post(format!("{url}/{OPUS_PATH}"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.text().await.unwrap(), r#"{"error":"quota"}"#);
}

#[tokio::test]
async fn an_unreachable_upstream_is_a_loud_502() {
    install_secret();
    // Bound and dropped, so nothing listens on it.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let port = test_relay().await;
    let url = relay_base_url(port, Uuid::new_v4(), Some(&closed)).unwrap();
    let response = client()
        .post(format!("{url}/{OPUS_PATH}"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("Lucidos Vertex relay:"));
}

/// The upstream holds its second chunk until the test reads the first. A
/// relay that buffered the stream would deadlock here instead of passing.
#[tokio::test]
async fn each_chunk_streams_through_as_it_arrives() {
    install_secret();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let release_rx = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
    let app = Router::new().fallback(move || {
        let release_rx = release_rx.clone();
        async move {
            let rx = release_rx.lock().await.take().expect("one request");
            let chunks = futures::stream::unfold(Some((0u8, rx)), |state| async move {
                match state {
                    Some((0, rx)) => Some((Ok::<_, std::io::Error>("first\n"), Some((1, rx)))),
                    Some((_, rx)) => {
                        rx.await.ok()?;
                        Some((Ok("second\n"), None))
                    }
                    None => None,
                }
            });
            Body::from_stream(chunks)
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let port = test_relay().await;
    let url = relay_base_url(port, Uuid::new_v4(), Some(&base)).unwrap();
    let mut response = client()
        .post(format!("{url}/{OPUS_PATH}"))
        .body("{}")
        .send()
        .await
        .unwrap();
    let first = tokio::time::timeout(std::time::Duration::from_secs(10), response.chunk())
        .await
        .expect("the first chunk arrives before the second is sent")
        .unwrap()
        .unwrap();
    assert_eq!(&first[..], b"first\n");
    release_tx.send(()).unwrap();
    let second = response.chunk().await.unwrap().unwrap();
    assert_eq!(&second[..], b"second\n");
}
