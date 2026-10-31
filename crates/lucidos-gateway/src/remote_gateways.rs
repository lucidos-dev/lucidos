//! Remote gateways, from the side that pairs: gateway A.
//!
//! A's user types B's address, a gateway pairing code B minted, and a short
//! name. A checks B's public health, redeems the code over `https`, and keeps
//! the gateway credential B returned. B's half is [`crate::gateway_protocol`],
//! and ADR 0590 records the decision.
//!
//! The credential crosses a network, so every call here is `https` only, with
//! certificates checked and redirects refused.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use axum::extract::State;
use axum::middleware;
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth;
use crate::error::ApiError;
use crate::gateway_protocol::{
    PairAnswer, GATEWAY_API_PREFIX, HEADER_GATEWAY_CREDENTIAL, PAIRING_PATH, PAIR_PATH,
};
use crate::peers::{GATEWAY_HEALTH_PATH, GATEWAY_ROLE};
use crate::server::GatewayState;
use lucidos_gateway_constants::REMOTE_GATEWAY_NAME_MAX_LEN;

/// How long one call to B may take, end to end. A person waits on it.
const PEER_TIMEOUT: Duration = Duration::from_secs(10);

/// One remote gateway, as A persists it.
///
/// The credential is in the clear because A presents it on every call. The
/// file is owner-only, and no API ever returns this field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteGateway {
    pub name: String,
    pub address: String,
    pub pairing_id: String,
    pub credential: String,
    pub gateway_protocol: u32,
    pub added_at: String,
}

/// A's remote-gateway file, `<data dir>/remote-gateways.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteGateways {
    #[serde(default)]
    pub gateways: Vec<RemoteGateway>,
}

/// Per gateway, beside the device store and never shared (ADR 0132).
pub fn remote_gateways_path(app_data: &Path) -> PathBuf {
    app_data.join("remote-gateways.json")
}

/// A's remote gateways, with the locking shape of `PairedGatewayStore`.
pub struct RemoteGatewayStore {
    path: PathBuf,
    data: Mutex<RemoteGateways>,
    writing: Mutex<()>,
}

impl RemoteGatewayStore {
    pub fn load(path: PathBuf) -> Result<Self, crate::BoxError> {
        let data = auth::load_store(&path)?;
        Ok(Self {
            path,
            data: Mutex::new(data),
            writing: Mutex::new(()),
        })
    }

    pub fn list(&self) -> Vec<RemoteGateway> {
        self.data
            .lock()
            .map(|d| d.gateways.clone())
            .unwrap_or_default()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.list().iter().any(|g| g.name == name)
    }

    /// Record `gateway`. Refuses a name already taken, checked under the same
    /// lock as the insert so two adds cannot both win.
    pub fn add(&self, gateway: RemoteGateway) -> Result<(), AddError> {
        let mut taken = false;
        self.write(|store| {
            taken = store.gateways.iter().any(|g| g.name == gateway.name);
            if !taken {
                store.gateways.push(gateway);
            }
        })
        .map_err(AddError::Internal)?;
        if taken {
            return Err(AddError::NameTaken);
        }
        Ok(())
    }

    /// Forget the remote gateway called `name`, handing it back.
    pub fn remove(&self, name: &str) -> Result<Option<RemoteGateway>, String> {
        let mut removed = None;
        self.write(|store| {
            if let Some(at) = store.gateways.iter().position(|g| g.name == name) {
                removed = Some(store.gateways.remove(at));
            }
        })?;
        Ok(removed)
    }

    fn write(&self, mutate: impl FnOnce(&mut RemoteGateways)) -> Result<(), String> {
        fn unavailable<T>(_: T) -> String {
            "remote gateway store is unavailable".to_string()
        }
        let _writing = self.writing.lock().map_err(unavailable)?;
        // Save first, publish after: a failed save changes nothing anyone sees.
        let mut next = self.data.lock().map_err(unavailable)?.clone();
        mutate(&mut next);
        auth::save_store(&next, &self.path)
            .map_err(|e| format!("could not save the remote gateway store: {e}"))?;
        *self.data.lock().map_err(unavailable)? = next;
        Ok(())
    }
}

