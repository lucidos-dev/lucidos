//! Reverse-proxy for Vite dev server (dev mode only).
//!
//! When `LUCIDOS_DEV_PROXY` is set, unmatched requests are forwarded to Vite
//! so the browser sees a single origin for both API and frontend. WebSocket
//! (HMR) is handled by Vite directly via explicit hmr.host/hmr.port config.

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use std::time::Duration;

/// Shared reqwest client — reuses connections to Vite instead of creating
/// a new TLS handshake + connection pool per request.
static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .pool_max_idle_per_host(5)
        .pool_idle_timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build dev_proxy reqwest client")
});

pub async fn proxy(vite_base: String, req: axum::extract::Request) -> Response {
    // Use path_and_query only — over HTTP/2 the full URI includes scheme+authority
    let uri = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .to_string();
    let method = req.method().clone();
    let headers = req.headers().clone();
    let body = match axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    let url = format!("{}{}", vite_base, uri);
    // `reqwest::Method` is a re-export of `http::Method` (the same type
    // axum's `req.method()` returned), so no parse / clone roundtrip is
    // needed — pass the already-parsed method straight through.
    let mut builder = CLIENT.request(method, &url);
    for (name, value) in &headers {
        if name != header::HOST {
            if let Ok(v) = value.to_str() {
                builder = builder.header(name.as_str(), v);
            }
        }
    }
    if !body.is_empty() {
        builder = builder.body(body);
    }

    match builder.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let resp_headers = resp.headers().clone();
            let resp_body = resp.bytes().await.unwrap_or_default();
            let mut response = Response::builder().status(status);
            for (name, value) in &resp_headers {
                response = response.header(name, value);
            }
            response
                .body(axum::body::Body::from(resp_body))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(e) => {
            crate::log!("[DevProxy] Failed to proxy {} -> {}: {:?}", uri, url, e);
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}
