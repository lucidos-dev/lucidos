use std::time::Duration;

use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use lucidos_gateway_stamp::remote_start::{RemotePairing, RemoteStartSetting};
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

pub(crate) const HEADER_RELAYED_CALL_SIGNATURE: &str = "x-lucidos-relayed-call-signature";

const RELAYED_CALL_KIND: &str = "workspace-call/v1";

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn router() -> Router<GatewayState> {
    Router::new().route("/:target/chat/stream", post(forward))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttestedWorkspaceCall {
    pub(crate) caller_workspace: String,
    pub(crate) caller_thread_id: String,
    pub(crate) verdict: String,
}

impl AttestedWorkspaceCall {
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

fn claimed_call(headers: &HeaderMap) -> Option<AttestedWorkspaceCall> {
    let header = |name| headers.get(name).and_then(|v| v.to_str().ok());
    Some(AttestedWorkspaceCall {
        caller_workspace: header(HEADER_CALLER_WORKSPACE)?.to_string(),
        caller_thread_id: header(HEADER_CALLER_THREAD)?.to_string(),
        verdict: header(HEADER_CALLER_VERDICT)?.to_string(),
    })
}

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

async fn paired_gateway_serving(state: &GatewayState, target: &str) -> Option<RemoteGateway> {
    if !lucidos_gateway_stamp::is_plain_target(target) {
        return None;
    }
    let client = state.peer_connection().clone();
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
    let client = state.peer_connection();
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
            crate::log_legacy!(
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

pub(crate) async fn takes_calls_for(
    State(state): State<GatewayState>,
    Path(target): Path<String>,
) -> Response {
    match state.workspace_call_target(&target) {
        Some(_) => StatusCode::NO_CONTENT.into_response(),
        None => not_on_this_gateway(&target),
    }
}

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
        crate::log_legacy!(
            "[Gateway] refused a workspace call relayed by '{}': its signature does not verify",
            gateway.label
        );
        return ApiError::unauthorized("The relayed workspace call's signature does not verify.")
            .into_response();
    };
    let Some(id) = state.workspace_call_target(&target) else {
        return not_on_this_gateway(&target);
    };
    if let Some(refusal) = remote_start_refusal(&state, &gateway) {
        return refusal.into_response();
    }
    serve_call(&state, &id, call, req).await
}

fn remote_start_refusal(state: &GatewayState, gateway: &AuthenticatedGateway) -> Option<ApiError> {
    match gateway.remote_starts {
        RemoteStartSetting::Allow => None,
        RemoteStartSetting::Deny => Some(crate::workspace_operations::remote_start_denied(
            state,
            Some(&RemotePairing {
                id: gateway.pairing_id.clone(),
                label: gateway.label.clone(),
            }),
        )),
        RemoteStartSetting::Ask => Some(ApiError::forbidden(format!(
            "Gateway {} asks its owner before agents on \"{}\" start a thread, and this route \
             cannot ask. Spawn it as a routed operation instead, with \
             `lucidos spawn-thread --to <workspace>@<remote>`, so its owner can answer.",
            state.gateway_id(),
            gateway.label
        ))),
    }
}

#[cfg(test)]
#[path = "workspace_calls_tests.rs"]
mod tests;