/// Why adding a remote gateway failed, by who has to act.
#[derive(Debug, PartialEq, Eq)]
pub enum AddError {
    /// The user typed something unusable.
    Invalid(String),
    /// Another remote gateway already has that name.
    NameTaken,
    /// B did not answer, or refused.
    Remote(String),
    Internal(String),
}

impl From<AddError> for ApiError {
    fn from(e: AddError) -> Self {
        match e {
            AddError::Invalid(m) => ApiError::bad_request(m),
            AddError::NameTaken => {
                ApiError::conflict("another remote gateway already has that name")
            }
            AddError::Remote(m) => ApiError::bad_gateway(m),
            AddError::Internal(m) => ApiError::internal(m),
        }
    }
}

/// `raw` as a short name for routes, or why not.
pub fn valid_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.len() <= REMOTE_GATEWAY_NAME_MAX_LEN && crate::registry::is_valid_id(name) {
        return Ok(name.to_string());
    }
    Err(format!(
        "a short name is up to {REMOTE_GATEWAY_NAME_MAX_LEN} lowercase letters, digits and dashes, such as cloud"
    ))
}

/// `raw` as B's `https` origin, or why not.
///
/// Plain `http` is refused with the fix named, because the credential and
/// every device claim would cross the network in clear text (ADR 0590).
/// Loopback is allowed: it reaches nothing but this machine, and the
/// health check refuses this very gateway.
pub fn valid_address(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let origin = crate::pairing_qr::valid_origin(trimmed).ok_or_else(|| {
        format!(
            "'{}' is not an address. Use https, a host and an optional port, such as \
             https://lucidos.example.com, with no path",
            raw.trim()
        )
    })?;
    if origin.starts_with("http://") {
        return Err(format!(
            "'{origin}' is plain http. Gateways pair over https only, so the credential never \
             crosses a network in clear text. On the other gateway, turn on remote access with \
             `tailscale serve`, or set an https external address, then use that address"
        ));
    }
    let parsed = reqwest::Url::parse(origin)
        .map_err(|e| format!("'{origin}' is not an address this gateway can open ({e})"))?;
    let unroutable = parsed
        .host_str()
        .map(|h| h.trim_start_matches('[').trim_end_matches(']'))
        .and_then(|h| h.parse::<IpAddr>().ok())
        .map(|ip| ip.to_canonical())
        .is_some_and(|ip| match ip {
            IpAddr::V4(v4) => v4.is_link_local() || v4.is_unspecified(),
            IpAddr::V6(v6) => v6.is_unicast_link_local() || v6.is_unspecified(),
        });
    if unroutable {
        return Err(format!(
            "'{origin}' is a link-local or unspecified address, which reaches no gateway"
        ));
    }
    Ok(origin.to_string())
}

/// What B's public health said about itself.
#[derive(Debug, PartialEq, Eq)]
pub struct RemoteHealth {
    pub instance_id: String,
    pub gateway_protocol: u32,
}

/// Pure: may A pair with whatever answered `body`?
pub fn read_health(body: &serde_json::Value, own_instance: &str) -> Result<RemoteHealth, String> {
    if body.get("role").and_then(serde_json::Value::as_str) != Some(GATEWAY_ROLE) {
        return Err("something answered there, but not a Lucidos gateway".to_string());
    }
    let instance_id = body
        .get("instance_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if instance_id == own_instance {
        return Err("that address reaches this gateway itself".to_string());
    }
    let protocol = body
        .get("gateway_protocol")
        .and_then(serde_json::Value::as_u64)
        .and_then(|p| u32::try_from(p).ok())
        .unwrap_or(0);
    if protocol < 1 || instance_id.is_empty() {
        return Err(
            "that Lucidos gateway is too old to pair with another gateway. Update it, then try \
             again"
                .to_string(),
        );
    }
    Ok(RemoteHealth {
        instance_id: instance_id.to_string(),
        gateway_protocol: protocol,
    })
}

/// A's client for B: `https` only, certificates checked, no redirects.
pub struct PeerClient {
    http: reqwest::Client,
}

impl PeerClient {
    pub fn new() -> Result<Self, String> {
        Self::build(None)
    }

