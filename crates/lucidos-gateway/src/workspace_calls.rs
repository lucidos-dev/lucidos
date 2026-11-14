//! `POST /~/api/v1/workspace-calls/{target}/chat/stream`: one engine's thread
//! spawns into another workspace, on this gateway or on a paired one.
//!
//! The calling engine signs its claim with its own stamp key, which this
//! gateway also holds: the caller workspace, its thread, and its spawn-gate
//! verdict on that thread. The gateway verifies the claim, then the proxy
//! stamps the same claim for the target under the target's key. The target
//! honors the verdict, since only the caller can read the caller's provenance.
//!
//! A target this gateway does not serve may sit on a paired gateway (ADR
//! 0590). This gateway asks each one, then relays the call to the one that
//! takes it, signed with that pairing's credential. That gateway verifies the
//! signature and stamps the call for its own engine. Both halves live here:
//! [`forward`] is the caller's gateway, [`receive`] the target's.
//!
//! Plan: `docs/plans/2026-11-10-unattested-callers-and-gateway-stamps.md`.

use std::time::Duration;

use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use lucidos_gateway_stamp::{
    Claim, StampKey, HEADER_CALLER_THREAD, HEADER_CALLER_VERDICT, HEADER_CALLER_WORKSPACE,
    HEADER_GATEWAY_STAMP, REASON_WORKSPACE_NOT_ON_THIS_GATEWAY, WORKSPACE_CALLS_PATH,
};

use crate::auth::{self, AuthenticatedLocalProcess};
use crate::error::ApiError;
use crate::gateway_protocol::{
    AuthenticatedGateway, GATEWAY_API_PREFIX, HEADER_GATEWAY_CREDENTIAL,
};
use crate::remote_gateways::RemoteGateway;
use crate::server::GatewayState;

/// The header carrying the caller's gateway's signature over a call it relays
/// to a paired gateway: `<issued unix seconds>.<hex HMAC>`, keyed by the
/// pairing's credential. Gateway-owned, like the claim headers.
pub(crate) const HEADER_RELAYED_CALL_SIGNATURE: &str = "x-lucidos-relayed-call-signature";

/// The first field of a relayed call's signed text. A relay claim starts with
/// its own version, so neither ever verifies as the other.
const RELAYED_CALL_KIND: &str = "workspace-call/v1";

/// How long a paired gateway may take to say whether it takes a call.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn router() -> Router<GatewayState> {
    Router::new().route("/:target/chat/stream", post(forward))
}

/// A workspace call this gateway verified, put on the request for the proxy
/// to stamp for the target. An extension, like `auth::AuthenticatedDevice`,
/// because a client can send a header and cannot send an extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttestedWorkspaceCall {
    pub(crate) caller_workspace: String,
    pub(crate) caller_thread_id: String,
    pub(crate) verdict: String,
}

impl AttestedWorkspaceCall {
    /// The claim's headers and its stamp for the engine of `target_workspace`,
    /// which holds `stamp_key`. With no key (an engine adopted keyless) the
    /// stamp is left off, and the target reads an unattested caller.
    pub(crate) fn forwarded_headers(
        &self,
        target_workspace: &str,
        stamp_key: Option<&StampKey>,
    ) -> Vec<(&'static str, String)> {
        let mut headers = self.claim_headers();
        if let Some(key) = stamp_key {
            headers.push((
                HEADER_GATEWAY_STAMP,
                key.sign_now(self.claim(target_workspace)),
            ));
        }
        headers
    }

    /// The claim's headers and their signature for a paired gateway, which
    /// knows `credential` by its digest.
    pub(crate) fn relayed_headers(
        &self,
        target: &str,
        credential: &str,
        issued_at: i64,
    ) -> Vec<(&'static str, String)> {
        let signature = crate::relay::sign(credential, &self.relayed_text(target, issued_at));
        let mut headers = self.claim_headers();
        headers.push((
            HEADER_RELAYED_CALL_SIGNATURE,
            format!("{issued_at}.{signature}"),
        ));
        headers
    }

    fn claim_headers(&self) -> Vec<(&'static str, String)> {
        vec![
            (HEADER_CALLER_WORKSPACE, self.caller_workspace.clone()),
            (HEADER_CALLER_THREAD, self.caller_thread_id.clone()),
            (HEADER_CALLER_VERDICT, self.verdict.clone()),
        ]
    }

    fn claim<'a>(&'a self, target_workspace: &'a str) -> Claim<'a> {
        Claim::WorkspaceCall {
            target_workspace,
            caller_workspace: &self.caller_workspace,
            caller_thread_id: &self.caller_thread_id,
            verdict: &self.verdict,
        }
    }

