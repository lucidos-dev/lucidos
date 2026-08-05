//! Integration test: a curl-equivalent POST from a Lucidos-spawned
//! subprocess of thread A targeting thread B must land as
//! `MessageOrigin::Api { mode: Agent, source_thread_id: Some(A) }`, never
//! as a `Device` or `Api { mode: Human }`. Locks in the actor-stamping
//! side of the cross-thread agent-impersonation fix. See `api::actor`
//! module docs for the full design.

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use lucidos_engine::api::actor::{
    build_message_origin, init_agent_origin_secret, mint_agent_origin_token, subprocess_origin,
    SubprocessOrigin, HEADER_AGENT_ORIGIN_TOKEN, HEADER_DEVICE_ID,
};
use lucidos_engine::engine::thread_events::{ActorMode, MessageOrigin};
use uuid::Uuid;

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.insert(
            HeaderName::from_bytes(k.as_bytes()).unwrap(),
            HeaderValue::from_str(v).unwrap(),
        );
    }
    h
}

/// Mint a thread-bound origin token for the integration test run, exactly
/// as a spawn would. `OnceLock::set` inside `init_agent_origin_secret`
/// makes the first writer win, so re-running this in the same process
/// signs under whatever secret the lock already holds.
fn token_for(thread_id: Option<Uuid>) -> String {
    init_agent_origin_secret("subprocess-integration-secret".to_string());
    mint_agent_origin_token(thread_id).expect("secret installed at least once")
}

/// Reproduce the exact request shape from the in-the-wild incident: a
/// subprocess of thread A `curl`s into a different thread's mutating
/// endpoint. The resulting actor MUST stamp as `Api { mode: Agent,
/// source_thread_id }` — not as a `Device` (would render "You") and not
/// as `Api { mode: Human }` (also renders "You").
#[test]
fn subprocess_curl_into_different_thread_stamps_agent_api_actor() {
    let source_thread = Uuid::new_v4(); // thread A — the agent's own thread
    let token = token_for(Some(source_thread));
    let h = headers(&[
        ("user-agent", "curl/8.7.1"),
        (HEADER_AGENT_ORIGIN_TOKEN, &token),
    ]);

    // The request body would carry mode=Human (curl's default) — but the
    // mutating endpoint handlers call `user_actor_resolved` which goes
    // through `build_message_origin(.. ActorMode::Human, .. None caller)`
    // for this shape. The subprocess override fires inside it.
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        None, // no device-id explicit override
        None, // no device label
        None, // no parent-thread
        None,
        None,
        None, // no caller_workspace
    );

    match origin {
        Some(MessageOrigin::Api {
            user_agent,
            mode,
            source_thread_id,
        }) => {
            assert_eq!(user_agent.as_deref(), Some("curl/8.7.1"));
            assert_eq!(
                mode,
                ActorMode::Agent,
                "subprocess curl MUST stamp as Agent mode so the UI \
                 doesn't render an agent's mutating action as a 'You' card"
            );
            assert_eq!(
                source_thread_id,
                Some(source_thread),
                "spawning thread id MUST be recorded so the popover can \
                 attribute back to the agent's source thread"
            );
        }
        other => panic!(
            "expected Api {{ mode: Agent, source_thread_id: Some(_) }}, got {:?}",
            other
        ),
    }
}

/// A subprocess request that also presents a device-id header (the agent
/// could read its env and impersonate further) still resolves to
/// `Api { mode: Agent }`. The subprocess origin is the strongest signal —
/// it beats device-id resolution.
#[test]
fn subprocess_token_beats_device_id_header() {
    let token = token_for(Some(Uuid::new_v4()));
    let h = headers(&[
        ("user-agent", "curl/8.7.1"),
        (HEADER_DEVICE_ID, "some-real-device-id"),
        (HEADER_AGENT_ORIGIN_TOKEN, &token),
    ]);

    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        Some("some-real-device-id"),
        Some("Chrome on Mac".into()),
        None,
        None,
        None,
        None,
    );

    assert!(
        matches!(
            origin,
            Some(MessageOrigin::Api {
                mode: ActorMode::Agent,
                ..
            })
        ),
        "subprocess origin must beat a presented device-id, got {:?}",
        origin
    );
}