    /// A client that also trusts `pem`, for a test's self-signed stand-in.
    #[cfg(test)]
    pub fn trusting(pem: &[u8]) -> Self {
        Self::build(Some(reqwest::Certificate::from_pem(pem).unwrap())).unwrap()
    }

    fn build(extra_root: Option<reqwest::Certificate>) -> Result<Self, String> {
        let mut builder = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(PEER_TIMEOUT);
        if let Some(root) = extra_root {
            builder = builder.add_root_certificate(root);
        }
        let http = builder
            .build()
            .map_err(|e| format!("could not start the remote gateway client: {e}"))?;
        Ok(Self { http })
    }

    /// B's public health body.
    pub async fn health(&self, origin: &str) -> Result<serde_json::Value, String> {
        let response = self
            .http
            .get(format!("{origin}{GATEWAY_HEALTH_PATH}"))
            .send()
            .await
            .map_err(|e| format!("could not reach {origin}: {}", reached_error(&e)))?;
        if !response.status().is_success() {
            return Err(format!(
                "{origin} answered with HTTP {}, not with Lucidos's health",
                response.status().as_u16()
            ));
        }
        read_json(response)
            .await
            .map_err(|_| format!("{origin} answered, but not with Lucidos's health"))
    }

    /// Redeem `code` at B.
    pub async fn pair(&self, origin: &str, code: &str, label: &str) -> Result<PairAnswer, String> {
        let response = self
            .http
            .post(format!("{origin}{GATEWAY_API_PREFIX}{PAIR_PATH}"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::json!({ "code": code, "label": label }).to_string())
            .send()
            .await
            .map_err(|e| format!("could not reach {origin}: {}", reached_error(&e)))?;
        if !response.status().is_success() {
            return Err(refusal(response).await);
        }
        read_json(response)
            .await
            .map_err(|e| format!("{origin} paired, but its answer was unreadable: {e}"))
    }

    /// Ask B to forget this pairing.
    pub async fn leave(&self, gateway: &RemoteGateway) -> Result<(), String> {
        let mut request = self.http.delete(format!(
            "{}{GATEWAY_API_PREFIX}{PAIRING_PATH}",
            gateway.address
        ));
        if let Some(credential) = auth::sensitive_credential(&gateway.credential) {
            request = request.header(HEADER_GATEWAY_CREDENTIAL, credential);
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("could not reach {}: {}", gateway.address, reached_error(&e)))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(refusal(response).await)
        }
    }
}

/// The reason a request failed, naming a refused redirect plainly.
fn reached_error(e: &reqwest::Error) -> String {
    if e.is_redirect() {
        return "it redirected elsewhere, and a gateway credential never follows a redirect"
            .to_string();
    }
    crate::external_address::error_chain(e)
}

/// A JSON body, without reqwest's `json` feature.
async fn read_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, String> {
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// B's own sentence for a refusal, else the status.
async fn refusal(response: reqwest::Response) -> String {
    let status = response.status();
    read_json::<serde_json::Value>(response)
        .await
        .ok()
        .and_then(|b| b.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| format!("the other gateway answered HTTP {}", status.as_u16()))
}

/// What the user typed to add a remote gateway.
#[derive(Debug, Deserialize)]
pub struct AddRequest {
    pub address: String,
    pub code: String,
    pub name: String,
}

/// A remote gateway as the API shows it. Never the credential.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RemoteGatewayRow {
    pub name: String,
    pub address: String,
    pub pairing_id: String,
    pub gateway_protocol: u32,
    pub added_at: String,
}

impl From<RemoteGateway> for RemoteGatewayRow {
    fn from(g: RemoteGateway) -> Self {
        Self {
            name: g.name,
            address: g.address,
            pairing_id: g.pairing_id,
            gateway_protocol: g.gateway_protocol,
            added_at: g.added_at,
        }
    }
}