    /// Every field percent-encoded, so none can shift a `|` boundary.
    fn relayed_text(&self, target: &str, issued_at: i64) -> String {
        let issued_at = issued_at.to_string();
        [
            RELAYED_CALL_KIND,
            &issued_at,
            target,
            &self.caller_workspace,
            &self.caller_thread_id,
            &self.verdict,
        ]
        .map(|field| urlencoding::encode(field).into_owned())
        .join("|")
    }
}

/// The call `headers` claim, before any check.
fn claimed_call(headers: &HeaderMap) -> Option<AttestedWorkspaceCall> {
    let header = |name| headers.get(name).and_then(|v| v.to_str().ok());
    Some(AttestedWorkspaceCall {
        caller_workspace: header(HEADER_CALLER_WORKSPACE)?.to_string(),
        caller_thread_id: header(HEADER_CALLER_THREAD)?.to_string(),
        verdict: header(HEADER_CALLER_VERDICT)?.to_string(),
    })
}

/// The call `headers` claim, when the engine of the calling workspace signed
/// it for `target`. `caller_key` looks that engine's stamp key up by id.
fn verified_call(
    target: &str,
    headers: &HeaderMap,
    caller_key: impl FnOnce(&str) -> Option<std::sync::Arc<StampKey>>,
) -> Option<AttestedWorkspaceCall> {
    let call = claimed_call(headers)?;
    let key = caller_key(&call.caller_workspace)?;
    let stamp = headers.get(HEADER_GATEWAY_STAMP)?.to_str().ok()?;
    key.verify_now(stamp, call.claim(target)).then_some(call)
}

/// The call `headers` claim, when the gateway holding `credential` signed it
/// for `target` within the relay's clock skew of `now`.
fn verified_relayed_call(
    target: &str,
    headers: &HeaderMap,
    credential: &str,
    now: i64,
) -> Option<AttestedWorkspaceCall> {
    let call = claimed_call(headers)?;
    let signature = headers.get(HEADER_RELAYED_CALL_SIGNATURE)?.to_str().ok()?;
    let (issued_at, mac) = signature.split_once('.')?;
    let issued_at: i64 = issued_at.parse().ok()?;
    if now.abs_diff(issued_at) > crate::relay::CLAIM_MAX_SKEW_SECS {
        return None;
    }
    let expected = crate::relay::sign(credential, &call.relayed_text(target, issued_at));
    auth::ct_eq(&expected, mac).then_some(call)
}

fn not_on_this_gateway(target: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": format!("This gateway serves no workspace called \"{target}\"."),
            "reason": REASON_WORKSPACE_NOT_ON_THIS_GATEWAY,
        })),
    )
        .into_response()
}

/// The target's engine hears the call as `/api/v1/chat/stream`, stamped by
/// the proxy from `call`.
async fn serve_call(
    state: &GatewayState,
    id: &str,
    call: AttestedWorkspaceCall,
    mut req: Request,
) -> Response {
    let Ok(uri) = format!("/{id}/api/v1/chat/stream").parse() else {
        return ApiError::internal("The target workspace id is not a path.").into_response();
    };
    *req.uri_mut() = uri;
    req.extensions_mut().remove::<auth::AuthenticatedDevice>();
    req.extensions_mut().insert(call);
    crate::server::serve_workspace(state, id, &format!("/{id}/"), req).await
}

async fn forward(
    State(state): State<GatewayState>,
    Path(target): Path<String>,
    local: Option<Extension<AuthenticatedLocalProcess>>,
    req: Request,
) -> Response {
    if local.is_none() {
        return ApiError::forbidden("Only an engine on this machine can place a workspace call.")
            .into_response();
    }
    let Some(call) = verified_call(&target, req.headers(), |id| state.stamp_key(id)) else {
        return ApiError::forbidden(
            "The calling engine's stamp did not verify, so the call names no workspace.",
        )
        .into_response();
    };
    if let Some(id) = state.workspace_call_target(&target) {
        return serve_call(&state, &id, call, req).await;
    }
    match paired_gateway_serving(&state, &target).await {
        Some(remote) => relay_call(&state, &remote, &target, &call, req).await,
        None => not_on_this_gateway(&target),
    }
}

