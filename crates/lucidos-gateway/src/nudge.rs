//! The landing nudge: a landing on one gateway wakes the follow tasks on
//! the gateways it is paired with.
//!
//! 1. A's engine pushes to `origin` and posts [`MAIN_LANDED_PATH`] to A.
//! 2. A posts [`NUDGE_PATH`] to every remote gateway it holds a credential for.
//! 3. B checks the credential and posts [`ENGINE_NUDGE_PATH`] to each engine
//!    it routes. Each engine asks `origin` at once.
//!
//! The nudge travels in the pairing's direction only, because B trusts A and
//! not the other way. Every step is best-effort: a lost nudge costs one
//! polling interval. B answers an older A's missing call with 404, and A logs
//! that quietly rather than showing anyone.

use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::Json;
use serde_json::{json, Value};

use crate::auth;
use crate::engine_hop::{self, Credential, EngineAddress};
use crate::gateway_protocol::{GATEWAY_API_PREFIX, HEADER_GATEWAY_CREDENTIAL};
use crate::remote_gateways::RemoteGateway;
use crate::server::GatewayState;

/// A's control route its engine posts to, under `/~/api/v1/control`.
pub const MAIN_LANDED_PATH: &str = "/main-landed";

/// The control route an engine asks before each landing pass and follow round.
pub const GATEWAY_PAIRINGS_PATH: &str = "/gateway-pairings";

/// B's protocol route, relative to [`GATEWAY_API_PREFIX`].
pub const NUDGE_PATH: &str = "/nudge";

/// The engine route that wakes its follow task.
const ENGINE_NUDGE_PATH: &str = "/api/v1/follow-main/nudge";

/// A's hop to B. A nudge is one small request, and nobody waits on it.
const NUDGE_TIMEOUT: Duration = Duration::from_secs(10);

/// B's hop to each engine. B answers A only once every engine hop ended, so
/// this stays under [`NUDGE_TIMEOUT`]: a slow engine never reads as a failed
/// nudge on A.
const ENGINE_NUDGE_TIMEOUT: Duration = Duration::from_secs(5);

/// How one remote gateway took a nudge.
#[derive(Debug, PartialEq, Eq)]
pub enum NudgeAnswer {
    Delivered,
    /// B predates the nudge. Its follow task still polls.
    Unsupported,
}

/// A's control route: its engine pushed to `origin`. Answers at once, since
/// the nudges outlive the engine's short request.
pub async fn main_landed(State(state): State<GatewayState>) -> StatusCode {
    tokio::spawn(async move {
        nudge_remote_gateways(&state).await;
    });
    StatusCode::ACCEPTED
}

/// The control route behind origin-first landing (ADR 0589): is this gateway
/// paired, in either direction? An unpaired engine lands locally.
pub async fn gateway_pairings(State(state): State<GatewayState>) -> Json<Value> {
    let paired =
        !state.remote_gateways().list().is_empty() || !state.paired_gateways().list().is_empty();
    Json(json!({ "paired": paired }))
}

/// Nudge every remote gateway, and log each one that did not take it.
pub async fn nudge_remote_gateways(
    state: &GatewayState,
) -> Vec<(String, Result<NudgeAnswer, String>)> {
    let Some(client) = state.relay_client().client() else {
        crate::log!("[Gateway] no relay client, so no remote gateway was nudged");
        return Vec::new();
    };
    let nudges = state.remote_gateways().list().into_iter().map(|gateway| {
        let client = client.clone();
        async move {
            let answer = nudge(&client, &gateway).await;
            match &answer {
                Ok(NudgeAnswer::Delivered) => {}
                Ok(NudgeAnswer::Unsupported) => crate::log!(
                    "[Gateway] remote gateway {} predates the landing nudge; it follows by polling",
                    gateway.name
                ),
                Err(e) => crate::log!(
                    "[Gateway] could not nudge remote gateway {}: {e}",
                    gateway.name
                ),
            }
            (gateway.name, answer)
        }
    });
    futures::future::join_all(nudges).await
}

async fn nudge(client: &reqwest::Client, gateway: &RemoteGateway) -> Result<NudgeAnswer, String> {
    let mut request = client
        .post(format!(
            "{}{GATEWAY_API_PREFIX}{NUDGE_PATH}",
            gateway.address
        ))
        .timeout(NUDGE_TIMEOUT);
    if let Some(credential) = auth::sensitive_credential(&gateway.credential) {
        request = request.header(HEADER_GATEWAY_CREDENTIAL, credential);
    }
    let response = request
        .send()
        .await
        .map_err(|e| crate::external_address::error_chain(&e))?;
    match response.status() {
        status if status.is_success() => Ok(NudgeAnswer::Delivered),
        reqwest::StatusCode::NOT_FOUND => Ok(NudgeAnswer::Unsupported),
        status => Err(format!("HTTP {}", status.as_u16())),
    }
}

/// B's protocol route: a paired gateway landed. `require_gateway` already
/// checked its credential.
pub async fn receive(State(state): State<GatewayState>) -> StatusCode {
    let engines = state.routed_engines();
    let token = state.local_token();
    futures::future::join_all(engines.iter().map(|engine| nudge_engine(engine, token))).await;
    StatusCode::ACCEPTED
}

async fn nudge_engine(engine: &EngineAddress, local_token: &str) {
    let Ok(request) = Request::post(ENGINE_NUDGE_PATH).body(Body::empty()) else {
        return;
    };
    let sent = engine_hop::send(engine, request, Credential::Local(local_token));
    match tokio::time::timeout(ENGINE_NUDGE_TIMEOUT, sent).await {
        Ok(Ok(response)) if response.status().is_success() => {}
        Ok(Ok(response)) => crate::log!(
            "[Gateway] engine at {engine} refused the landing nudge: HTTP {}",
            response.status().as_u16()
        ),
        Ok(Err(e)) => crate::log!("[Gateway] could not nudge the engine at {engine}: {e}"),
        Err(_) => crate::log!("[Gateway] the engine at {engine} did not take the nudge in time"),
    }
}

#[cfg(test)]
#[path = "nudge_tests.rs"]
mod tests;
