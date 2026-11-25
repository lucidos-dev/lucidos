//! Verified engine operations, directional peer hops and return callbacks.

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, Request};
use axum::response::{IntoResponse, Response};
use axum::{routing::post, Extension, Json, Router};
use lucidos_gateway_stamp::remote_start::{
    RemotePairing, RemoteStart, RemoteStartSetting, HEADER_REMOTE_START,
};
use lucidos_gateway_stamp::workspace_operation::{
    receive_route, OperationEnvelope, OperationKind, ReadKind, SignedOperation, WorkspaceOperation,
};
use lucidos_gateway_stamp::Claim;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

use crate::auth::AuthenticatedLocalProcess;
use crate::error::ApiError;
use crate::gateway_protocol::{
    AuthenticatedGateway, GATEWAY_API_PREFIX, HEADER_GATEWAY_CREDENTIAL, OPERATIONS_PROTOCOL,
};
use crate::remote_gateways::RemoteGateway;
use crate::server::GatewayState;

const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

enum RoutingAuthority {
    Local,
    Return(RemoteGateway),
    Peer(HeaderMap),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReturnOperation {
    envelope: OperationEnvelope,
    routing_instance: String,
}

struct ReturnReceiptLease {
    state: GatewayState,
    receipt: String,
}

impl Drop for ReturnReceiptLease {
    fn drop(&mut self) {
        self.state
            .paired_gateways()
            .discard_return_receipt(&self.receipt);
    }
}

impl RoutingAuthority {
    /// Re-read the authority, and the remote start it allows. A local or
    /// return-routed start is always allowed: its own gateway's owner granted it.
    fn recheck(&self, state: &GatewayState) -> Result<RemoteStart, ApiError> {
        let start = match self {
            Self::Local => Some(RemoteStart::allowed()),
            Self::Return(remote) => (remote.return_access.is_some()
                && state.remote_gateways().list().contains(remote))
            .then(RemoteStart::allowed),
            Self::Peer(headers) => state
                .paired_gateways()
                .authorize(headers)
                .map_err(ApiError::internal)?
                .map(|gateway| RemoteStart {
                    setting: gateway.remote_starts,
                    pairing: Some(RemotePairing {
                        id: gateway.pairing_id,
                        label: gateway.label,
                    }),
                }),
        };
        start.ok_or_else(|| {
            ApiError::forbidden(
                "The operation's pairing or return grant was revoked before dispatch.",
            )
        })
    }
}

/// Names the setting and where its owner changes it, so an agent can relay it.
pub(crate) fn remote_start_denied(
    state: &GatewayState,
    pairing: Option<&RemotePairing>,
) -> ApiError {
    let label = pairing.map_or("this pairing", |p| p.label.as_str());
    ApiError::forbidden(format!(
        "Gateway {} does not let agents on \"{label}\" start threads. \
         Its owner can change that under Settings → Access → Pair a gateway.",
        state.gateway_id()
    ))
}

pub(crate) fn router() -> Router<GatewayState> {
    Router::new()
        .route("/", post(forward))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(axum::middleware::map_response(
            |mut response: Response| async move {
                response.headers_mut().insert(
                    lucidos_gateway_stamp::workspace_operation::PROTOCOL_HEADER,
                    lucidos_gateway_stamp::workspace_operation::PROTOCOL_VERSION
                        .parse()
                        .expect("literal header value"),
                );
                response
            },
        ))
}

fn signature_text(envelope: &OperationEnvelope, issued: u64) -> Result<String, ApiError> {
    let digest = envelope.digest().map_err(ApiError::bad_request)?;
    serde_json::to_string(&(
        issued,
        &envelope.request.via,
        &envelope.request.to,
        &envelope.caller_gateway,
        &envelope.caller_workspace,
        &envelope.caller_thread_id,
        &envelope.verdict,
        digest,
    ))
    .map_err(|e| ApiError::internal(e.to_string()))
}

async fn forward(
    State(state): State<GatewayState>,
    local: Option<Extension<AuthenticatedLocalProcess>>,
    Json(signed): Json<SignedOperation>,
) -> Result<Response, ApiError> {
    if local.is_none() {
        return Err(ApiError::forbidden(
            "An operation must enter through its source engine.",
        ));
    }
    let mut envelope = signed.envelope;
    let digest = envelope.digest().map_err(ApiError::bad_request)?;
    let key = state
        .stamp_key(&envelope.caller_workspace)
        .ok_or_else(|| ApiError::forbidden("The source engine has no signing key."))?;
    if !envelope.caller_gateway.is_empty()
        || !lucidos_gateway_stamp::is_plain_target(&envelope.caller_thread_id)
        || !key.verify_now(
            &signed.stamp,
            Claim::WorkspaceOperation(envelope.claim(&digest)),
        )
    {
        return Err(ApiError::forbidden(
            "The source engine's operation stamp does not verify.",
        ));
    }
    state
        .paired_gateways()
        .accept_operation_nonce(&envelope.caller_workspace, &envelope.nonce)
        .map_err(ApiError::conflict)?;
    envelope.caller_gateway = state.gateway_id().into();
    let selector = envelope.request.via.as_deref();
    if selector.is_none() || selector == Some(state.gateway_id()) {
        envelope.request.via = Some(state.gateway_id().into());
        return dispatch(&state, envelope, RoutingAuthority::Local).await;
    }
    let selector = selector.expect("explicit routing selector");
    let routes: Vec<_> = state
        .paired_gateways()
        .list()
        .into_iter()
        .filter(|g| {
            g.return_route
                .as_ref()
                .is_some_and(|route| route.gateway_id == selector || g.label == selector)
        })
        .collect();
    let [route] = routes.as_slice() else {
        return Err(no_single_route(selector, routes.len()));
    };
    let metadata = route.return_route.as_ref().expect("filtered route");
    envelope.request.via = Some(metadata.gateway_id.clone());
    let health = state
        .peer_connection()
        .health_with_headers(&metadata.address, &Default::default())
        .await
        .map_err(ApiError::bad_gateway)?;
    let routing_instance = health["instance_id"]
        .as_str()
        .filter(|id| lucidos_gateway_stamp::is_plain_target(id) && id.len() <= 256)
        .ok_or_else(|| {
            ApiError::conflict(
                "Update the routing gateway to establish its current process identity.",
            )
        })?;
    // Bind the process inside the source's authoritative receipt, never in the
    // untrusted callback hints. A routing restart cuts every pending receipt.
    let operation = serde_json::to_string(&ReturnOperation {
        envelope,
        routing_instance: routing_instance.into(),
    })
    .map_err(|e| ApiError::internal(e.to_string()))?;
    let receipt = state
        .paired_gateways()
        .with_return_receipts(&route.id, &metadata.grant_id, |receipts| {
            receipts.issue(
                &route.id,
                &metadata.grant_id,
                operation,
                lucidos_gateway_stamp::now_unix(),
            )
        })
        .map_err(ApiError::forbidden)?
        .map_err(ApiError::too_many_requests)?;
    let hint = ReturnHint {
        pairing_id: route.id.clone(),
        grant_id: metadata.grant_id.clone(),
        receipt,
    };
    let lease = ReturnReceiptLease {
        state: state.clone(),
        receipt: hint.receipt.clone(),
    };
    let response = state.peer_connection().return_request(&metadata.address, &format!("{GATEWAY_API_PREFIX}/return-operation"))
        .map_err(ApiError::bad_gateway)?.timeout(TIMEOUT).header(header::CONTENT_TYPE, "application/json").body(json_bytes(&hint)?).send().await
        .map_err(|_| ApiError::bad_gateway("The return operation did not finish. Its outcome may be uncertain; it was not retried."))?;
    drop(lease);
    Ok(crate::relay::streamed_response(response))
}

/// Names the one step the user takes, so an agent can relay it verbatim.
/// A pairing without a route stores no gateway id, so "off" and "unpaired"
/// look alike here, and the step covers both.
fn no_single_route(selector: &str, matching_routes: usize) -> ApiError {
    if matching_routes > 1 {
        return ApiError::conflict(format!(
            "Several paired gateways match \"{selector}\". \
             Select one by the gateway id `lucidos workspaces targets` reports."
        ));
    }
    ApiError::conflict(format!(
        "{selector} gives this gateway no agent return access. \
         On {selector}, open Settings → Access → Remote gateways. \
         Pair this gateway there if it is not listed, then open its fold \
         and turn on \"Agent return access\". Then retry."
    ))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReturnHint {
    pairing_id: String,
    grant_id: String,
    receipt: String,
}

pub(crate) async fn receive_return(
    State(state): State<GatewayState>,
    Json(hint): Json<ReturnHint>,
) -> Result<Response, ApiError> {
    let current = || {
        state.remote_gateways().list().into_iter().find(|g| {
            g.pairing_id == hint.pairing_id
                && g.return_access
                    .as_ref()
                    .is_some_and(|grant| grant.id == hint.grant_id)
        })
    };
    let remote = current()
        .ok_or_else(|| ApiError::forbidden("Return access is not enabled for this pairing."))?;
    let source = remote.gateway_id.as_deref().ok_or_else(|| {
        ApiError::conflict(
            "Pairing identity needs a gateway upgrade before return access can be used.",
        )
    })?;
    if !lucidos_gateway_stamp::is_plain_target(&hint.receipt) {
        return Err(ApiError::bad_request("Invalid return receipt."));
    }
    let response = state
        .peer_connection()
        .request(
            &remote,
            reqwest::Method::POST,
            &format!("{GATEWAY_API_PREFIX}/return-receipts/{}", hint.receipt),
        )
        .map_err(ApiError::bad_gateway)?
        .timeout(TIMEOUT)
        .header(header::CONTENT_TYPE, "application/json")
        .body(json_bytes(&json!({"grant_id":hint.grant_id}))?)
        .send()
        .await
        .map_err(|_| {
            ApiError::bad_gateway(
                "Could not authenticate the return receipt with its source gateway.",
            )
        })?;
    let operation: ReturnOperation = read_json(response).await?;
    if operation.routing_instance != crate::server::instance_id() {
        return Err(ApiError::forbidden("The routing gateway restarted after this return operation began. Start a new operation; an uncertain spawn must be inspected before retrying."));
    }
    let envelope = operation.envelope;
    if current().as_ref() != Some(&remote)
        || envelope.caller_gateway != source
        || envelope.request.via.as_deref() != Some(state.gateway_id())
    {
        return Err(ApiError::forbidden(
            "The current return grant, pairing or operation identity does not match.",
        ));
    }
    envelope.digest().map_err(ApiError::bad_request)?;
    dispatch(&state, envelope, RoutingAuthority::Return(remote)).await
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Challenge {
    receipt: String,
}

pub(crate) async fn challenge(
    State(state): State<GatewayState>,
    Extension(gateway): Extension<AuthenticatedGateway>,
) -> Result<Json<Challenge>, ApiError> {
    let receipt = state
        .paired_gateways()
        .with_pairing_receipts(&gateway.pairing_id, None, |receipts| {
            receipts.issue(
                &gateway.pairing_id,
                "peer",
                String::new(),
                lucidos_gateway_stamp::now_unix(),
            )
        })
        .map_err(ApiError::forbidden)?
        .map_err(ApiError::too_many_requests)?;
    Ok(Json(Challenge { receipt }))
}

pub(crate) async fn receive_peer(
    State(state): State<GatewayState>,
    Extension(gateway): Extension<AuthenticatedGateway>,
    headers: HeaderMap,
    Json(signed): Json<SignedOperation>,
) -> Result<Response, ApiError> {
    let credential = headers
        .get(HEADER_GATEWAY_CREDENTIAL)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("The peer credential is missing."))?;
    let (issued, mac) = signed
        .stamp
        .split_once('.')
        .ok_or_else(|| ApiError::forbidden("Invalid operation signature."))?;
    let issued: u64 = issued
        .parse()
        .map_err(|_| ApiError::forbidden("Invalid operation timestamp."))?;
    let now = lucidos_gateway_stamp::now_unix();
    let text = signature_text(&signed.envelope, issued)?;
    if now.abs_diff(issued) > lucidos_gateway_stamp::WINDOW_SECS
        || !crate::auth::ct_eq(mac, &crate::relay::sign(credential, &text))
    {
        return Err(ApiError::forbidden(
            "The peer's operation signature does not verify.",
        ));
    }
    state
        .paired_gateways()
        .with_pairing_receipts(&gateway.pairing_id, None, |receipts| {
            receipts.redeem(&signed.envelope.nonce, &gateway.pairing_id, "peer", now)
        })
        .map_err(ApiError::forbidden)?
        .ok_or_else(|| {
            ApiError::forbidden("The operation challenge is expired or already spent.")
        })?;
    if state
        .paired_gateways()
        .authorize(&headers)
        .map_err(ApiError::internal)?
        .is_none()
    {
        return Err(ApiError::unauthorized(
            "The peer pairing is no longer current.",
        ));
    }
    dispatch(&state, signed.envelope, RoutingAuthority::Peer(headers)).await
}

async fn dispatch(
    state: &GatewayState,
    mut envelope: OperationEnvelope,
    authority: RoutingAuthority,
) -> Result<Response, ApiError> {
    authority.recheck(state)?;
    let may_route = !matches!(&authority, RoutingAuthority::Peer(_));
    if let WorkspaceOperation::Read { request } = &envelope.request.operation {
        if request.kind == ReadKind::WorkspaceTargets {
            let workspaces: Vec<_> = state
                .list_status()
                .await
                .into_iter()
                .map(|w| json!({"id":w.id, "name":w.name}))
                .collect();
            let outgoing: Vec<_> = if may_route {
                state.remote_gateways().list().into_iter().map(|g|
                json!({"name":g.name, "gateway_id":g.gateway_id, "gateway_protocol":g.gateway_protocol})
            ).collect()
            } else {
                Vec::new()
            };
            let routes: Vec<_> = if may_route {
                state
                    .paired_gateways()
                    .list()
                    .into_iter()
                    .filter_map(|g| {
                        g.return_route
                            .map(|r| json!({"name":g.label, "gateway_id":r.gateway_id}))
                    })
                    .collect()
            } else {
                Vec::new()
            };
            authority.recheck(state)?;
            return Ok(Json(json!({"gateway_id":state.gateway_id(), "workspaces":workspaces, "outgoing":outgoing, "via":routes})).into_response());
        }
    }
    let target = envelope
        .request
        .to
        .clone()
        .ok_or_else(|| ApiError::bad_request("Select a target workspace."))?;
    if let Some((workspace, name)) = target.split_once('@') {
        if !may_route {
            return Err(ApiError::forbidden(
                "An ordinary pairing does not grant transitive routing.",
            ));
        }
        let remote = state
            .remote_gateways()
            .list()
            .into_iter()
            .find(|g| g.name == name)
            .ok_or_else(|| {
                let id = state.gateway_id();
                ApiError::not_found(format!(
                    "Gateway {id} has no outgoing pairing named \"{name}\". \
                     Run `lucidos workspaces targets --via {id}` to list the gateways it reaches. \
                     To add {name}, pair it under Settings → Access → Remote gateways on gateway {id}."
                ))
            })?;
        envelope.request.to = Some(workspace.into());
        return send_peer(state, remote, envelope, authority).await;
    }
    let id = state.workspace_call_target(&target).ok_or_else(|| ApiError::not_found("This gateway does not serve that workspace. Select workspace@remote for an outgoing pairing."))?;
    let key = state
        .stamp_key(&id)
        .ok_or_else(|| ApiError::conflict("The target engine has no gateway signing key."))?;
    let address = state
        .engine_address(&id)
        .ok_or_else(|| ApiError::conflict("The target workspace is not running."))?;
    envelope.request.to = Some(id.clone());
    let digest = envelope.digest().map_err(ApiError::bad_request)?;
    let stamp = key.sign_now(Claim::WorkspaceOperation(envelope.claim(&digest)));
    let start = authority.recheck(state)?;
    let kind = envelope.request.operation.kind();
    let mut request = Request::post(format!("/api/v1{}", receive_route(kind)))
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-lucidos-target-workspace", &id);
    if kind == OperationKind::Spawn {
        if start.setting == RemoteStartSetting::Deny {
            return Err(remote_start_denied(state, start.pairing.as_ref()));
        }
        request = request.header(
            HEADER_REMOTE_START,
            start.header_value(&key, &id, &envelope.nonce),
        );
    }
    let signed = SignedOperation { envelope, stamp };
    let request = request
        .body(Body::from(
            serde_json::to_vec(&signed).map_err(|e| ApiError::internal(e.to_string()))?,
        ))
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let response = tokio::time::timeout(
        TIMEOUT,
        crate::engine_hop::send(
            &address,
            request,
            crate::engine_hop::Credential::Local(state.local_token()),
        ),
    )
    .await
    .map_err(|_| {
        ApiError::bad_gateway("The target operation timed out; its outcome may be uncertain.")
    })?
    .map_err(|error| {
        crate::log_legacy!(
            "[Gateway] Could not dispatch a routed operation to the target engine: {error}"
        );
        ApiError::bad_gateway("Could not reach and verify the target workspace's engine.")
    })?;
    if response
        .headers()
        .get(lucidos_gateway_stamp::workspace_operation::PROTOCOL_HEADER)
        .is_none_or(|version| {
            version != lucidos_gateway_stamp::workspace_operation::PROTOCOL_VERSION
        })
    {
        // Only an engine that enforces the remote start setting serves the
        // start route. An older one answered 404 and started nothing.
        return Err(ApiError::conflict(match kind {
            OperationKind::Spawn => format!(
                "Gateway {} refused this start: the target workspace's engine predates the \
                 remote start setting and cannot enforce it. Restart that workspace, then \
                 retry. Nothing was started.",
                state.gateway_id()
            ),
            OperationKind::Read => {
                "Upgrade the target workspace's engine to use routed workspace operations.".into()
            }
        }));
    }
    Ok(response.map(Body::new))
}

async fn send_peer(
    state: &GatewayState,
    remote: RemoteGateway,
    mut envelope: OperationEnvelope,
    authority: RoutingAuthority,
) -> Result<Response, ApiError> {
    let health = state
        .peer_connection()
        .health_with_headers(&remote.address, &remote.headers)
        .await
        .map_err(ApiError::bad_gateway)?;
    if health["gateway_protocol"].as_u64().unwrap_or_default() < OPERATIONS_PROTOCOL.into() {
        return Err(ApiError::conflict(
            "Upgrade the remote gateway to use typed workspace operations.",
        ));
    }
    let identity = state
        .peer_connection()
        .destination_identity(&remote)
        .await
        .map_err(ApiError::bad_gateway)?
        .ok_or_else(|| {
            ApiError::conflict("Update the remote gateway to establish its routing identity.")
        })?;
    let remote = state
        .remote_gateways()
        .confirm_destination(&remote, &identity)
        .map_err(ApiError::conflict)?;
    let challenge: Challenge = read_json(
        state
            .peer_connection()
            .request(
                &remote,
                reqwest::Method::POST,
                &format!("{GATEWAY_API_PREFIX}/operation-challenges"),
            )
            .map_err(ApiError::bad_gateway)?
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(|_| {
                ApiError::bad_gateway("The remote operation challenge could not be obtained.")
            })?,
    )
    .await?;
    if !state.remote_gateways().list().contains(&remote) {
        return Err(ApiError::forbidden(
            "The outgoing pairing changed before the operation was sent.",
        ));
    }
    authority.recheck(state)?;
    envelope.nonce = challenge.receipt;
    let issued = lucidos_gateway_stamp::now_unix();
    let stamp = format!(
        "{issued}.{}",
        crate::relay::sign(&remote.credential, &signature_text(&envelope, issued)?)
    );
    let response = state.peer_connection().request(&remote, reqwest::Method::POST,
        &format!("{GATEWAY_API_PREFIX}/operations")).map_err(ApiError::bad_gateway)?
        .timeout(TIMEOUT).header(header::CONTENT_TYPE, "application/json").body(json_bytes(&SignedOperation { envelope, stamp })?).send().await
        .map_err(|_| ApiError::bad_gateway("The remote operation did not finish. Its outcome may be uncertain; it was not retried."))?;
    Ok(crate::relay::streamed_response(response))
}

fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>, ApiError> {
    serde_json::to_vec(value).map_err(|e| ApiError::internal(e.to_string()))
}

async fn read_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, ApiError> {
    if !response.status().is_success() {
        return Err(ApiError::bad_gateway(format!(
            "The peer refused the operation with HTTP {}.",
            response.status().as_u16()
        )));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ApiError::bad_gateway("Could not read the peer operation reply."))?
    {
        if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(ApiError::bad_gateway(
                "The peer operation reply is too large.",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::bad_gateway("The peer operation reply is invalid."))
}

#[cfg(test)]
#[path = "workspace_operations_tests.rs"]
mod tests;
