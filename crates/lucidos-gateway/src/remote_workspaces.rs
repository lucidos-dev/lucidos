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

use crate::auth;
use crate::gateway_protocol::{
    ListedWorkspace, GATEWAY_API_PREFIX, HEADER_GATEWAY_CREDENTIAL, WORKSPACES_PATH,
};
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
    #[serde(flatten)]
    pub listing: Listing,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Listing {
    Reachable { workspaces: Vec<ListedWorkspace> },
    Unreachable { reason: String },
}

/// A's routes, nested at `/~/api/v1/remote-workspaces`, device-gated by
/// `auth_api::enforce` and closed to app documents.
pub fn router() -> Router<GatewayState> {
    Router::new()
        .route("/", get(list))
        .route_layer(middleware::from_fn(crate::control::control_authz))
}

async fn list(State(state): State<GatewayState>) -> Json<Vec<RemoteGatewayListing>> {
    let client = state.relay_client().client();
    let listings = state.remote_gateways().list().into_iter().map(|gateway| {
        let client = client.clone();
        async move {
            let listing = match client {
                Some(client) => match fetch(&client, &gateway).await {
                    Ok(workspaces) => Listing::Reachable { workspaces },
                    Err(reason) => Listing::Unreachable { reason },
                },
                None => Listing::Unreachable {
                    reason: "the relay client is unavailable".to_string(),
                },
            };
            RemoteGatewayListing {
                gateway: gateway.name,
                listing,
            }
        }
    });
    Json(futures::future::join_all(listings).await)
}

/// `gateway`'s workspaces, or a sentence saying why not.
async fn fetch(
    client: &reqwest::Client,
    gateway: &RemoteGateway,
) -> Result<Vec<ListedWorkspace>, String> {
    let mut request = client
        .get(format!(
            "{}{GATEWAY_API_PREFIX}{WORKSPACES_PATH}",
            gateway.address
        ))
        .timeout(LIST_TIMEOUT);
    if let Some(credential) = auth::sensitive_credential(&gateway.credential) {
        request = request.header(HEADER_GATEWAY_CREDENTIAL, credential);
    }
    let response = request.send().await.map_err(|e| {
        format!(
            "could not reach {}: {}",
            gateway.address,
            crate::external_address::error_chain(&e)
        )
    })?;
    match response.status() {
        status if status.is_success() => {
            let bytes = response.bytes().await.map_err(|e| e.to_string())?;
            serde_json::from_slice(&bytes)
                .map_err(|_| format!("{} answered with an unreadable list", gateway.address))
        }
        reqwest::StatusCode::UNAUTHORIZED => Err(format!(
            "{} no longer grants this gateway access. Remove it here and pair again",
            gateway.address
        )),
        status => Err(format!(
            "{} answered HTTP {}",
            gateway.address,
            status.as_u16()
        )),
    }
}

#[cfg(test)]
#[path = "remote_workspaces_tests.rs"]
mod tests;
