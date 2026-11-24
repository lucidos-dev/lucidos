use super::*;
use axum::{http::HeaderMap, routing::post, Router};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A gateway that echoes the request's credential into its error body.
fn echoing_gateway(status: axum::http::StatusCode) -> u16 {
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
    lucidos_local_token::test_support::serve_https(app).port()
}

fn client_with(token: &str) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        lucidos_local_token::HEADER_LOCAL_TOKEN,
        token.parse().unwrap(),
    );
    lucidos_local_token::client::client_builder()
        .default_headers(headers)
        .build()
        .unwrap()
}

#[tokio::test]
async fn successful_get_and_put_answers_never_carry_the_local_token() {
    let token = "success-secret-token-123";
    let app = Router::new().fallback(|headers: HeaderMap| async move {
        axum::Json(serde_json::json!({"echo":headers[lucidos_local_token::HEADER_LOCAL_TOKEN].to_str().unwrap()}))
    });
    let port = lucidos_local_token::test_support::serve_https(app)
        .port()
        .to_string();
    for method in [reqwest::Method::GET, reqwest::Method::PUT] {
        let text = send_with(
            &client_with(token),
            Some(token),
            &port,
            (method, "/gateway/status"),
            &serde_json::json!({}),
        )
        .await
        .unwrap();
        assert!(!text.contains(token));
        assert!(text.contains("[redacted]"));
    }
}

#[tokio::test]
async fn a_refusal_never_carries_the_local_token() {
    let port = echoing_gateway(axum::http::StatusCode::FORBIDDEN);
    let token = "local-secret-token-123";
    let refusal = post_with(
        &client_with(token),
        Some(token),
        &port.to_string(),
        "/workspaces/demo/stop",
        &serde_json::json!({}),
    )
    .await
    .unwrap_err();
    let ControlRefusal::Refused {
        status, message, ..
    } = &refusal
    else {
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

/// A stop route that counts the requests it handles.
fn counting_stop() -> (Router, Arc<AtomicUsize>) {
    let handled = Arc::new(AtomicUsize::new(0));
    let count = handled.clone();
    let app = Router::<()>::new().route(
        "/~/api/v1/control/workspaces/ida/stop",
        post(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async { axum::http::StatusCode::NO_CONTENT }
        }),
    );
    (app, handled)
}

fn loopback_client() -> reqwest::Client {
    lucidos_local_token::client::client_builder()
        .retry(reqwest::retry::never())
        .build()
        .unwrap()
}

#[tokio::test]
async fn a_stop_reaches_a_gateway_serving_the_loopback_ca() {
    let (app, handled) = counting_stop();
    let port = lucidos_local_token::test_support::serve_https(app).port();
    let done = post_with(
        &loopback_client(),
        None,
        &port.to_string(),
        "/workspaces/ida/stop",
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(done, Ok(String::new()));
    assert_eq!(handled.load(Ordering::SeqCst), 1);
}

/// Another process holding the port cannot present a leaf the loopback CA
/// signed, so the stop reaches nothing and fails loudly.
#[tokio::test]
async fn a_squatter_with_another_certificate_gets_no_request() {
    crate::net_config::install_crypto_provider();
    let (app, handled) = counting_stop();
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
    let refusal = post_with(
        &loopback_client(),
        None,
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