/// A regular external `curl` without the subprocess token resolves the way
/// it always has — `Api { mode: Human }`. The detection mechanism only
/// fires when the per-engine token matches; bare external API clients are
/// untouched. (Critical for back-compat with non-subprocess HTTP callers.)
#[test]
fn external_curl_without_token_resolves_to_human_api_as_before() {
    token_for(None);
    let h = headers(&[("user-agent", "curl/8.7.1")]);

    let origin = build_message_origin(&h, ActorMode::Human, None, None, None, None, None, None);

    match origin {
        Some(MessageOrigin::Api {
            mode,
            source_thread_id,
            ..
        }) => {
            assert_eq!(
                mode,
                ActorMode::Human,
                "external curl without the subprocess token MUST stay Human \
                 — the detection mechanism must not poison non-subprocess paths"
            );
            assert_eq!(source_thread_id, None);
        }
        other => panic!("expected Api {{ mode: Human }}, got {:?}", other),
    }
}

/// Forged token (the engine secret rotates per startup; an attacker outside
/// the process can't know it). Resolves on the external-curl path.
#[test]
fn wrong_token_is_not_recognised_as_subprocess() {
    token_for(None);
    let h = headers(&[(
        HEADER_AGENT_ORIGIN_TOKEN,
        &format!("{}.definitely-not-the-real-mac", Uuid::new_v4()),
    )]);
    assert_eq!(subprocess_origin(&h), SubprocessOrigin::NotSubprocess);
}

/// The end-to-end shape of the thread binding, at the integration layer:
/// a subprocess of thread A that re-points its own token at thread B is
/// not a subprocess at all, so `build_message_origin` falls through to the
/// external-caller path instead of stamping B.
#[test]
fn a_subprocess_cannot_stamp_a_thread_its_token_was_not_minted_for() {
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();
    let token = token_for(Some(mine));
    let mac = token.rsplit_once('.').expect("minted token has a mac").1;
    let h = headers(&[
        ("user-agent", "curl/8.7.1"),
        (HEADER_AGENT_ORIGIN_TOKEN, &format!("{theirs}.{mac}")),
    ]);

    assert_eq!(subprocess_origin(&h), SubprocessOrigin::NotSubprocess);
    let origin = build_message_origin(&h, ActorMode::Human, None, None, None, None, None, None);
    match origin {
        Some(MessageOrigin::Api {
            mode,
            source_thread_id,
            ..
        }) => {
            assert_eq!(mode, ActorMode::Human, "a forger is an external caller");
            assert_eq!(
                source_thread_id, None,
                "no thread may be stamped from a token that does not cover it"
            );
        }
        other => panic!("expected Api {{ mode: Human }}, got {:?}", other),
    }
}

/// JSON round-trip on the new `source_thread_id` field. Persisted event
/// payloads serialize this through serde_json — make sure the wire shape
/// matches the frontend's TS mirror (`MessageOrigin` in
/// `crates/lucidos-app/src/store/thread-events.ts`).
#[test]
fn api_origin_with_source_thread_id_round_trips_as_json() {
    let source = Uuid::new_v4();
    let origin = MessageOrigin::Api {
        user_agent: Some("curl/8.7.1".into()),
        mode: ActorMode::Agent,
        source_thread_id: Some(source),
    };
    let json = serde_json::to_value(&origin).unwrap();
    assert_eq!(json["kind"], "api");
    assert_eq!(json["mode"], "agent");
    assert_eq!(json["user_agent"], "curl/8.7.1");
    assert_eq!(json["source_thread_id"], source.to_string());

    let back: MessageOrigin = serde_json::from_value(json).unwrap();
    assert_eq!(back, origin);
}

/// Legacy DB rows (persisted before the field existed) deserialize cleanly
/// — `source_thread_id` defaults to `None`, and the new field is omitted
/// from serialization when None (so we don't bloat every existing row).
#[test]
fn api_origin_legacy_rows_deserialize_without_source_thread_id() {
    let legacy_json = serde_json::json!({
        "kind": "api",
        "user_agent": "Mozilla/5.0",
        "mode": "human",
    });
    let origin: MessageOrigin = serde_json::from_value(legacy_json).unwrap();
    match origin {
        MessageOrigin::Api {
            source_thread_id, ..
        } => assert_eq!(source_thread_id, None),
        other => panic!("expected Api, got {:?}", other),
    }

    // And the reverse: a freshly constructed Api{source_thread_id: None}
    // must NOT emit the field, to keep persisted-event payloads compact.
    let origin = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Human,
        source_thread_id: None,
    };
    let json = serde_json::to_value(&origin).unwrap();
    assert!(
        json.get("source_thread_id").is_none(),
        "absent source_thread_id must not serialize (back-compat)"
    );
}
