//! Plan invariant 9, through the real gateway router: once any device is
//! paired, a local process's pairing redeems only after a paired device's
//! Allow.

use axum::http::StatusCode;
use serde_json::{json, Value};
use tower::ServiceExt as _;

use crate::auth;
use crate::server::GatewayState;

const LOCAL_TOKEN: &str = "test-local-token";
const DEVICE_CREDENTIAL: &str = "cred-abc";

#[derive(Clone, Copy)]
enum Caller {
    LocalProcess,
    PairedDevice,
    Nobody,
}

/// A gateway whose stores live in a directory of its own, so a gateway
/// pairing can save.
fn gateway() -> (GatewayState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(dir.path().to_path_buf()));
    (state, dir)
}

/// A gateway with one paired device, which [`Caller::PairedDevice`] reaches.
fn with_a_paired_device() -> (GatewayState, tempfile::TempDir) {
    let (state, dir) = gateway();
    state
        .seed_device_for_test(auth::PairedDevice {
            id: "device-1".into(),
            label: "My MacBook".into(),
            credential_digest: auth::digest(DEVICE_CREDENTIAL),
            paired_at: "2020-01-01T00:00:00Z".into(),
            last_seen_at: None,
        })
        .unwrap();
    (state, dir)
}

async fn call(
    state: &GatewayState,
    caller: Caller,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = axum::extract::Request::builder().method(method).uri(path);
    builder = match caller {
        Caller::LocalProcess => builder.header(auth::HEADER_LOCAL_TOKEN, LOCAL_TOKEN),
        Caller::PairedDevice => builder.header(
            "cookie",
            format!("{}={DEVICE_CREDENTIAL}", state.device_cookie_name()),
        ),
        Caller::Nobody => builder,
    };
    let body = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            axum::body::Body::from(json.to_string())
        }
        None => axum::body::Body::empty(),
    };
    let response = crate::server::gateway_router(state.clone())
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Which pairing a test drives: a device's or a gateway's.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Device,
    Gateway,
}

impl Kind {
    async fn mint(self, state: &GatewayState, caller: Caller) -> String {
        let (path, field) = match self {
            Kind::Device => ("/~/api/v1/auth/pairing-code", "secret"),
            Kind::Gateway => ("/~/api/v1/auth/gateway-pairing-code", "code"),
        };
        let (status, body) = call(state, caller, "POST", path, None).await;
        assert_eq!(status, StatusCode::OK, "{self:?} mint: {body}");
        body[field].as_str().unwrap().to_string()
    }

    async fn redeem(self, state: &GatewayState, secret: &str) -> (StatusCode, Value) {
        let path = match self {
            Kind::Device => "/~/api/v1/auth/pair",
            Kind::Gateway => "/~/api/gateway/v1/pair",
        };
        let body = json!({ "code": secret, "label": "New phone" });
        call(state, Caller::Nobody, "POST", path, Some(body)).await
    }

    fn wire(self) -> &'static str {
        match self {
            Kind::Device => "device",
            Kind::Gateway => "gateway",
        }
    }
}

const KINDS: [Kind; 2] = [Kind::Device, Kind::Gateway];

