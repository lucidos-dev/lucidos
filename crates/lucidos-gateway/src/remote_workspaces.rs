//! Remote workspaces: the Workspaces list's half from gateway A's side.
//!
//! A asks every remote gateway for its workspaces over the gateway protocol,
//! all at once. A gateway that does not answer stays in the list with the
//! reason, so the picker greys it out rather than drops it. B's half is
//! `gateway_protocol::list_workspaces`.

use std::time::Duration;

use axum::extract::State;
use axum::middleware;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::gateway_protocol::{ListedWorkspace, GATEWAY_API_PREFIX, WORKSPACES_PATH};
use crate::remote_gateways::RemoteGateway;
use crate::server::GatewayState;

/// How long A waits for one remote gateway's list. The picker shows the local
/// workspaces meanwhile, so a dead gateway costs this much and no more.
const LIST_TIMEOUT: Duration = Duration::from_secs(5);

/// One remote gateway's workspaces, or why A could not list them.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RemoteGatewayListing {
    /// The short name A's user gave the gateway, the `<name>` in
    /// `/<slug>@<name>/`.
    pub gateway: String,
    pub host: String,
    pub pairing_id: String,
    pub last_seen_at: Option<String>,
    #[serde(flatten)]
    pub listing: Listing,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Listing {
    Reachable {
        workspaces: Vec<ListedWorkspace>,
    },
    Unreachable {
        reason: String,
        reason_kind: ReasonKind,
    },
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum ReasonKind {
    Timeout,
    Refused,
    Tls,
    Revoked,
    Incompatible,
}

pub fn transport_reason(error: &reqwest::Error) -> ReasonKind {
    if error.is_timeout() {
        return ReasonKind::Timeout;
    }
    let chain = crate::external_address::error_chain(error).to_lowercase();
    if chain.contains("certificate") || chain.contains("tls") {
        ReasonKind::Tls
    } else {
        ReasonKind::Refused
    }
}

/// A's routes, nested at `/~/api/v1/remote-workspaces`, device-gated by
/// `auth_api::enforce` and closed to app documents.
pub fn router() -> Router<GatewayState> {
    Router::new()
        .route("/", get(list))
        .route_layer(middleware::from_fn(crate::control::control_authz))
}

async fn list(State(state): State<GatewayState>) -> Json<Vec<RemoteGatewayListing>> {
    let client = state.peer_connection().client();
    let listings = state.remote_gateways().list().into_iter().map(|gateway| {
        let client = client.clone();
        let state = state.clone();
        async move {
            let listing = match client {
                Some(client) => match fetch(&client, &gateway).await {
                    Ok(workspaces) => {
                        state.remote_gateways().last_seen.lock().unwrap().insert(
                            format!("{}@{}", gateway.pairing_id, gateway.address),
                            chrono::Utc::now().to_rfc3339(),
                        );
                        Listing::Reachable { workspaces }
                    }
                    Err((reason_kind, reason)) => Listing::Unreachable {
                        reason,
                        reason_kind,
                    },
                },
                None => Listing::Unreachable {
                    reason: "the relay client is unavailable".to_string(),
                    reason_kind: ReasonKind::Refused,
                },
            };
            let last_seen_at = state
                .remote_gateways()
                .last_seen
                .lock()
                .unwrap()
                .get(&format!("{}@{}", gateway.pairing_id, gateway.address))
                .cloned();
            RemoteGatewayListing {
                host: reqwest::Url::parse(&gateway.address)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_owned))
                    .unwrap_or_default(),
                last_seen_at,
                pairing_id: gateway.pairing_id,
                gateway: gateway.name,
                listing,
            }
        }
    });
    Json(futures::future::join_all(listings).await)
}

/// `gateway`'s workspaces, or a sentence saying why not.
async fn fetch(
    client: &crate::peer_connection::PeerConnection,
    gateway: &RemoteGateway,
) -> Result<Vec<ListedWorkspace>, (ReasonKind, String)> {
    if gateway.gateway_protocol < 1 {
        return Err((
            ReasonKind::Incompatible,
            "Update the remote gateway to a compatible Lucidos version".into(),
        ));
    }
    let request = client
        .request(
            gateway,
            reqwest::Method::GET,
            &format!("{GATEWAY_API_PREFIX}{WORKSPACES_PATH}"),
        )
        .map_err(|e| (ReasonKind::Refused, e))?
        .timeout(LIST_TIMEOUT);
    let response = request.send().await.map_err(|e| {
        (
            transport_reason(&e),
            format!(
                "could not reach {}: {}",
                gateway.address,
                crate::external_address::error_chain(&e)
            ),
        )
    })?;
    match response.status() {
        status if status.is_success() => {
            let bytes = response
                .bytes()
                .await
                .map_err(|e| (transport_reason(&e), e.to_string()))?;
            serde_json::from_slice(&bytes).map_err(|_| {
                (
                    ReasonKind::Incompatible,
                    format!("{} answered with an unreadable list", gateway.address),
                )
            })
        }
        reqwest::StatusCode::UNAUTHORIZED => Err((
            ReasonKind::Revoked,
            format!(
                "{} no longer grants this gateway access. Remove it here and pair again",
                gateway.address
            ),
        )),
        status => Err((
            ReasonKind::Refused,
            format!("{} answered HTTP {}", gateway.address, status.as_u16()),
        )),
    }
}

#[cfg(test)]
#[path = "remote_workspaces_tests.rs"]
mod tests;