/// The one paired gateway that takes calls for `target`, all asked at once.
/// Two that both do leave the call to the caller, as two directories
/// carrying one name do on a single gateway.
async fn paired_gateway_serving(state: &GatewayState, target: &str) -> Option<RemoteGateway> {
    if !lucidos_gateway_stamp::is_plain_target(target) {
        return None;
    }
    let client = state.peer_connection().client()?;
    let path = format!("{GATEWAY_API_PREFIX}{WORKSPACE_CALLS_PATH}/{target}");
    let probes = state.remote_gateways().list().into_iter().map(|remote| {
        let (client, path) = (client.clone(), path.clone());
        async move {
            let request = client.request(&remote, reqwest::Method::GET, &path).ok()?;
            let answer = request.timeout(PROBE_TIMEOUT).send().await.ok()?;
            (answer.status() == reqwest::StatusCode::NO_CONTENT).then_some(remote)
        }
    });
    let mut serving = futures::future::join_all(probes)
        .await
        .into_iter()
        .flatten();
    match (serving.next(), serving.next()) {
        (Some(remote), None) => Some(remote),
        _ => None,
    }
}

/// Relay `req` to `remote`, signed with its pairing's credential. The body and
/// the answer stream through, as the relay's do.
async fn relay_call(
    state: &GatewayState,
    remote: &RemoteGateway,
    target: &str,
    call: &AttestedWorkspaceCall,
    req: Request,
) -> Response {
    let Some(path) = lucidos_gateway_stamp::workspace_call_path(target) else {
        return not_on_this_gateway(target);
    };
    let Some(client) = state.peer_connection().client() else {
        return ApiError::internal("The relay client is unavailable.").into_response();
    };
    let (parts, body) = req.into_parts();
    let mut headers = HeaderMap::new();
    for (name, value) in &parts.headers {
        if name != header::COOKIE && !crate::proxy::gateway_owns_header(name, false) {
            headers.append(name.clone(), value.clone());
        }
    }
    for name in remote.headers.names() {
        headers.remove(name.as_str());
    }
    let signed = call.relayed_headers(target, &remote.credential, chrono::Utc::now().timestamp());
    for (name, value) in signed {
        let Ok(value) = HeaderValue::from_str(&value) else {
            return ApiError::internal("The call's claim is not a header value.").into_response();
        };
        headers.insert(name, value);
    }
    let upstream = match client.request(
        remote,
        reqwest::Method::POST,
        &format!("{GATEWAY_API_PREFIX}{path}"),
    ) {
        Ok(request) => crate::relay::with_body(request.headers(headers), &parts.headers, body),
        Err(reason) => return ApiError::internal(reason).into_response(),
    };
    match upstream.send().await {
        Ok(response) => crate::relay::streamed_response(response),
        Err(e) => {
            crate::log!(
                "[Gateway] workspace call to '{target}' on remote gateway '{}' failed: {}",
                remote.name,
                crate::external_address::error_chain(&e.without_url())
            );
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": format!(
                        "The remote gateway '{}' that serves \"{target}\" could not be reached.",
                        remote.name
                    ),
                })),
            )
                .into_response()
        }
    }
}

/// `GET /~/api/gateway/v1/workspace-calls/{target}`: does this gateway take
/// calls for `target`? A paired gateway asks before it relays one.
pub(crate) async fn takes_calls_for(
    State(state): State<GatewayState>,
    Path(target): Path<String>,
) -> Response {
    match state.workspace_call_target(&target) {
        Some(_) => StatusCode::NO_CONTENT.into_response(),
        None => not_on_this_gateway(&target),
    }
}

/// `POST /~/api/gateway/v1/workspace-calls/{target}/chat/stream`: a call a
/// paired gateway relayed, behind `require_gateway`.
///
/// The signature is checked against the credential the request presented,
/// which `require_gateway` already matched to a current pairing.
pub(crate) async fn receive(
    State(state): State<GatewayState>,
    Extension(gateway): Extension<AuthenticatedGateway>,
    Path(target): Path<String>,
    req: Request,
) -> Response {
    let credential = req
        .headers()
        .get(HEADER_GATEWAY_CREDENTIAL)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .unwrap_or_default();
    let now = chrono::Utc::now().timestamp();
    let Some(call) = verified_relayed_call(&target, req.headers(), credential, now) else {
        crate::log!(
            "[Gateway] refused a workspace call relayed by '{}': its signature does not verify",
            gateway.label
        );
        return ApiError::unauthorized("The relayed workspace call's signature does not verify.")
            .into_response();
    };
    let Some(id) = state.workspace_call_target(&target) else {
        return not_on_this_gateway(&target);
    };
    serve_call(&state, &id, call, req).await
}

#[cfg(test)]
#[path = "workspace_calls_tests.rs"]
mod tests;
