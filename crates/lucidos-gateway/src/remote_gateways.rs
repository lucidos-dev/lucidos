//! Remote gateways, from the side that pairs: gateway A.
//!
//! A's user types B's address, a gateway pairing secret B minted, and a short
//! name. A checks B's public health, redeems the code over `https`, and keeps
//! the gateway credential B returned. B's half is [`crate::gateway_protocol`],
//! and ADR 0590 records the decision.
//!
//! The credential crosses a network, so every call here is `https` only, with
//! certificates checked and redirects refused.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use axum::extract::State;
use axum::middleware;
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth;
use crate::error::ApiError;
use crate::pairing::{self, PairingKind};
use crate::peer_connection::PeerConnection;
use crate::peers::GATEWAY_ROLE;
use crate::server::GatewayState;
use lucidos_gateway_constants::REMOTE_GATEWAY_NAME_MAX_LEN;

/// Validated proxy headers. Debug never prints values.
#[derive(Clone, Default, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct ExtraHeaders(std::collections::BTreeMap<String, String>);

impl std::fmt::Debug for ExtraHeaders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}
impl<'de> Deserialize<'de> for ExtraHeaders {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(Deserialize::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
impl ExtraHeaders {
    pub fn new(values: std::collections::BTreeMap<String, String>) -> Result<Self, String> {
        let mut normalized = std::collections::BTreeMap::new();
        for (name, value) in values {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| "invalid proxy header name")?;
            let name = name.as_str();
            if name.starts_with("x-lucidos-")
                || matches!(
                    name,
                    "host"
                        | "connection"
                        | "keep-alive"
                        | "proxy-connection"
                        | "proxy-authenticate"
                        | "proxy-authorization"
                        | "te"
                        | "trailer"
                        | "transfer-encoding"
                        | "upgrade"
                        | "content-length"
                        | "content-type"
                )
            {
                return Err(format!("the header {name} is reserved"));
            }
            reqwest::header::HeaderValue::from_str(&value)
                .map_err(|_| "invalid proxy header value")?;
            if normalized.insert(name.to_owned(), value).is_some() {
                return Err(format!("duplicate proxy header {name}"));
            }
        }
        Ok(Self(normalized))
    }
    pub(crate) fn apply(&self, mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        for (name, value) in &self.0 {
            let mut value =
                reqwest::header::HeaderValue::from_str(value).expect("validated header value");
            value.set_sensitive(true);
            request = request.header(name, value);
        }
        request
    }
    pub(crate) fn names(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

struct HeaderContext<'a>(&'a str, &'a str);
impl lucidos_secret_store::SealingContext for HeaderContext<'_> {
    fn name(&self) -> String {
        format!("remote-gateway.{}.header.{}", self.0, self.1)
    }
    fn empty_means_none(&self) -> bool {
        false
    }
}

/// One remote gateway, as A persists it.
///
/// Credentials are plaintext only in memory; the disk representation is sealed.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteGateway {
    pub name: String,
    pub address: String,
    pub pairing_id: String,
    pub credential: String,
    #[serde(default)]
    pub headers: ExtraHeaders,
    pub gateway_protocol: u32,
    pub added_at: String,
    #[serde(default)]
    pub rotation: RotationState,
}

/// Durable sender recovery. Pending credentials never leave the sealed store.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RotationState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pending: Option<String>,
    pub last_rotated_at: Option<String>,
    pub error: Option<String>,
}

struct PendingContext<'a>(&'a str);
impl lucidos_secret_store::SealingContext for PendingContext<'_> {
    fn name(&self) -> String {
        format!("remote-gateway.{}.pending-rotation", self.0)
    }
    fn empty_means_none(&self) -> bool {
        false
    }
}

impl std::fmt::Debug for RemoteGateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteGateway")
            .field("name", &self.name)
            .field("address", &self.address)
            .field("credential", &"[redacted]")
            .finish_non_exhaustive()
    }
}

struct CredentialContext<'a>(&'a str);
impl lucidos_secret_store::SealingContext for CredentialContext<'_> {
    fn name(&self) -> String {
        format!("remote-gateway.{}.credential", self.0)
    }
    fn empty_means_none(&self) -> bool {
        false
    }
}

/// A's remote-gateway file, `<data dir>/remote-gateways.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteGateways {
    #[serde(default)]
    pub gateways: Vec<RemoteGateway>,
}

