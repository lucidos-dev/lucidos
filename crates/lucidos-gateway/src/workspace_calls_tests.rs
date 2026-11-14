//! A workspace call: only a calling engine's own signature passes, and the
//! target hears the same claim under its own key.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::HeaderMap;
use tower::ServiceExt as _;

use super::*;
use crate::engine_hop::test_engine;
use crate::registry::Workspace;

const CALLER: &str = "dev";
const TARGET: &str = "work2";
const THREAD: &str = "6b3f7a2e-0000-4000-8000-000000000001";
const VERDICT: &str = "refused:no%20grant";

fn signed_headers(key: &StampKey, target: &str, verdict: &str) -> HeaderMap {
    let stamp = key.sign_now(Claim::WorkspaceCall {
        target_workspace: target,
        caller_workspace: CALLER,
        caller_thread_id: THREAD,
        verdict,
    });
    let mut headers = HeaderMap::new();
    for (name, value) in [
        (HEADER_CALLER_WORKSPACE, CALLER),
        (HEADER_CALLER_THREAD, THREAD),
        (HEADER_CALLER_VERDICT, VERDICT),
        (HEADER_GATEWAY_STAMP, stamp.as_str()),
    ] {
        headers.insert(name, value.parse().unwrap());
    }
    headers
}

fn keyed(key: &Arc<StampKey>) -> impl FnOnce(&str) -> Option<Arc<StampKey>> + '_ {
    move |id| (id == CALLER).then(|| key.clone())
}

#[test]
fn a_call_the_caller_signed_verifies() {
    let key = Arc::new(StampKey::generate().unwrap());
    let headers = signed_headers(&key, TARGET, VERDICT);
    assert_eq!(
        verified_call(TARGET, &headers, keyed(&key)),
        Some(AttestedWorkspaceCall {
            caller_workspace: CALLER.into(),
            caller_thread_id: THREAD.into(),
            verdict: VERDICT.into(),
        })
    );
}

#[test]
fn a_call_for_another_target_or_verdict_or_key_does_not_verify() {
    let key = Arc::new(StampKey::generate().unwrap());
    let for_another_target = signed_headers(&key, "work3", VERDICT);
    assert_eq!(
        verified_call(TARGET, &for_another_target, keyed(&key)),
        None
    );
    let mut upgraded = signed_headers(&key, TARGET, VERDICT);
    upgraded.insert(HEADER_CALLER_VERDICT, "allowed".parse().unwrap());
    assert_eq!(verified_call(TARGET, &upgraded, keyed(&key)), None);
    let other = Arc::new(StampKey::generate().unwrap());
    let headers = signed_headers(&key, TARGET, VERDICT);
    assert_eq!(verified_call(TARGET, &headers, keyed(&other)), None);
    assert_eq!(verified_call(TARGET, &headers, |_| None), None);
}

/// The proxy stamps the verified claim for the target, under the target's key.
#[test]
fn the_forwarded_claim_is_stamped_for_the_target() {
    let call = AttestedWorkspaceCall {
        caller_workspace: CALLER.into(),
        caller_thread_id: THREAD.into(),
        verdict: VERDICT.into(),
    };
    let target_key = StampKey::generate().unwrap();
    let headers = call.forwarded_headers(TARGET, Some(&target_key));
    let stamp = &headers
        .iter()
        .find(|(name, _)| *name == HEADER_GATEWAY_STAMP)
        .expect("a stamp")
        .1;
    assert!(target_key.verify_now(stamp, call.claim(TARGET)));
    assert!(!call
        .forwarded_headers(TARGET, None)
        .iter()
        .any(|(name, _)| *name == HEADER_GATEWAY_STAMP));
}

type Seen = Arc<Mutex<Option<(HeaderMap, String)>>>;

