//! A best-effort signal from the gateway to every engine it routes.
//!
//! The gateway owns state its engines show, such as users and pairings
//! that wait for an Allow. When it changes, each engine hears a POST and tells
//! its pages through its own event stream. Nobody waits on the hops: a missed
//! signal costs one stale screen until the next read, never a wrong decision.

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};

use crate::engine_hop::{self, Credential, EngineAddress};
use crate::server::GatewayState;

/// One engine hop.
const ENGINE_SIGNAL_TIMEOUT: Duration = Duration::from_secs(5);

/// POST `body` to `path` on every routed engine. Returns at once; outside a
/// runtime, which only a sync test is, it does nothing. `what` names the
/// signal in the log.
pub fn signal_engines(
    state: &GatewayState,
    path: &'static str,
    body: Option<serde_json::Value>,
    what: &'static str,
) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let engines = state.routed_engines();
    let token = state.local_token().to_string();
    handle.spawn(async move {
        let body = body.map(|b| b.to_string());
        futures::future::join_all(
            engines
                .iter()
                .map(|engine| signal_engine(engine, &token, path, body.clone(), what)),
        )
        .await;
    });
}

async fn signal_engine(
    engine: &EngineAddress,
    local_token: &str,
    path: &str,
    body: Option<String>,
    what: &str,
) {
    let request = match body {
        Some(json) => Request::post(path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json)),
        None => Request::post(path).body(Body::empty()),
    };
    let Ok(request) = request else {
        return;
    };
    let sent = engine_hop::send(engine, request, Credential::Local(local_token));
    match tokio::time::timeout(ENGINE_SIGNAL_TIMEOUT, sent).await {
        Ok(Ok(response)) if response.status().is_success() => {}
        // An engine that predates the signal has no such route.
        Ok(Ok(response)) if response.status() == StatusCode::NOT_FOUND => {}
        Ok(Ok(response)) => crate::log!(
            "[Gateway] engine at {engine} refused the {what} signal: HTTP {}",
            response.status().as_u16()
        ),
        Ok(Err(e)) => crate::log!("[Gateway] could not signal the engine at {engine}: {e}"),
        Err(_) => {
            crate::log!("[Gateway] the engine at {engine} did not take the {what} signal in time")
        }
    }
}