/// A missing version is the legacy plaintext format, accepted only for upgrade.
#[derive(Default, Serialize, Deserialize)]
struct StoredRemoteGateways {
    #[serde(default)]
    version: Option<u32>,
    #[serde(flatten)]
    data: RemoteGateways,
}

/// Per gateway, beside the device store and never shared (ADR 0132).
pub fn remote_gateways_path(app_data: &Path) -> PathBuf {
    app_data.join("remote-gateways.json")
}

/// A's remote gateways, with the locking shape of `PairedGatewayStore`.
pub struct RemoteGatewayStore {
    path: PathBuf,
    key_store: Box<dyn lucidos_secret_store::key_store::KeyStore>,
    key_name: String,
    data: Mutex<RemoteGateways>,
    writing: Mutex<()>,
    operations: tokio::sync::Mutex<()>,
    pub(crate) last_seen: Mutex<std::collections::HashMap<String, String>>,
}

impl RemoteGatewayStore {
    pub fn load(path: PathBuf) -> Result<Self, crate::BoxError> {
        #[cfg(not(test))]
        let keys = lucidos_secret_store::key_store::resolve()?;
        // Each test gateway owns its key directory; no test touches the user's keys.
        #[cfg(test)]
        let keys = Box::new(lucidos_secret_store::key_store::FileKeyStore::new(
            path.parent()
                .ok_or("remote store has no parent")?
                .join("test-secret-keys"),
        ));
        Self::load_with_key_store(path, keys)
    }

    fn load_with_key_store(
        path: PathBuf,
        key_store: Box<dyn lucidos_secret_store::key_store::KeyStore>,
    ) -> Result<Self, crate::BoxError> {
        use lucidos_secret_store::SEALED_PREFIX;
        use sha2::{Digest, Sha256};
        let stored: StoredRemoteGateways = auth::load_store(&path)?;
        if stored.version.is_some_and(|version| version != 1) {
            return Err("unsupported remote gateway store version".into());
        }
        let is_sealed_format = stored.version == Some(1);
        let mut data = stored.data;
        let identity = std::path::absolute(&path)?;
        let key_name = format!(
            "gateway-remotes-{:x}",
            Sha256::digest(identity.as_os_str().as_encoded_bytes())
        );
        let has_sealed = data
            .gateways
            .iter()
            .any(|g| g.credential.starts_with(SEALED_PREFIX));
        let key = key_store.get(&key_name)?;
        if has_sealed && key.is_none() {
            return Err("remote gateway secrets need their original sealing key; restore it before starting".into());
        }
        let mut migrate = false;
        for remote in &mut data.gateways {
            if is_sealed_format || remote.credential.starts_with(SEALED_PREFIX) {
                remote.credential = key
                    .as_ref()
                    .ok_or("remote sealing key is missing")?
                    .open(&CredentialContext(&remote.pairing_id), &remote.credential)?;
            } else {
                migrate = true;
            }
            if let Some(candidate) = &mut remote.rotation.pending {
                *candidate = key
                    .as_ref()
                    .ok_or("remote sealing key is missing")?
                    .open(&PendingContext(&remote.pairing_id), candidate)?;
            }
            for (name, value) in &mut remote.headers.0 {
                *value = key
                    .as_ref()
                    .ok_or("remote sealing key is missing")?
                    .open(&HeaderContext(&remote.pairing_id, name), value)?;
            }
            // Validate decrypted values too, before any request can use them.
            remote.headers = ExtraHeaders::new(remote.headers.0.clone())?;
        }
        let store = Self {
            path,
            key_store,
            key_name,
            data: Mutex::new(data.clone()),
            writing: Mutex::new(()),
            operations: tokio::sync::Mutex::new(()),
            last_seen: Mutex::new(Default::default()),
        };
        if migrate {
            store.save_sealed(&data)?;
        }
        Ok(store)
    }

