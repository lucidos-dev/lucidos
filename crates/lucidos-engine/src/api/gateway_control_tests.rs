use super::*;
use crate::net_config::{SCHEME_HTTP, SCHEME_HTTPS};
use axum::{http::HeaderMap, routing::post, Router};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A gateway that echoes the request's credential into its error body.
async fn echoing_gateway(status: axum::http::StatusCode) -> u16 {
    let app = Router::<()>::new().route(
        "/~/api/v1/control/workspaces/demo/stop",
        post(move |headers: HeaderMap| async move {
            let token = headers
                .get(lucidos_local_token::HEADER_LOCAL_TOKEN)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            (
                status,
                axum::Json(serde_json::json!({ "error": format!("bad token {token}") })),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

fn client_with(token: &str) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        lucidos_local_token::HEADER_LOCAL_TOKEN,
        token.parse().unwrap(),
    );
    reqwest::Client::builder()
        .no_proxy()
        .default_headers(headers)
        .build()
        .unwrap()
}

#[tokio::test]
async fn a_refusal_never_carries_the_local_token() {
    let port = echoing_gateway(axum::http::StatusCode::FORBIDDEN).await;
    let token = "local-secret-token-123";
    let refusal = post_with(
        &client_with(token),
        Some(token),
        [SCHEME_HTTP, SCHEME_HTTPS],
        &port.to_string(),
        "/workspaces/demo/stop",
        &serde_json::json!({}),
    )
    .await
    .unwrap_err();
    let ControlRefusal::Refused { status, message } = &refusal else {
        panic!("expected a refusal, got {refusal:?}");
    };
    assert_eq!(*status, 403);
    assert_eq!(message, "bad token [redacted]");
    assert!(!format!("{refusal:?}").contains(token));
}

#[test]
fn a_body_without_an_error_field_reads_as_its_status() {
    let message = refusal_message(b"<html>secret</html>", reqwest::StatusCode::BAD_GATEWAY);
    assert_eq!(message, "the gateway answered 502 Bad Gateway");
}

/// A gateway that answers only HTTPS, as the dev gateway does, counting the
/// stop requests it handles.
async fn tls_only_gateway() -> (u16, Arc<AtomicUsize>) {
    crate::net_config::install_crypto_provider();
    let handled = Arc::new(AtomicUsize::new(0));
    let count = handled.clone();
    let app = Router::<()>::new().route(
        "/~/api/v1/control/workspaces/ida/stop",
        post(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async { axum::http::StatusCode::NO_CONTENT }
        }),
    );
    let (cert, key) = crate::test_support::self_signed_pem();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(cert, key)
        .await
        .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .serve(app.into_make_service())
            .await
            .unwrap()
    });
    (port, handled)
}

fn loopback_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .retry(reqwest::retry::never())
        .build()
        .unwrap()
}

/// The live bug: an engine behind a TLS gateway serves plain HTTP itself, so
/// its own scheme put `http` first and the stop never reached the gateway.
#[tokio::test]
async fn a_stop_reaches_a_tls_only_gateway_when_the_handoff_names_tls() {
    let (port, handled) = tls_only_gateway().await;
    let schemes = crate::net_config::gateway_scheme_order_from(
        (Some("/certs/cert.pem"), Some("/certs/key.pem")),
        (None, None),
    );
    let done = post_with(
        &loopback_client(),
        None,
        schemes,
        &port.to_string(),
        "/workspaces/ida/stop",
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(done, Ok(String::new()));
    assert_eq!(handled.load(Ordering::SeqCst), 1);
}

/// Never act twice: a plaintext attempt into a TLS listener may not fall
/// through to `https`, so the wrong order fails loudly and acts on nothing.
#[tokio::test]
async fn plain_http_into_a_tls_gateway_fails_without_a_retry() {
    let (port, handled) = tls_only_gateway().await;
    let refusal = post_with(
        &loopback_client(),
        None,
        [SCHEME_HTTP, SCHEME_HTTPS],
        &port.to_string(),
        "/workspaces/ida/stop",
        &serde_json::json!({}),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(refusal, ControlRefusal::Unreachable(_)),
        "{refusal:?}"
    );
    assert_eq!(handled.load(Ordering::SeqCst), 0);
}
