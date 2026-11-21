use super::*;
use crate::engine_hop::test_engine;
use crate::gateway_protocol::{GatewayRedemption, GATEWAY_PROTOCOL};
use crate::pairing::{Minter, OwnerGate};
use crate::registry::Workspace;
use crate::return_access::ReturnRoute;
use lucidos_gateway_stamp::remote_start::{
    RemotePairing, RemoteStart, RemoteStartSetting, HEADER_REMOTE_START,
};
use lucidos_gateway_stamp::workspace_operation::{
    receive_route, ReadRequest, RoutingRequest, RECEIVE_ROUTE, RECEIVE_START_ROUTE,
};
use lucidos_gateway_stamp::StampKey;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

fn source(state: &GatewayState) -> Arc<StampKey> {
    let key = StampKey::generate().unwrap();
    let copy = Arc::new(StampKey::read_handover(key.handover_line().as_bytes()).unwrap());
    state.set_stamp_key_for_test("source", key);
    copy
}

fn signed(key: &StampKey, via: Option<&str>, to: Option<&str>) -> SignedOperation {
    sign(
        key,
        via,
        to,
        WorkspaceOperation::Read {
            request: ReadRequest {
                kind: if to.is_some() {
                    ReadKind::ThreadMessages
                } else {
                    ReadKind::WorkspaceTargets
                },
                thread_id: to.map(|_| "thread-1".to_owned()),
                query: Default::default(),
            },
        },
    )
}

fn signed_spawn(key: &StampKey, to: &str) -> SignedOperation {
    let body =
        json!({"message": "Start a task", "thread_id": "00000000-0000-4000-8000-000000000009"});
    sign(
        key,
        None,
        Some(to),
        WorkspaceOperation::Spawn {
            body: body.as_object().unwrap().clone(),
        },
    )
}

fn sign(
    key: &StampKey,
    via: Option<&str>,
    to: Option<&str>,
    operation: WorkspaceOperation,
) -> SignedOperation {
    let envelope = OperationEnvelope {
        request: RoutingRequest {
            via: via.map(str::to_owned),
            to: to.map(str::to_owned),
            operation,
        },
        caller_gateway: String::new(),
        caller_workspace: "source".into(),
        caller_thread_id: "00000000-0000-0000-0000-000000000001".into(),
        verdict: String::new(),
        nonce: crate::auth::mint_credential().unwrap(),
    };
    let digest = envelope.digest().unwrap();
    let stamp = key.sign_now(Claim::WorkspaceOperation(envelope.claim(&digest)));
    SignedOperation { envelope, stamp }
}