    fn save_sealed(&self, data: &RemoteGateways) -> Result<(), crate::BoxError> {
        use lucidos_secret_store::SecretKey;
        let key = match self.key_store.get(&self.key_name)? {
            Some(key) => key,
            None => {
                // Never replace a key if the existing file needs one.
                let disk: RemoteGateways = auth::load_store(&self.path)?;
                if disk.gateways.iter().any(|g| {
                    g.credential
                        .starts_with(lucidos_secret_store::SEALED_PREFIX)
                }) {
                    return Err(
                        "remote gateway sealing key is missing; restore the original key".into(),
                    );
                }
                self.key_store
                    .put_new(&self.key_name, &SecretKey::generate())?;
                self.key_store
                    .get(&self.key_name)?
                    .ok_or("remote gateway sealing key could not be stored")?
            }
        };
        let mut sealed = data.clone();
        for remote in &mut sealed.gateways {
            remote.credential =
                key.seal(&CredentialContext(&remote.pairing_id), &remote.credential)?;
            if let Some(candidate) = &mut remote.rotation.pending {
                *candidate = key.seal(&PendingContext(&remote.pairing_id), candidate)?;
            }
            for (name, value) in &mut remote.headers.0 {
                *value = key.seal(&HeaderContext(&remote.pairing_id, name), value)?;
            }
        }
        auth::save_store(
            &StoredRemoteGateways {
                version: Some(1),
                data: sealed,
            },
            &self.path,
        )
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
        if let Some(gateway) = &removed {
            self.last_seen
                .lock()
                .unwrap()
                .remove(&format!("{}@{}", gateway.pairing_id, gateway.address));
        }
        Ok(removed)
    }

    fn begin_rotation(&self, name: &str) -> Result<RemoteGateway, String> {
        let mut result = None;
        let candidate =
            auth::mint_credential().map_err(|_| "could not generate rotation credential")?;
        self.write(|data| {
            if let Some(remote) = data.gateways.iter_mut().find(|g| g.name == name) {
                remote.rotation.pending.get_or_insert(candidate);
                result = Some(remote.clone());
            }
        })?;
        result.ok_or_else(|| "remote gateway not found".into())
    }

    /// Hold across network I/O: header replacement and unpair use this same gate.
    pub async fn rotate(&self, client: &PeerConnection, name: &str) -> Result<(), String> {
        let _operation = self.operations.lock().await;
        let remote = self.begin_rotation(name)?;
        let candidate = remote
            .rotation
            .pending
            .as_ref()
            .ok_or("rotation intent missing")?;
        let outcome = client.rotate(&remote, candidate).await;
        self.write(|data| {
            if let Some(current) = data.gateways.iter_mut().find(|g| g.name == name) {
                match &outcome {
                    Ok(protocol) => {
                        current.credential = candidate.clone();
                        current.rotation.pending = None;
                        current.rotation.last_rotated_at = Some(chrono::Utc::now().to_rfc3339());
                        current.rotation.error = None;
                        current.gateway_protocol = *protocol;
                    }
                    Err(error) => current.rotation.error = Some(error.clone()),
                }
            }
        })?;
        outcome.map(|_| ())
    }

    pub async fn rotate_due(&self, client: &PeerConnection) {
        for remote in self.list() {
            let since = remote
                .rotation
                .last_rotated_at
                .as_ref()
                .unwrap_or(&remote.added_at);
            let due = chrono::DateTime::parse_from_rfc3339(since)
                .map(|at| chrono::Utc::now().signed_duration_since(at).num_days() >= 30)
                .unwrap_or(true);
            if due || remote.rotation.pending.is_some() {
                if let Err(error) = self.rotate(client, &remote.name).await {
                    crate::log!(
                        "[Gateway] remote credential rotation for '{}': {error}",
                        remote.name
                    );
                }
            }
        }
    }