/// A gateway serving `CALLER` and `TARGET`, both keyed, with `TARGET`'s
/// engine recording what reaches it. The target's directory is `target_dir`.
fn gateway(
    target_dir: &str,
) -> (
    GatewayState,
    Arc<StampKey>,
    StampKey,
    Seen,
    tempfile::TempDir,
) {
    let state = GatewayState::for_tests();
    for (id, dir) in [(CALLER, "/tmp/lucidos-call-test/dev"), (TARGET, target_dir)] {
        state.register_for_test(Workspace {
            id: id.into(),
            name: id.into(),
            dir: dir.into(),
            port: test_engine::TEST_PORT,
            database_url: None,
            cleanup: Default::default(),
            autostart: false,
        });
    }
    let caller_key = StampKey::generate().unwrap();
    let caller_copy =
        Arc::new(StampKey::read_handover(caller_key.handover_line().as_bytes()).unwrap());
    state.set_stamp_key_for_test(CALLER, caller_key);
    let target_key = StampKey::generate().unwrap();
    state.set_stamp_key_for_test(
        TARGET,
        StampKey::read_handover(target_key.handover_line().as_bytes()).unwrap(),
    );
    let seen: Seen = Default::default();
    let record = seen.clone();
    let app = axum::Router::new().fallback(move |headers: HeaderMap, body: String| {
        let record = record.clone();
        async move {
            *record.lock().unwrap() = Some((headers, body));
            axum::Json(serde_json::json!({ "thread_id": "spawned" }))
        }
    });
    let dir = test_engine::scratch();
    state.set_route_for_test(TARGET, test_engine::serve(&dir, app));
    (state, caller_copy, target_key, seen, dir)
}