async fn call(state: &GatewayState, signed: &SignedOperation, local: bool) -> Response {
    let mut request = Request::post("/~/api/v1/workspace-operations")
        .header(header::CONTENT_TYPE, "application/json");
    if local {
        request = request.header(crate::auth::HEADER_LOCAL_TOKEN, state.local_token());
    }
    crate::server::gateway_router(state.clone())
        .oneshot(
            request
                .body(Body::from(serde_json::to_vec(signed).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn assert_ok(response: Response) {
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert!(
        status.is_success(),
        "{status}: {}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn local_operations_require_the_source_key_and_reject_tampering_and_replay() {
    let dir = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(dir.path().to_path_buf()));
    let key = source(&state);
    let request = signed(&key, None, None);
    assert!(call(&state, &request, false)
        .await
        .status()
        .is_client_error());
    let mut tampered = request.clone();
    tampered.envelope.caller_thread_id = "another-thread".into();
    assert!(call(&state, &tampered, true)
        .await
        .status()
        .is_client_error());
    assert_ok(call(&state, &request, true).await).await;
    assert!(call(&state, &request, true)
        .await
        .status()
        .is_client_error());
    let forged = signed(&StampKey::generate().unwrap(), None, None);
    assert!(call(&state, &forged, true).await.status().is_client_error());
}

async fn listen(state: &GatewayState) -> String {
    listen_router(crate::server::gateway_router(state.clone())).await
}

async fn listen_router(app: Router) -> String {
    let port = lucidos_local_token::test_support::serve_https(app).port();
    lucidos_local_token::client::origin(port)
}

fn isolated() -> (GatewayState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    (
        GatewayState::for_tests_with_static_dir(Some(dir.path().to_path_buf())),
        dir,
    )
}

fn grant_return(a: &GatewayState, b: &GatewayState, pairing: &RemoteGateway, a_url: String) {
    let granted = a
        .remote_gateways()
        .set_return_access("b", true)
        .unwrap()
        .unwrap();
    b.paired_gateways()
        .set_return_route(
            &pairing.pairing_id,
            Some(ReturnRoute {
                grant_id: granted.return_access.unwrap().id,
                gateway_id: a.gateway_id().into(),
                address: a_url,
            }),
        )
        .unwrap();
}

fn pair(a: &GatewayState, b: &GatewayState, address: &str, name: &str) -> RemoteGateway {
    let secret = b
        .paired_gateways()
        .mint_secret(Some("local".into()), false, Minter::PairedDevice)
        .unwrap();
    let GatewayRedemption::Paired {
        pairing_id,
        credential,
    } = b
        .paired_gateways()
        .redeem(&secret, None, OwnerGate::NoDevicePaired)
        .unwrap()
    else {
        panic!()
    };
    let remote = RemoteGateway {
        name: name.into(),
        address: address.into(),
        pairing_id,
        credential,
        gateway_id: Some(b.gateway_id().into()),
        headers: Default::default(),
        gateway_protocol: GATEWAY_PROTOCOL,
        added_at: "2020-01-01T00:00:00Z".into(),
        rotation: Default::default(),
        return_access: None,
    };
    a.remote_gateways().add(remote.clone()).unwrap();
    remote
}

type Seen = Arc<Mutex<Vec<SignedOperation>>>;
/// The verified remote start header of each operation the target received.
type Starts = Arc<Mutex<Vec<Option<RemoteStart>>>>;

fn target(state: &GatewayState) -> (Seen, tempfile::TempDir) {
    let (seen, _, dir) = target_with_starts(state);
    (seen, dir)
}

fn target_with_starts(state: &GatewayState) -> (Seen, Starts, tempfile::TempDir) {
    engine_serving(state, &[RECEIVE_ROUTE, RECEIVE_START_ROUTE])
}

/// A target engine that serves only `routes`. An engine that predates the
/// remote start setting serves only [`RECEIVE_ROUTE`].
fn engine_serving(
    state: &GatewayState,
    routes: &[&'static str],
) -> (Seen, Starts, tempfile::TempDir) {
    let dir = test_engine::scratch();
    state.register_for_test(Workspace {
        id: "target".into(),
        name: "Target workspace".into(),
        dir: dir.path().join("target").to_string_lossy().into_owned(),
        port: test_engine::TEST_PORT,
        database_url: None,
        cleanup: Default::default(),
        autostart: false,
    });
    state.set_stamp_key_for_test("target", StampKey::generate().unwrap());
    let key = state.stamp_key("target").unwrap();
    let seen: Seen = Default::default();
    let starts: Starts = Default::default();
    let mut app = Router::new();
    for &route in routes {
        let (record, record_start, key) = (seen.clone(), starts.clone(), key.clone());
        app = app.route(
            &format!("/api/v1{route}"),
            post(
                move |headers: HeaderMap, Json(signed): Json<SignedOperation>| async move {
                    assert_eq!(
                        receive_route(signed.envelope.request.operation.kind()),
                        route,
                        "each operation kind has its own route"
                    );
                    let digest = signed.envelope.digest().unwrap();
                    assert!(key.verify_now(
                        &signed.stamp,
                        Claim::WorkspaceOperation(signed.envelope.claim(&digest))
                    ));
                    let start = headers.get(HEADER_REMOTE_START).map(|value| {
                        RemoteStart::from_header(
                            value.to_str().unwrap(),
                            &key,
                            "target",
                            &signed.envelope.nonce,
                            lucidos_gateway_stamp::now_unix(),
                        )
                        .expect("a remote start header the target can verify")
                    });
                    record_start.lock().unwrap().push(start);
                    record.lock().unwrap().push(signed);
                    (
                        [(
                            lucidos_gateway_stamp::workspace_operation::PROTOCOL_HEADER,
                            lucidos_gateway_stamp::workspace_operation::PROTOCOL_VERSION,
                        )],
                        Json(json!({"messages":["actual target reply"]})),
                    )
                },
            ),
        );
    }
    state.set_route_for_test("target", test_engine::serve(&dir, app));
    (seen, starts, dir)
}

async fn body_text(response: Response) -> String {
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&body).into_owned()
}

#[tokio::test]
async fn a_peer_spawn_obeys_the_receiving_pairing_setting_and_reads_never_do() {
    let (a, _a_dir) = isolated();
    let (c, _c_dir) = isolated();
    let c_url = listen(&c).await;
    let ac = pair(&a, &c, &c_url, "c");
    let a_key = source(&a);
    let (seen, starts, _target_dir) = target_with_starts(&c);
    let pairing = Some(RemotePairing {
        id: ac.pairing_id.clone(),
        label: "local".into(),
    });

    // A new pairing starts at Deny: the spawn never reaches the engine.
    let denied = call(&a, &signed_spawn(&a_key, "target@c"), true).await;
    assert!(denied.status().is_client_error(), "{}", denied.status());
    let denied = body_text(denied).await;
    assert!(
        denied.contains("Settings → Access → Pair a gateway"),
        "{denied}"
    );
    assert!(seen.lock().unwrap().is_empty());

    // A read through the same Deny pairing still works, and carries no start.
    assert_ok(call(&a, &signed(&a_key, None, Some("target@c")), true).await).await;
    assert_eq!(*starts.lock().unwrap(), vec![None]);

    for setting in [RemoteStartSetting::Ask, RemoteStartSetting::Allow] {
        c.paired_gateways()
            .set_remote_starts(&ac.pairing_id, setting)
            .unwrap()
            .unwrap();
        assert_ok(call(&a, &signed_spawn(&a_key, "target@c"), true).await).await;
        assert_eq!(
            starts.lock().unwrap().last().cloned().flatten(),
            Some(RemoteStart {
                setting,
                pairing: pairing.clone(),
            })
        );
    }

    // C's own workspaces start threads with no question and no pairing.
    let c_key = source(&c);
    assert_ok(call(&c, &signed_spawn(&c_key, "target"), true).await).await;
    assert_eq!(
        starts.lock().unwrap().last().cloned().flatten(),
        Some(RemoteStart::allowed())
    );
}

#[tokio::test]
async fn a_start_to_an_engine_that_cannot_enforce_the_setting_is_refused() {
    let (a, _a_dir) = isolated();
    let (c, _c_dir) = isolated();
    let c_url = listen(&c).await;
    let ac = pair(&a, &c, &c_url, "c");
    let a_key = source(&a);
    // An engine built before the remote start setting serves only the read route.
    let (seen, _, _target_dir) = engine_serving(&c, &[RECEIVE_ROUTE]);
    let assert_refused = |body: &str| {
        assert!(body.contains("cannot enforce"), "{body}");
        assert!(body.contains("Nothing was started"), "{body}");
    };

    for setting in [RemoteStartSetting::Ask, RemoteStartSetting::Allow] {
        c.paired_gateways()
            .set_remote_starts(&ac.pairing_id, setting)
            .unwrap()
            .unwrap();
        let refused = call(&a, &signed_spawn(&a_key, "target@c"), true).await;
        assert!(refused.status().is_client_error(), "{}", refused.status());
        assert_refused(&body_text(refused).await);
    }
    // C's own workspaces get the same refusal.
    let refused = call(&c, &signed_spawn(&source(&c), "target"), true).await;
    assert!(refused.status().is_client_error(), "{}", refused.status());
    assert_refused(&body_text(refused).await);
    assert!(
        seen.lock().unwrap().is_empty(),
        "the old engine saw no start"
    );

    // Reads still reach the old engine.
    assert_ok(call(&a, &signed(&a_key, None, Some("target@c")), true).await).await;
    assert_eq!(seen.lock().unwrap().len(), 1);
}

async fn refusal(state: &GatewayState, via: Option<&str>, to: &str) -> String {
    let response = call(state, &signed(&source(state), via, Some(to)), true).await;
    assert!(response.status().is_client_error());
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&body).into_owned()
}

#[tokio::test]
async fn a_refused_route_names_the_step_the_user_takes() {
    let (a, _a_dir) = isolated();
    let (b, _b_dir) = isolated();
    pair(&a, &b, "http://127.0.0.1:1", "b");

    // Paired with access off, by label or by id, and not paired at all: one step covers each.
    for selector in ["local", a.gateway_id(), "elsewhere"] {
        let off = refusal(&b, Some(selector), "target").await;
        assert!(
            off.contains(&format!(
                "{selector} gives this gateway no agent return access"
            )),
            "{off}"
        );
        assert!(
            off.contains("Pair this gateway there if it is not listed"),
            "{off}"
        );
        assert!(off.contains("Then retry."), "{off}");
    }

    let outgoing = refusal(&b, None, "target@elsewhere").await;
    assert!(
        outgoing.contains("no outgoing pairing named \\\"elsewhere\\\""),
        "{outgoing}"
    );
    assert!(
        outgoing.contains(&format!(
            "lucidos workspaces targets --via {}",
            b.gateway_id()
        )),
        "{outgoing}"
    );
}

#[tokio::test]
async fn outgoing_reads_and_granted_return_reads_cross_three_directional_gateways() {
    let a_dir = tempfile::tempdir().unwrap();
    let b_dir = tempfile::tempdir().unwrap();
    let c_dir = tempfile::tempdir().unwrap();
    let a = GatewayState::for_tests_with_static_dir(Some(a_dir.path().to_path_buf()));
    let b = GatewayState::for_tests_with_static_dir(Some(b_dir.path().to_path_buf()));
    let c = GatewayState::for_tests_with_static_dir(Some(c_dir.path().to_path_buf()));
    let (a_url, b_url, c_url) = (listen(&a).await, listen(&b).await, listen(&c).await);
    let ab = pair(&a, &b, &b_url, "b");
    let ac = pair(&a, &c, &c_url, "c");
    let (a_key, b_key) = (source(&a), source(&b));
    let (seen, _target_dir) = target(&c);
    assert_ok(call(&a, &signed(&a_key, None, Some("target@c")), true).await).await;
    assert_eq!(seen.lock().unwrap().len(), 1);
    let replay = seen.lock().unwrap()[0].envelope.clone();
    let issued = lucidos_gateway_stamp::now_unix();
    let stamp = format!(
        "{issued}.{}",
        crate::relay::sign(&ac.credential, &signature_text(&replay, issued).unwrap())
    );
    let replay = SignedOperation {
        envelope: replay,
        stamp,
    };
    let response = crate::server::gateway_router(c.clone())
        .oneshot(
            Request::post(format!("{GATEWAY_API_PREFIX}/operations"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(HEADER_GATEWAY_CREDENTIAL, &ac.credential)
                .body(Body::from(serde_json::to_vec(&replay).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        response.status().is_client_error(),
        "a spent peer challenge cannot be replayed"
    );
    b.paired_gateways()
        .set_return_route(
            &ab.pairing_id,
            Some(ReturnRoute {
                grant_id: "no-grant".into(),
                gateway_id: a.gateway_id().into(),
                address: a_url.clone(),
            }),
        )
        .unwrap();
    assert!(call(
        &b,
        &signed(&b_key, Some(a.gateway_id()), Some("target@c")),
        true
    )
    .await
    .status()
    .is_client_error());
    assert_eq!(seen.lock().unwrap().len(), 1);

    let granted = a
        .remote_gateways()
        .set_return_access("b", true)
        .unwrap()
        .unwrap();
    let grant = granted.return_access.unwrap();
    b.paired_gateways()
        .set_return_route(
            &ab.pairing_id,
            Some(ReturnRoute {
                grant_id: grant.id.clone(),
                gateway_id: a.gateway_id().into(),
                address: a_url,
            }),
        )
        .unwrap();
    let response = call(
        &b,
        &signed(&b_key, Some(a.gateway_id()), Some("target@c")),
        true,
    )
    .await;
    assert!(response.status().is_success(), "{}", response.status());
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1].envelope.caller_gateway, b.gateway_id());
        assert_eq!(
            seen[1].envelope.request.via.as_deref(),
            Some(a.gateway_id())
        );
    }
    assert_ok(call(&b, &signed(&b_key, Some("local"), Some("target@c")), true).await).await;
    a.remote_gateways().set_return_access("b", false).unwrap();
    assert!(call(
        &b,
        &signed(&b_key, Some(a.gateway_id()), Some("target@c")),
        true
    )
    .await
    .status()
    .is_client_error());
    assert_eq!(seen.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn revoking_return_access_while_the_target_preflight_waits_blocks_dispatch() {
    let (a, _a_dir) = isolated();
    let (b, _b_dir) = isolated();
    let (c, _c_dir) = isolated();
    let started = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let (started_server, resume_server) = (started.clone(), resume.clone());
    let c_app = crate::server::gateway_router(c.clone()).layer(axum::middleware::from_fn(
        move |req: axum::extract::Request, next: axum::middleware::Next| {
            let (started, resume) = (started_server.clone(), resume_server.clone());
            async move {
                if req.uri().path().ends_with("/operation-challenges") {
                    started.notify_one();
                    resume.notified().await;
                }
                next.run(req).await
            }
        },
    ));
    let a_url = listen(&a).await;
    let ab = pair(&a, &b, &listen(&b).await, "b");
    pair(&a, &c, &listen_router(c_app).await, "c");
    grant_return(&a, &b, &ab, a_url);
    let key = source(&b);
    let (seen, _target_dir) = target(&c);
    let via = a.gateway_id().to_owned();
    let running = tokio::spawn(async move {
        call(&b, &signed(&key, Some(&via), Some("target@c")), true)
            .await
            .status()
    });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    a.remote_gateways().set_return_access("b", false).unwrap();
    resume.notify_one();
    assert!(tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap()
        .is_client_error());
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_outgoing_operation_refuses_a_changed_destination_identity() {
    let (a, _a_dir) = isolated();
    let (c, _c_dir) = isolated();
    let mut remote = pair(&a, &c, &listen(&c).await, "c");
    a.remote_gateways().remove("c").unwrap();
    remote.gateway_id = Some("another-gateway".into());
    a.remote_gateways().add(remote).unwrap();
    let key = source(&a);
    let (seen, _target_dir) = target(&c);
    let response = call(&a, &signed(&key, None, Some("target@c")), true).await;
    assert!(response.status().is_client_error());
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_ended_return_request_drops_its_unredeemed_receipt() {
    let (a, _a_dir) = isolated();
    let (b, _b_dir) = isolated();
    let hint: Arc<Mutex<Option<ReturnHint>>> = Default::default();
    let captured = hint.clone();
    let app = Router::new()
        .route(
            crate::peers::GATEWAY_HEALTH_PATH,
            axum::routing::get(|| async {
                Json(json!({"instance_id": crate::server::instance_id()}))
            }),
        )
        .route(
            &format!("{GATEWAY_API_PREFIX}/return-operation"),
            post(move |Json(request): Json<ReturnHint>| {
                let captured = captured.clone();
                async move {
                    *captured.lock().unwrap() = Some(request);
                    axum::http::StatusCode::SERVICE_UNAVAILABLE
                }
            }),
        );
    let ab = pair(&a, &b, &listen(&b).await, "b");
    grant_return(&a, &b, &ab, listen_router(app).await);
    let key = source(&b);
    let response = call(
        &b,
        &signed(&key, Some(a.gateway_id()), Some("target")),
        true,
    )
    .await;
    assert_eq!(
        response.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    let hint = hint.lock().unwrap().take().unwrap();
    assert!(b
        .paired_gateways()
        .with_return_receipts(&hint.pairing_id, &hint.grant_id, |receipts| receipts
            .redeem(
                &hint.receipt,
                &hint.pairing_id,
                &hint.grant_id,
                lucidos_gateway_stamp::now_unix()
            ))
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_return_receipt_from_before_the_routing_gateway_restart_is_refused() {
    let (a, _a_dir) = isolated();
    let (b, _b_dir) = isolated();
    let ab = pair(&a, &b, &listen(&b).await, "b");
    grant_return(&a, &b, &ab, listen(&a).await);
    let key = source(&b);
    let (seen, _target_dir) = target(&a);
    let mut envelope = signed(&key, Some(a.gateway_id()), Some("target")).envelope;
    envelope.caller_gateway = b.gateway_id().into();
    let grant = a.remote_gateways().list()[0].return_access.clone().unwrap();
    let operation = serde_json::to_string(&ReturnOperation {
        envelope,
        routing_instance: "previous-routing-process".into(),
    })
    .unwrap();
    let receipt = b
        .paired_gateways()
        .with_return_receipts(&ab.pairing_id, &grant.id, |receipts| {
            receipts.issue(
                &ab.pairing_id,
                &grant.id,
                operation,
                lucidos_gateway_stamp::now_unix(),
            )
        })
        .unwrap()
        .unwrap();
    let response = crate::server::gateway_router(a.clone())
        .oneshot(
            Request::post(format!("{GATEWAY_API_PREFIX}/return-operation"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&ReturnHint {
                        pairing_id: ab.pairing_id,
                        grant_id: grant.id,
                        receipt,
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
    assert!(seen.lock().unwrap().is_empty());
}