    fn write(&self, mutate: impl FnOnce(&mut RemoteGateways)) -> Result<(), String> {
        fn unavailable<T>(_: T) -> String {
            "remote gateway store is unavailable".to_string()
        }
        let _writing = self.writing.lock().map_err(unavailable)?;
        // Save first, publish after: a failed save changes nothing anyone sees.
        let mut next = self.data.lock().map_err(unavailable)?.clone();
        mutate(&mut next);
        self.save_sealed(&next)
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

/// What the user typed to add a remote gateway.
#[derive(Deserialize)]
pub struct AddRequest {
    pub address: String,
    pub code: String,
    pub name: String,
    #[serde(default)]
    pub headers: ExtraHeaders,
}

/// A remote gateway as the API shows it. Never the credential.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RemoteGatewayRow {
    pub header_names: Vec<String>,
    pub name: String,
    pub address: String,
    pub pairing_id: String,
    pub gateway_protocol: u32,
    pub added_at: String,
    pub last_rotated_at: Option<String>,
    pub rotation_error: Option<String>,
}

impl From<RemoteGateway> for RemoteGatewayRow {
    fn from(g: RemoteGateway) -> Self {
        Self {
            header_names: g.headers.names(),
            name: g.name,
            address: g.address,
            pairing_id: g.pairing_id,
            gateway_protocol: g.gateway_protocol,
            added_at: g.added_at,
            last_rotated_at: g.rotation.last_rotated_at,
            rotation_error: g.rotation.error,
        }
    }
}

/// Pair with B and record it.
///
/// The name, the address and the secret's shape are checked before the secret
/// is spent, so a typo burns no secret. B's external address then replaces the
/// typed one only when A reaches this same B there and no proxy headers bind
/// the pairing to the typed origin.
pub async fn add(
    store: &RemoteGatewayStore,
    client: &PeerConnection,
    own_instance: &str,
    own_label: &str,
    request: AddRequest,
) -> Result<RemoteGatewayRow, AddError> {
    let name = valid_name(&request.name).map_err(AddError::Invalid)?;
    if store.contains(&name) {
        return Err(AddError::NameTaken);
    }
    let address = valid_address(&request.address).map_err(AddError::Invalid)?;
    // A short code is still taken for an older B, which mints nothing else.
    let code = pairing::valid_pairing_secret(&request.code, PairingKind::Gateway)
        .or_else(|| pairing::valid_pairing_code(&request.code))
        .ok_or_else(|| {
            AddError::Invalid(
                "paste the gateway pairing secret the other gateway shows, starting lgp_".into(),
            )
        })?;
    let body = client
        .health_with_headers(&address, &request.headers)
        .await
        .map_err(AddError::Remote)?;
    let health = read_health(&body, own_instance).map_err(AddError::Invalid)?;
    let answer = client
        .pair_with_headers(&address, code, own_label, &request.headers)
        .await
        .map_err(AddError::Remote)?;
    let address = match answer.external_address.as_deref() {
        Some(external) if external != address && request.headers.is_empty() => {
            preferred_address(client, &address, external, &health.instance_id).await
        }
        _ => address,
    };
    let gateway = RemoteGateway {
        name,
        address,
        pairing_id: answer.pairing_id,
        credential: answer.credential,
        headers: request.headers,
        gateway_protocol: health.gateway_protocol,
        added_at: chrono::Utc::now().to_rfc3339(),
        rotation: RotationState::default(),
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
    client: &PeerConnection,
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
        .route("/:name", delete(remove_handler).put(update_headers_handler))
        .route("/:name/rotate", axum::routing::post(rotate_handler))
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
    let row = add(
        state.remote_gateways(),
        state.peer_connection(),
        crate::server::instance_id(),
        &own_label(),
        request,
    )
    .await?;
    Ok(Json(row))
}

#[derive(Deserialize)]
struct UpdateHeadersRequest {
    headers: ExtraHeaders,
}

async fn update_headers_handler(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(request): Json<UpdateHeadersRequest>,
) -> Result<Json<RemoteGatewayRow>, ApiError> {
    let _operation = state.remote_gateways().operations.lock().await;
    let mut updated = None;
    state
        .remote_gateways()
        .write(|data| {
            if let Some(remote) = data.gateways.iter_mut().find(|g| g.name == name) {
                remote.headers = request.headers;
                updated = Some(remote.clone());
            }
        })
        .map_err(ApiError::internal)?;
    updated
        .map(|g| Json(g.into()))
        .ok_or_else(|| ApiError::not_found("remote gateway not found"))
}

async fn rotate_handler(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<axum::http::StatusCode, ApiError> {
    state
        .remote_gateways()
        .rotate(state.peer_connection(), &name)
        .await
        .map_err(ApiError::internal)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
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
    let _operation = state.remote_gateways().operations.lock().await;
    let gateway = state
        .remote_gateways()
        .remove(&name)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("no remote gateway with that name"))?;
    // A pending intent may already be current on B, even if A's save failed.
    let left = if let Some(candidate) = &gateway.rotation.pending {
        let mut proposed = gateway.clone();
        proposed.credential = candidate.clone();
        match state.peer_connection().leave(&proposed).await {
            Ok(()) => Ok(()),
            Err(_) => state.peer_connection().leave(&gateway).await,
        }
    } else {
        state.peer_connection().leave(&gateway).await
    };
    Ok(Json(Removed {
        remote_forgot: left.is_ok(),
        detail: left.err(),
    }))
}

#[cfg(test)]
#[path = "remote_gateways_tests.rs"]
pub(crate) mod tests;