/// Pair with B and record it.
///
/// The name, the address and the code's shape are checked before the code is
/// spent, so a typo burns no code. B's external address then replaces the
/// typed one only when A reaches this same B there.
pub async fn add(
    store: &RemoteGatewayStore,
    client: &PeerClient,
    own_instance: &str,
    own_label: &str,
    request: AddRequest,
) -> Result<RemoteGatewayRow, AddError> {
    let name = valid_name(&request.name).map_err(AddError::Invalid)?;
    if store.contains(&name) {
        return Err(AddError::NameTaken);
    }
    let address = valid_address(&request.address).map_err(AddError::Invalid)?;
    let code = auth::valid_pairing_code(&request.code).ok_or_else(|| {
        AddError::Invalid(format!(
            "a gateway pairing code is {} digits",
            auth::PAIRING_CODE_DIGITS
        ))
    })?;
    let body = client.health(&address).await.map_err(AddError::Remote)?;
    let health = read_health(&body, own_instance).map_err(AddError::Invalid)?;
    let answer = client
        .pair(&address, code, own_label)
        .await
        .map_err(AddError::Remote)?;
    let address = match answer.external_address.as_deref() {
        Some(external) if external != address => {
            preferred_address(client, &address, external, &health.instance_id).await
        }
        _ => address,
    };
    let gateway = RemoteGateway {
        name,
        address,
        pairing_id: answer.pairing_id,
        credential: answer.credential,
        gateway_protocol: health.gateway_protocol,
        added_at: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = store.add(gateway.clone()) {
        // B already granted access, so give the grant back rather than leave
        // one nobody holds.
        let _ = client.leave(&gateway).await;
        return Err(e);
    }
    crate::log!(
        "[Gateway] paired with remote gateway '{}' at {}",
        gateway.name,
        gateway.address
    );
    Ok(gateway.into())
}

/// B's external address when A reaches this same B there, else `typed`.
async fn preferred_address(
    client: &PeerClient,
    typed: &str,
    external: &str,
    instance_id: &str,
) -> String {
    let Ok(external) = valid_address(external) else {
        return typed.to_string();
    };
    let same_gateway = client.health(&external).await.is_ok_and(|body| {
        body.get("instance_id").and_then(serde_json::Value::as_str) == Some(instance_id)
    });
    if same_gateway {
        external
    } else {
        typed.to_string()
    }
}

/// What this gateway calls itself to B: its machine's name.
fn own_label() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its whole length, and gethostname
    // writes at most that many bytes.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    let host = if rc == 0 {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).trim().to_string()
    } else {
        String::new()
    };
    if host.is_empty() {
        "Lucidos gateway".to_string()
    } else {
        format!("Lucidos on {host}")
    }
}

/// A's routes, nested at `/~/api/v1/remote-gateways`, device-gated by
/// `auth_api::enforce` and closed to app documents.
pub fn router() -> Router<GatewayState> {
    Router::new()
        .route("/", get(list).post(add_handler))
        .route("/:name", delete(remove_handler))
        .route_layer(middleware::from_fn(crate::control::control_authz))
}

async fn list(State(state): State<GatewayState>) -> Json<Vec<RemoteGatewayRow>> {
    Json(
        state
            .remote_gateways()
            .list()
            .into_iter()
            .map(Into::into)
            .collect(),
    )
}

async fn add_handler(
    State(state): State<GatewayState>,
    Json(request): Json<AddRequest>,
) -> Result<Json<RemoteGatewayRow>, ApiError> {
    let client = PeerClient::new().map_err(ApiError::internal)?;
    let row = add(
        state.remote_gateways(),
        &client,
        crate::server::instance_id(),
        &own_label(),
        request,
    )
    .await?;
    Ok(Json(row))
}

#[derive(Serialize)]
struct Removed {
    /// Did B forget the pairing too? `false` leaves a grant on B its user can
    /// Unpair there.
    remote_forgot: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

async fn remove_handler(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<Json<Removed>, ApiError> {
    let gateway = state
        .remote_gateways()
        .remove(&name)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("no remote gateway with that name"))?;
    let left = match PeerClient::new() {
        Ok(client) => client.leave(&gateway).await,
        Err(e) => Err(e),
    };
    Ok(Json(Removed {
        remote_forgot: left.is_ok(),
        detail: left.err(),
    }))
}

#[cfg(test)]
#[path = "remote_gateways_tests.rs"]
pub(crate) mod tests;
