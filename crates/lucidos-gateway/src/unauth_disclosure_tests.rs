//! What the public surface tells a caller with no credential.
//!
//! A gateway on a LAN or tailnet bind answers anyone who can reach it. Its
//! public paths must not tell such a caller which workspaces exist, which
//! release runs, or which addresses it still waits to bind.

use super::*;

const LOCAL: (&str, &str) = (auth::HEADER_LOCAL_TOKEN, "test-local-token");

/// A gateway with a picker on disk and one stopped workspace, `secret-ws`.
fn gateway_with_one_workspace() -> (GatewayState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("index.html"),
        "<html><head></head><body></body></html>",
    )
    .unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(dir.path().to_path_buf()));
    state
        .inner
        .registry
        .lock()
        .unwrap()
        .add(Workspace::gateway_provisioned(
            "secret-ws".into(),
            "Secret WS".into(),
            51997,
        ))
        .unwrap();
    (state, dir)
}

async fn call(state: &GatewayState, uri: &str, headers: &[(&str, &str)]) -> Response {
    let mut builder = axum::extract::Request::builder().method("GET").uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    gateway_router(state.clone())
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

/// Status, the headers a client can read, and the body, as one comparable value.
async fn observed(response: Response) -> (StatusCode, Vec<(String, String)>, String) {
    let status = response.status();
    let mut headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
        .collect();
    headers.sort();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

#[tokio::test]
async fn the_bare_root_never_names_the_sole_workspace_to_a_stranger() {
    let (state, _dir) = gateway_with_one_workspace();
    let page_load = [
        ("sec-fetch-mode", "navigate"),
        ("sec-fetch-dest", "document"),
    ];

    let (status, headers, body) = observed(call(&state, "/", &page_load).await).await;
    assert!(
        !status.is_redirection(),
        "a stranger is not sent into a workspace"
    );
    assert!(
        headers.iter().all(|(_, v)| !v.contains("secret-ws")),
        "{headers:?}"
    );
    assert!(!body.contains("secret-ws"));

    // The owner's own browser still drops straight into its only workspace.
    let owner = call(&state, "/", &[page_load[0], page_load[1], LOCAL]).await;
    assert_eq!(owner.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(owner.headers()[header::LOCATION], "/secret-ws/");
}

#[tokio::test]
async fn a_public_app_asset_answers_a_stopped_and_an_unknown_workspace_alike() {
    let (state, _dir) = gateway_with_one_workspace();
    // A script tag, a page load and a frame: the three shapes the fallback
    // tells apart.
    let shapes: [&[(&str, &str)]; 3] = [
        &[("sec-fetch-mode", "no-cors"), ("sec-fetch-dest", "script")],
        &[
            ("sec-fetch-mode", "navigate"),
            ("sec-fetch-dest", "document"),
        ],
        &[("sec-fetch-mode", "navigate"), ("sec-fetch-dest", "iframe")],
    ];
    for shape in shapes {
        let stopped = observed(call(&state, "/secret-ws/api/v1/sdk.js", shape).await).await;
        let unknown = observed(call(&state, "/nowhere-ws/api/v1/sdk.js", shape).await).await;
        assert_eq!(stopped, unknown, "{shape:?}");
        assert!(!stopped.2.contains("secret-ws"), "{shape:?}: {}", stopped.2);
    }
    tokio::task::yield_now().await;
    assert!(
        state.inner.starting.lock().await.is_empty(),
        "a stranger's asset request must not wake a stopped workspace"
    );
}

#[tokio::test]
async fn public_health_holds_no_release_count_or_pending_address() {
    let (state, _dir) = gateway_with_one_workspace();
    state
        .inner
        .pending_binds
        .write()
        .unwrap()
        .insert("100.64.0.9:5252".parse().unwrap());

    let (status, _, body) =
        observed(call(&state, crate::peers::GATEWAY_HEALTH_PATH, &[]).await).await;
    assert_eq!(status, StatusCode::OK);
    let public: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(public["status"], "ok");
    assert_eq!(public["role"], crate::peers::GATEWAY_ROLE);
    for detail in ["release", "workspaces", "pending_binds"] {
        assert!(public.get(detail).is_none(), "{detail} is public: {body}");
    }
    assert!(!body.contains("100.64.0.9"));

    // This machine's own processes still read all of it.
    let (_, _, body) =
        observed(call(&state, crate::peers::GATEWAY_HEALTH_PATH, &[LOCAL]).await).await;
    let local: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(local["release"], crate::LUCIDOS_RELEASE);
    assert_eq!(local["workspaces"], 0, "no engine is routed");
    assert_eq!(
        local["pending_binds"],
        serde_json::json!(["100.64.0.9:5252"])
    );
}