async fn requests(state: &GatewayState) -> Vec<Value> {
    let (status, body) = call(
        state,
        Caller::PairedDevice,
        "GET",
        "/~/api/v1/auth/pairing-requests",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body.as_array().unwrap().clone()
}

async fn answer(state: &GatewayState, caller: Caller, id: &str, verb: &str) -> StatusCode {
    let path = format!("/~/api/v1/auth/pairing-requests/{id}/{verb}");
    call(state, caller, "POST", &path, None).await.0
}

#[tokio::test]
async fn with_no_device_paired_a_local_process_s_code_redeems_at_once() {
    for kind in KINDS {
        let (state, _dir) = gateway();
        let secret = kind.mint(&state, Caller::LocalProcess).await;
        let (status, body) = kind.redeem(&state, &secret).await;
        assert_eq!(status, StatusCode::OK, "{kind:?}: {body}");
    }
}

#[tokio::test]
async fn a_paired_device_s_code_redeems_at_once() {
    for kind in KINDS {
        let (state, _dir) = with_a_paired_device();
        let secret = kind.mint(&state, Caller::PairedDevice).await;
        let (status, body) = kind.redeem(&state, &secret).await;
        assert_eq!(status, StatusCode::OK, "{kind:?}: {body}");
        assert!(requests(&state).await.is_empty());
    }
}

#[tokio::test]
async fn a_local_process_s_code_waits_for_a_paired_device_s_allow() {
    for kind in KINDS {
        let (state, _dir) = with_a_paired_device();
        let secret = kind.mint(&state, Caller::LocalProcess).await;
        let (status, body) = kind.redeem(&state, &secret).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{kind:?}: {body}");
        assert_eq!(body["awaiting_owner"], json!(true), "{kind:?}: {body}");

        let waiting = requests(&state).await;
        assert_eq!(waiting.len(), 1, "{kind:?}: {waiting:?}");
        assert_eq!(waiting[0]["kind"], json!(kind.wire()));
        assert_eq!(waiting[0]["label"], json!("New phone"));
        let id = waiting[0]["id"].as_str().unwrap();

        // The local process cannot answer its own request.
        assert_eq!(
            answer(&state, Caller::LocalProcess, id, "allow").await,
            StatusCode::FORBIDDEN,
            "{kind:?}"
        );
        assert_eq!(kind.redeem(&state, &secret).await.0, StatusCode::ACCEPTED);

        assert_eq!(
            answer(&state, Caller::PairedDevice, id, "allow").await,
            StatusCode::NO_CONTENT
        );
        let (status, body) = kind.redeem(&state, &secret).await;
        assert_eq!(status, StatusCode::OK, "{kind:?}: {body}");
        assert!(requests(&state).await.is_empty());
    }
}

#[tokio::test]
async fn don_t_allow_fails_the_redeem() {
    for kind in KINDS {
        let (state, _dir) = with_a_paired_device();
        let secret = kind.mint(&state, Caller::LocalProcess).await;
        assert_eq!(kind.redeem(&state, &secret).await.0, StatusCode::ACCEPTED);
        let id = requests(&state).await[0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            answer(&state, Caller::PairedDevice, &id, "decline").await,
            StatusCode::NO_CONTENT
        );
        let (status, body) = kind.redeem(&state, &secret).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{kind:?}: {body}");
        assert!(
            body["error"].as_str().unwrap().contains("declined"),
            "{kind:?}: {body}"
        );
        assert_eq!(
            kind.redeem(&state, &secret).await.0,
            StatusCode::BAD_REQUEST,
            "{kind:?}: a declined pairing is gone"
        );
    }
}

#[tokio::test]
async fn an_unknown_request_answers_404() {
    let (state, _dir) = with_a_paired_device();
    assert_eq!(
        answer(&state, Caller::PairedDevice, "no-such-request", "allow").await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn nobody_unpaired_reads_or_answers_the_requests() {
    let (state, _dir) = with_a_paired_device();
    let (status, _) = call(
        &state,
        Caller::Nobody,
        "GET",
        "/~/api/v1/auth/pairing-requests",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        answer(&state, Caller::Nobody, "any", "allow").await,
        StatusCode::UNAUTHORIZED
    );
}

/// Both ends hear it: a started wait tells each routed engine who waits, so it
/// notifies the owner's devices, and an answer refreshes their list.
#[tokio::test]
async fn the_engines_hear_a_started_wait_and_its_answer() {
    let (state, dir) = with_a_paired_device();
    let (sent, mut received) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let app = axum::Router::new().route(
        "/api/v1/pairing-requests/changed",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let sent = sent.clone();
            async move {
                sent.send(body).unwrap();
                StatusCode::ACCEPTED
            }
        }),
    );
    state.set_route_for_test("dev", crate::engine_hop::test_engine::serve(&dir, app));
    async fn next(received: &mut tokio::sync::mpsc::UnboundedReceiver<Value>) -> Value {
        tokio::time::timeout(std::time::Duration::from_secs(5), received.recv())
            .await
            .expect("the engine heard nothing")
            .unwrap()
    }

    let secret = Kind::Device.mint(&state, Caller::LocalProcess).await;
    assert_eq!(
        Kind::Device.redeem(&state, &secret).await.0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        next(&mut received).await,
        json!({ "raised": { "kind": "device", "label": "New phone" } })
    );

    // Asking again is the same wait, so the owner is not told twice.
    assert_eq!(
        Kind::Device.redeem(&state, &secret).await.0,
        StatusCode::ACCEPTED
    );
    let id = requests(&state).await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        answer(&state, Caller::PairedDevice, &id, "allow").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(next(&mut received).await, json!({}));
}

#[tokio::test]
async fn a_local_process_may_read_the_requests() {
    let (state, _dir) = with_a_paired_device();
    let (status, body) = call(
        &state,
        Caller::LocalProcess,
        "GET",
        "/~/api/v1/auth/pairing-requests",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
