use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::Json;
use serde_json::{json, Value};

use crate::engine_hop::{self, Credential, EngineAddress};
use crate::gateway_protocol::GATEWAY_API_PREFIX;
use crate::remote_gateways::RemoteGateway;
use crate::server::GatewayState;

pub const MAIN_LANDED_PATH: &str = "/main-landed";

pub const GATEWAY_PAIRINGS_PATH: &str = "/gateway-pairings";

pub const NUDGE_PATH: &str = "/nudge";

const ENGINE_NUDGE_PATH: &str = "/api/v1/follow-main/nudge";

const NUDGE_TIMEOUT: Duration = Duration::from_secs(10);

const ENGINE_NUDGE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq)]
pub enum NudgeAnswer {
    Delivered,
    Unsupported,
}

pub async fn main_landed(State(state): State<GatewayState>) -> StatusCode {
    tokio::spawn(async move {
        nudge_remote_gateways(&state).await;
    });
    StatusCode::ACCEPTED
}

pub async fn gateway_pairings(State(state): State<GatewayState>) -> Json<Value> {
    let paired =
        !state.remote_gateways().list().is_empty() || !state.paired_gateways().list().is_empty();
    Json(json!({ "paired": paired }))
}

pub async fn nudge_remote_gateways(
    state: &GatewayState,
) -> Vec<(String, Result<NudgeAnswer, String>)> {
    let client = state.peer_connection();
    let nudges = state.remote_gateways().list().into_iter().map(|gateway| {
        let client = client.clone();
        async move {
            let answer = nudge(&client, &gateway).await;
            match &answer {
                Ok(NudgeAnswer::Delivered) => {}
                Ok(NudgeAnswer::Unsupported) => crate::log_legacy!(
                    "[Gateway] remote gateway {} predates the landing nudge; it follows by polling",
                    gateway.name
                ),
                Err(e) => crate::log_legacy!(
                    "[Gateway] could not nudge remote gateway {}: {e}",
                    gateway.name
                ),
            }
            (gateway.name, answer)
        }
    });
    futures::future::join_all(nudges).await
}

async fn nudge(
    client: &crate::peer_connection::PeerConnection,
    gateway: &RemoteGateway,
) -> Result<NudgeAnswer, String> {
    let request = client
        .request(
            gateway,
            reqwest::Method::POST,
            &format!("{GATEWAY_API_PREFIX}{NUDGE_PATH}"),
        )?
        .timeout(NUDGE_TIMEOUT);
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
        Ok(Ok(response)) => crate::log_legacy!(
            "[Gateway] engine at {engine} refused the landing nudge: HTTP {}",
            response.status().as_u16()
        ),
        Ok(Err(e)) => crate::log_legacy!("[Gateway] could not nudge the engine at {engine}: {e}"),
        Err(_) => {
            crate::log_legacy!("[Gateway] the engine at {engine} did not take the nudge in time")
        }
    }
}

#[cfg(test)]
#[path = "nudge_tests.rs"]
mod tests;