async fn call(
    state: &GatewayState,
    name: &str,
    mut headers: HeaderMap,
) -> axum::response::Response {
    headers.insert(
        crate::auth::HEADER_LOCAL_TOKEN,
        state.local_token().parse().unwrap(),
    );
    let mut req = Request::post(format!("/~/api/v1/workspace-calls/{name}/chat/stream"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"message":"hi"}"#))
        .unwrap();
    req.headers_mut().extend(headers);
    crate::server::gateway_router(state.clone())
        .oneshot(req)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_signed_call_reaches_the_target_stamped_under_its_key() {
    let (state, caller_key, target_key, seen, _dir) = gateway("/tmp/lucidos-call-test/work2");
    let response = call(&state, TARGET, signed_headers(&caller_key, TARGET, VERDICT)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let (headers, body) = seen.lock().unwrap().clone().expect("the target heard it");
    assert_eq!(body, r#"{"message":"hi"}"#);
    let stamp = headers[HEADER_GATEWAY_STAMP].to_str().unwrap();
    let claim = Claim::WorkspaceCall {
        target_workspace: TARGET,
        caller_workspace: CALLER,
        caller_thread_id: THREAD,
        verdict: VERDICT,
    };
    assert!(target_key.verify_now(stamp, claim), "{stamp}");
    assert!(
        !caller_key.verify_now(stamp, claim),
        "the caller's own stamp never reaches B"
    );
}

/// A caller knows its target by directory name, which may differ from the id.
#[tokio::test]
async fn a_target_named_by_its_directory_resolves_to_its_id() {
    let (state, caller_key, target_key, seen, _dir) = gateway("/tmp/lucidos-call-test/second-ws");
    let response = call(
        &state,
        "second-ws",
        signed_headers(&caller_key, "second-ws", VERDICT),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let (headers, _) = seen.lock().unwrap().clone().expect("the target heard it");
    let stamp = headers[HEADER_GATEWAY_STAMP].to_str().unwrap();
    assert!(target_key.verify_now(
        stamp,
        Claim::WorkspaceCall {
            target_workspace: TARGET,
            caller_workspace: CALLER,
            caller_thread_id: THREAD,
            verdict: VERDICT,
        }
    ));
}

#[tokio::test]
async fn a_forged_call_is_refused_before_it_reaches_the_target() {
    let (state, _, _, seen, _dir) = gateway("/tmp/lucidos-call-test/work2");
    let forged = StampKey::generate().unwrap();
    let response = call(&state, TARGET, signed_headers(&forged, TARGET, VERDICT)).await;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(seen.lock().unwrap().is_none());
}

#[tokio::test]
async fn a_target_this_gateway_does_not_serve_hands_the_call_back() {
    let (state, caller_key, _, seen, _dir) = gateway("/tmp/lucidos-call-test/work2");
    let response = call(
        &state,
        "elsewhere",
        signed_headers(&caller_key, "elsewhere", VERDICT),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["reason"], REASON_WORKSPACE_NOT_ON_THIS_GATEWAY);
    assert!(seen.lock().unwrap().is_none());
}

/// A paired device is not an engine: it holds no stamp key to sign with.
#[tokio::test]
async fn only_a_local_process_may_place_a_call() {
    let (state, caller_key, _, seen, _dir) = gateway("/tmp/lucidos-call-test/work2");
    let mut req = Request::post(format!("/~/api/v1/workspace-calls/{TARGET}/chat/stream"))
        .body(Body::from("{}"))
        .unwrap();
    req.headers_mut()
        .extend(signed_headers(&caller_key, TARGET, VERDICT));
    let response = crate::server::gateway_router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert!(response.status().is_client_error(), "{}", response.status());
    assert!(seen.lock().unwrap().is_none());
}

/// A call relayed from a paired gateway verifies only under that pairing's
/// credential, for its own fields, within the relay's clock skew.
#[test]
fn a_relayed_call_verifies_only_as_signed() {
    let call = AttestedWorkspaceCall {
        caller_workspace: CALLER.into(),
        caller_thread_id: THREAD.into(),
        verdict: VERDICT.into(),
    };
    let now = 1_000_000;
    let headers = |credential: &str, target: &str, at: i64| {
        let mut headers = HeaderMap::new();
        for (name, value) in call.relayed_headers(target, credential, at) {
            headers.insert(name, value.parse().unwrap());
        }
        headers
    };
    let good = headers("cred-1", TARGET, now);
    assert_eq!(
        verified_relayed_call(TARGET, &good, "cred-1", now + 300),
        Some(call.clone())
    );
    assert_eq!(verified_relayed_call(TARGET, &good, "cred-2", now), None);
    assert_eq!(verified_relayed_call("work3", &good, "cred-1", now), None);
    assert_eq!(
        verified_relayed_call(TARGET, &good, "cred-1", now + 301),
        None
    );
    let mut upgraded = good.clone();
    upgraded.insert(HEADER_CALLER_VERDICT, "allowed".parse().unwrap());
    assert_eq!(
        verified_relayed_call(TARGET, &upgraded, "cred-1", now),
        None
    );
    let mut no_signature = good.clone();
    no_signature.remove(HEADER_RELAYED_CALL_SIGNATURE);
    assert_eq!(
        verified_relayed_call(TARGET, &no_signature, "cred-1", now),
        None
    );
    for malformed in ["", "1000000", "x.00", "1000000.00.00"] {
        let mut bad = good.clone();
        bad.insert(HEADER_RELAYED_CALL_SIGNATURE, malformed.parse().unwrap());
        assert_eq!(
            verified_relayed_call(TARGET, &bad, "cred-1", now),
            None,
            "{malformed:?}"
        );
    }
}

/// Gateway A pairs with gateway B, and B serves the target `dev`.
mod across_gateways {
    use super::*;
    use crate::relay::tests::{paired, Paired};

    const REMOTE: &str = "dev";

    /// `p` with B registering `dev` under a stamp key, and A holding the
    /// calling engine's key. Returns the caller's and B's engine's keys.
    fn keyed(p: &Paired) -> (StampKey, StampKey) {
        p.b.register_for_test(Workspace {
            id: REMOTE.into(),
            name: REMOTE.into(),
            dir: "/tmp/lucidos-call-test/remote/dev".into(),
            port: test_engine::TEST_PORT,
            database_url: None,
            cleanup: Default::default(),
            autostart: false,
        });
        let copy =
            |key: &StampKey| StampKey::read_handover(key.handover_line().as_bytes()).unwrap();
        let caller = StampKey::generate().unwrap();
        p.a.set_stamp_key_for_test(CALLER, copy(&caller));
        let engine = StampKey::generate().unwrap();
        p.b.set_stamp_key_for_test(REMOTE, copy(&engine));
        (caller, engine)
    }

    /// What B's engine heard on its chat route, one lowercase line per header.
    fn heard(p: &Paired) -> Option<String> {
        p.seen
            .lock()
            .unwrap()
            .iter()
            .find(|line| line.starts_with("post /api/v1/chat/stream"))
            .cloned()
    }

    fn header_in<'a>(line: &'a str, name: &str) -> Option<&'a str> {
        line.lines()
            .find_map(|l| l.strip_prefix(&format!("{name}: ")))
    }

    #[tokio::test]
    async fn a_call_for_a_paired_gateways_workspace_reaches_it_stamped_under_its_key() {
        let p = paired().await;
        let (caller, engine) = keyed(&p);
        let response = call(&p.a, REMOTE, signed_headers(&caller, REMOTE, VERDICT)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let line = heard(&p).expect("B's engine heard the call");
        let stamp = header_in(&line, HEADER_GATEWAY_STAMP).expect("a stamp");
        let claim = Claim::WorkspaceCall {
            target_workspace: REMOTE,
            caller_workspace: CALLER,
            caller_thread_id: THREAD,
            verdict: VERDICT,
        };
        assert!(engine.verify_now(stamp, claim), "{line}");
        assert!(!caller.verify_now(stamp, claim));
        assert_eq!(header_in(&line, HEADER_CALLER_VERDICT), Some(VERDICT));
        // An engine's own call, so no restore generation fences it.
        assert_eq!(
            header_in(&line, crate::proxy::HEADER_CALLER_KIND),
            Some("local-process")
        );
        for gone in [
            crate::gateway_protocol::HEADER_GATEWAY_CREDENTIAL,
            HEADER_RELAYED_CALL_SIGNATURE,
        ] {
            assert!(
                header_in(&line, gone).is_none(),
                "{gone} reached B's engine"
            );
        }
    }

    #[tokio::test]
    async fn a_target_no_paired_gateway_serves_hands_the_call_back() {
        let p = paired().await;
        let (caller, _) = keyed(&p);
        let response = call(
            &p.a,
            "elsewhere",
            signed_headers(&caller, "elsewhere", VERDICT),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["reason"], REASON_WORKSPACE_NOT_ON_THIS_GATEWAY);
        assert!(heard(&p).is_none());
    }

    /// B's half, called directly with the pairing's credential, as A's gateway calls it.
    async fn to_b(
        p: &Paired,
        method: reqwest::Method,
        path: &str,
        signature: Option<String>,
    ) -> reqwest::StatusCode {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();
        let mut request = client
            .request(method, format!("{}{path}", p.b_address))
            .header("x-proxy-token", "proxy-secret")
            .header(
                crate::gateway_protocol::HEADER_GATEWAY_CREDENTIAL,
                &p.credential,
            )
            .header(HEADER_CALLER_WORKSPACE, CALLER)
            .header(HEADER_CALLER_THREAD, THREAD)
            .header(HEADER_CALLER_VERDICT, "allowed");
        if let Some(signature) = signature {
            request = request.header(HEADER_RELAYED_CALL_SIGNATURE, signature);
        }
        request.body("{}").send().await.unwrap().status()
    }

    fn relayed_path(target: &str) -> String {
        format!(
            "{}{}",
            crate::gateway_protocol::GATEWAY_API_PREFIX,
            lucidos_gateway_stamp::workspace_call_path(target).unwrap()
        )
    }

    #[tokio::test]
    async fn b_refuses_a_relayed_call_its_pairing_did_not_sign() {
        let p = paired().await;
        keyed(&p);
        let forged = AttestedWorkspaceCall {
            caller_workspace: CALLER.into(),
            caller_thread_id: THREAD.into(),
            verdict: "allowed".into(),
        }
        .relayed_headers(REMOTE, "not-the-credential", chrono::Utc::now().timestamp())
        .into_iter()
        .find(|(name, _)| *name == HEADER_RELAYED_CALL_SIGNATURE)
        .map(|(_, value)| value);
        for signature in [None, forged] {
            let status = to_b(&p, reqwest::Method::POST, &relayed_path(REMOTE), signature).await;
            assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
        }
        assert!(heard(&p).is_none());
    }

    #[tokio::test]
    async fn b_says_which_workspaces_it_takes_calls_for() {
        let p = paired().await;
        keyed(&p);
        let probe = |target: &str| {
            format!(
                "{}/workspace-calls/{target}",
                crate::gateway_protocol::GATEWAY_API_PREFIX
            )
        };
        let served = to_b(&p, reqwest::Method::GET, &probe(REMOTE), None).await;
        assert_eq!(served, reqwest::StatusCode::NO_CONTENT);
        let unserved = to_b(&p, reqwest::Method::GET, &probe("elsewhere"), None).await;
        assert_eq!(unserved, reqwest::StatusCode::NOT_FOUND);
    }
}
