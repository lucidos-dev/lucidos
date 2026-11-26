use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use axum::extract::State;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth;
use crate::error::ApiError;
use crate::pairing::{self, PairingKind};
use crate::peer_connection::{PairReply, PeerConnection};
use crate::peers::GATEWAY_ROLE;
use crate::server::GatewayState;
use lucidos_gateway_constants::REMOTE_GATEWAY_NAME_MAX_LEN;

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
        lucidos_diagnostic_log::sanitize::register_secret_values(normalized.values().cloned());
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

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteGateway {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_id: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) return_access: Option<crate::return_access::ReturnGrant>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) trusted_pairing: bool,
}

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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteGateways {
    #[serde(default)]
    pub gateways: Vec<RemoteGateway>,
}

#[derive(Default, Serialize, Deserialize)]
struct StoredRemoteGateways {
    #[serde(default)]
    version: Option<u32>,
    #[serde(flatten)]
    data: RemoteGateways,
}

pub fn remote_gateways_path(app_data: &Path) -> PathBuf {
    app_data.join("remote-gateways.json")
}

pub struct RemoteGatewayStore {
    path: PathBuf,
    key_store: Box<dyn lucidos_secret_store::key_store::KeyStore>,
    key_name: String,
    data: Mutex<RemoteGateways>,
    writing: Mutex<()>,
    operations: tokio::sync::Mutex<()>,
    pub(crate) last_seen: Mutex<std::collections::HashMap<String, String>>,
    generation: std::sync::atomic::AtomicU64,
}

impl RemoteGatewayStore {
    pub fn load(path: PathBuf) -> Result<Self, crate::BoxError> {
        #[cfg(not(test))]
        let keys = lucidos_secret_store::key_store::resolve()?;
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
            remote.headers = ExtraHeaders::new(remote.headers.0.clone())?;
        }
        register_secrets(&data);
        let store = Self {
            path,
            key_store,
            key_name,
            data: Mutex::new(data.clone()),
            writing: Mutex::new(()),
            operations: tokio::sync::Mutex::new(()),
            last_seen: Mutex::new(Default::default()),
            generation: Default::default(),
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

    pub fn add(&self, gateway: RemoteGateway) -> Result<(), AddError> {
        let mut rejection = None;
        self.write(|store| {
            if store.gateways.iter().any(|g| g.name == gateway.name) {
                rejection = Some(AddError::NameTaken);
            } else if let Some(existing) = store
                .gateways
                .iter()
                .find(|g| gateway.gateway_id.is_some() && g.gateway_id == gateway.gateway_id)
            {
                rejection = Some(AddError::Duplicate(existing.name.clone()));
            } else {
                store.gateways.push(gateway);
            }
        })
        .map_err(AddError::Internal)?;
        rejection.map_or(Ok(()), Err)
    }

    pub fn replace(
        &self,
        expected: &RemoteGateway,
        renewed: RemoteGateway,
    ) -> Result<(), AddError> {
        let moved = || {
            AddError::Invalid(format!(
                "{} now answers as a different gateway than '{}'. Remove '{}' and pair that gateway as a new one",
                expected.address, expected.name, expected.name
            ))
        };
        if expected
            .gateway_id
            .as_ref()
            .is_some_and(|saved| renewed.gateway_id.as_ref() != Some(saved))
        {
            return Err(moved());
        }
        let mut rejection = Some(AddError::Missing);
        self.write(|store| {
            if let Some(other) = store.gateways.iter().find(|g| {
                g.name != expected.name
                    && renewed.gateway_id.is_some()
                    && g.gateway_id == renewed.gateway_id
            }) {
                rejection = Some(AddError::Duplicate(other.name.clone()));
            } else if let Some(current) = store.gateways.iter_mut().find(|g| *g == expected) {
                *current = renewed;
                rejection = None;
            }
        })
        .map_err(AddError::Internal)?;
        if rejection.is_none() {
            self.last_seen
                .lock()
                .unwrap()
                .remove(&format!("{}@{}", expected.pairing_id, expected.address));
        }
        rejection.map_or(Ok(()), Err)
    }

    #[cfg(test)]
    pub(crate) fn set_return_access(
        &self,
        name: &str,
        enabled: bool,
    ) -> Result<Option<RemoteGateway>, String> {
        let grant = if enabled {
            Some(crate::return_access::ReturnGrant {
                id: auth::mint_credential().map_err(|e| e.to_string())?,
            })
        } else {
            None
        };
        self.persist_return_grant(name, grant)
    }

    fn persist_return_grant(
        &self,
        name: &str,
        grant: Option<crate::return_access::ReturnGrant>,
    ) -> Result<Option<RemoteGateway>, String> {
        let mut updated = None;
        self.write(|store| {
            if let Some(remote) = store.gateways.iter_mut().find(|g| g.name == name) {
                remote.return_access = grant;
                updated = Some(remote.clone());
            }
        })?;
        Ok(updated)
    }

    pub(crate) fn set_trusted_pairing(
        &self,
        name: &str,
        trusted: bool,
    ) -> Result<Option<RemoteGateway>, String> {
        let mut updated = None;
        self.write(|store| {
            if let Some(remote) = store.gateways.iter_mut().find(|g| g.name == name) {
                remote.trusted_pairing = trusted;
                updated = Some(remote.clone());
            }
        })?;
        Ok(updated)
    }

    pub(crate) fn confirm_destination(
        &self,
        expected: &RemoteGateway,
        identity: &str,
    ) -> Result<RemoteGateway, String> {
        if expected
            .gateway_id
            .as_deref()
            .is_some_and(|saved| saved != identity)
        {
            return Err(
                "The remote gateway identity changed. Pair it again before routing operations."
                    .into(),
            );
        }
        if expected.gateway_id.is_some() {
            return self
                .list()
                .into_iter()
                .find(|g| g == expected)
                .ok_or_else(|| {
                    "The outgoing pairing changed while its identity was checked.".into()
                });
        }
        let mut confirmed = None;
        self.write(|data| {
            if data
                .gateways
                .iter()
                .any(|g| g.name != expected.name && g.gateway_id.as_deref() == Some(identity))
            {
                return;
            }
            if let Some(remote) = data.gateways.iter_mut().find(|g| *g == expected) {
                remote.gateway_id = Some(identity.into());
                confirmed = Some(remote.clone());
            }
        })?;
        confirmed.ok_or_else(|| {
            "The pairing changed or its destination duplicates another gateway.".into()
        })
    }

    pub fn set_protocol(&self, name: &str, protocol: u32) -> Result<(), String> {
        self.write(|store| {
            if let Some(remote) = store.gateways.iter_mut().find(|g| g.name == name) {
                remote.gateway_protocol = remote.gateway_protocol.max(protocol);
            }
        })
    }

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
                    crate::log_legacy!(
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
        let mut next = self.data.lock().map_err(unavailable)?.clone();
        mutate(&mut next);
        register_secrets(&next);
        self.save_sealed(&next)
            .map_err(|e| format!("could not save the remote gateway store: {e}"))?;
        *self.data.lock().map_err(unavailable)? = next;
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AddError {
    Invalid(String),
    NameTaken,
    Duplicate(String),
    Remote(String),
    AwaitingOwner,
    Missing,
    Internal(String),
}

impl IntoResponse for AddError {
    fn into_response(self) -> Response {
        match self {
            AddError::Invalid(m) => ApiError::bad_request(m).into_response(),
            AddError::NameTaken => {
                ApiError::conflict("another remote gateway already has that name").into_response()
            }
            AddError::Duplicate(name) => ApiError::conflict(format!(
                "that gateway is already paired as '{name}'; use the existing remote entry"
            ))
            .into_response(),
            AddError::Remote(m) => ApiError::bad_gateway(m).into_response(),
            AddError::AwaitingOwner => crate::pairing_requests::awaiting_owner(),
            AddError::Missing => {
                ApiError::not_found("no remote gateway with that name").into_response()
            }
            AddError::Internal(m) => ApiError::internal(m).into_response(),
        }
    }
}

pub fn valid_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.len() <= REMOTE_GATEWAY_NAME_MAX_LEN && crate::registry::is_valid_id(name) {
        return Ok(name.to_string());
    }
    Err(format!(
        "a short name is up to {REMOTE_GATEWAY_NAME_MAX_LEN} lowercase letters, digits and dashes, such as cloud"
    ))
}

pub fn valid_address(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let origin = crate::pairing_qr::valid_origin(trimmed).ok_or_else(|| {
        format!(
            "'{}' is not an address. Use https, a host and an optional port, such as \
             https://lucidos.example.com, with no path",
            raw.trim()
        )
    })?;
    if origin.starts_with("http://") && !is_loopback_origin(origin) {
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

pub fn is_loopback_origin(origin: &str) -> bool {
    let Some(host) = reqwest::Url::parse(origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
    else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.to_canonical().is_loopback())
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteHealth {
    pub instance_id: String,
    pub gateway_protocol: u32,
}

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

#[derive(Deserialize)]
pub struct AddRequest {
    pub address: String,
    pub code: String,
    pub name: String,
    #[serde(default)]
    pub headers: ExtraHeaders,
}

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
    pub return_access: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_access_warning: Option<String>,
    pub trusted_pairing: bool,
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
            return_access: g.return_access.is_some(),
            return_access_warning: None,
            trusted_pairing: g.trusted_pairing,
        }
    }
}

pub async fn add(
    store: &RemoteGatewayStore,
    client: &PeerConnection,
    own_instance: &str,
    own_label: &str,
    request: AddRequest,
) -> Result<RemoteGatewayRow, AddError> {
    let _operation = store.operations.lock().await;
    let name = valid_name(&request.name).map_err(AddError::Invalid)?;
    if store.contains(&name) {
        return Err(AddError::NameTaken);
    }
    let address = valid_address(&request.address).map_err(AddError::Invalid)?;
    for existing in store.list() {
        if valid_address(&existing.address).is_ok_and(|saved| saved == address) {
            return Err(AddError::Duplicate(existing.name));
        }
    }
    let code = valid_secret(&request.code)?;
    let body = client
        .health_with_headers(&address, &request.headers)
        .await
        .map_err(AddError::Remote)?;
    let health = read_health(&body, own_instance).map_err(AddError::Invalid)?;
    let evidence = destination_evidence(store, client, &health, own_instance, None).await?;
    let answer = match client
        .pair_with_headers(&address, code, own_label, &request.headers)
        .await
        .map_err(AddError::Remote)?
    {
        PairReply::Paired(answer) => answer,
        PairReply::AwaitingOwner => return Err(AddError::AwaitingOwner),
    };
    let address = match answer.external_address.as_deref() {
        Some(external) if external != address && request.headers.is_empty() => {
            preferred_address(client, &address, external, &health.instance_id).await
        }
        _ => address,
    };
    let gateway = RemoteGateway {
        gateway_id: answer.gateway_id.filter(|id| !id.trim().is_empty()),
        name,
        address,
        pairing_id: answer.pairing_id,
        credential: answer.credential,
        headers: request.headers,
        gateway_protocol: health.gateway_protocol,
        added_at: chrono::Utc::now().to_rfc3339(),
        rotation: RotationState::default(),
        return_access: None,
        trusted_pairing: false,
    };
    let unique = distinct_from(&evidence, &gateway);
    if let Err(error) = unique.and_then(|()| store.add(gateway.clone())) {
        return Err(give_back(client, &gateway, error).await);
    }
    crate::log_legacy!(
        "[Gateway] paired with remote gateway '{}' at {}",
        gateway.name,
        gateway.address
    );
    Ok(gateway.into())
}

fn valid_secret(raw: &str) -> Result<&str, AddError> {
    pairing::valid_pairing_secret(raw, PairingKind::Gateway)
        .or_else(|| pairing::valid_pairing_code(raw))
        .ok_or_else(|| {
            AddError::Invalid(
                "paste the gateway pairing secret the other gateway shows, starting lgp_".into(),
            )
        })
}

pub async fn repair(
    store: &RemoteGatewayStore,
    client: &PeerConnection,
    own_instance: &str,
    own_label: &str,
    name: &str,
    code: &str,
) -> Result<(RemoteGateway, RemoteGateway), AddError> {
    let existing = store
        .list()
        .into_iter()
        .find(|g| g.name == name)
        .ok_or(AddError::Missing)?;
    let code = valid_secret(code)?;
    let body = client
        .health_with_headers(&existing.address, &existing.headers)
        .await
        .map_err(AddError::Remote)?;
    let health = read_health(&body, own_instance).map_err(AddError::Invalid)?;
    let evidence = match existing.gateway_id {
        Some(_) => Vec::new(),
        None => {
            destination_evidence(store, client, &health, own_instance, Some(&existing.name)).await?
        }
    };
    let answer = match client
        .pair_with_headers(&existing.address, code, own_label, &existing.headers)
        .await
        .map_err(AddError::Remote)?
    {
        PairReply::Paired(answer) => answer,
        PairReply::AwaitingOwner => return Err(AddError::AwaitingOwner),
    };
    let renewed = RemoteGateway {
        gateway_id: answer.gateway_id.filter(|id| !id.trim().is_empty()),
        pairing_id: answer.pairing_id,
        credential: answer.credential,
        gateway_protocol: health.gateway_protocol,
        rotation: RotationState {
            last_rotated_at: Some(chrono::Utc::now().to_rfc3339()),
            ..RotationState::default()
        },
        return_access: None,
        trusted_pairing: false,
        ..existing.clone()
    };
    let unique = distinct_from(&evidence, &renewed);
    if let Err(error) = unique.and_then(|()| store.replace(&existing, renewed.clone())) {
        return Err(give_back(client, &renewed, error).await);
    }
    crate::log_legacy!(
        "[Gateway] paired again with remote gateway '{}' at {}",
        renewed.name,
        renewed.address
    );
    Ok((renewed, existing))
}

struct DestinationEvidence {
    name: String,
    gateway_id: Option<String>,
    distinct_boot: bool,
}

fn distinct_from(
    evidence: &[DestinationEvidence],
    gateway: &RemoteGateway,
) -> Result<(), AddError> {
    evidence.iter().try_for_each(
        |existing| match (&existing.gateway_id, &gateway.gateway_id) {
            (Some(saved), Some(new)) if saved == new => {
                Err(AddError::Duplicate(existing.name.clone()))
            }
            (Some(_), Some(_)) => Ok(()),
            _ if existing.distinct_boot => Ok(()),
            _ => Err(ambiguous_destination(&existing.name)),
        },
    )
}

fn ambiguous_destination(name: &str) -> AddError {
    AddError::Invalid(format!(
        "cannot establish whether this is already paired as '{name}'; reconnect or update that gateway, then try again"
    ))
}

async fn destination_evidence(
    store: &RemoteGatewayStore,
    client: &PeerConnection,
    candidate: &RemoteHealth,
    own_instance: &str,
    except: Option<&str>,
) -> Result<Vec<DestinationEvidence>, AddError> {
    let mut evidence = Vec::new();
    for mut remote in store.list() {
        if except == Some(remote.name.as_str()) {
            continue;
        }
        let health = match client
            .health_with_headers(&remote.address, &remote.headers)
            .await
        {
            Ok(body) => read_health(&body, own_instance).ok(),
            Err(_) => None,
        };
        if health
            .as_ref()
            .is_some_and(|h| h.instance_id == candidate.instance_id)
        {
            return Err(AddError::Duplicate(remote.name));
        }
        if remote.gateway_id.is_none() {
            if let Ok(Some(identity)) = client.destination_identity(&remote).await {
                store
                    .write(|data| {
                        if let Some(saved) =
                            data.gateways.iter_mut().find(|g| g.name == remote.name)
                        {
                            saved.gateway_id = Some(identity.clone());
                        }
                    })
                    .map_err(AddError::Internal)?;
                remote.gateway_id = Some(identity);
            }
        }
        if remote.gateway_id.is_none() && health.is_none() {
            return Err(ambiguous_destination(&remote.name));
        }
        evidence.push(DestinationEvidence {
            name: remote.name,
            gateway_id: remote.gateway_id,
            distinct_boot: health.is_some(),
        });
    }
    Ok(evidence)
}

async fn give_back(client: &PeerConnection, gateway: &RemoteGateway, error: AddError) -> AddError {
    match client.leave(gateway).await {
        Ok(()) => error,
        Err(cleanup) => {
            let reason = match error {
                AddError::Duplicate(name) => format!("already paired as '{name}'"),
                AddError::NameTaken => "the name is already taken".into(),
                AddError::Invalid(message)
                | AddError::Remote(message)
                | AddError::Internal(message) => message,
                AddError::AwaitingOwner => "waiting for the destination owner".into(),
                AddError::Missing => "the remote gateway was removed meanwhile".into(),
            };
            AddError::Internal(format!(
                "pairing was not saved ({reason}), and the new grant could not be revoked: {cleanup}. Unpair the new grant for this gateway on the destination's Access page before retrying"
            ))
        }
    }
}

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

pub fn router() -> Router<GatewayState> {
    Router::new()
        .route("/", get(list).post(add_handler))
        .route("/:name", delete(remove_handler).put(update_headers_handler))
        .route("/:name/rotate", axum::routing::post(rotate_handler))
        .route("/:name/repair", axum::routing::post(repair_handler))
        .route(
            "/:name/return-access",
            axum::routing::put(update_return_access_handler),
        )
        .route(
            "/:name/trusted-pairing",
            axum::routing::put(update_trusted_pairing_handler),
        )
        .route(
            "/:name/manage/*rest",
            any(crate::gateway_hop::forward_manage),
        )
        .route(
            "/:name/control/*rest",
            any(crate::gateway_hop::forward_control),
        )
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
) -> Result<Json<RemoteGatewayRow>, AddError> {
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToggleRequest {
    enabled: bool,
}

const RETURN_ACCESS: &str = "agent return access";

fn require_owner_device(
    state: &GatewayState,
    device: Option<&auth::AuthenticatedDevice>,
    headers: &axum::http::HeaderMap,
    what: &str,
) -> Result<(), ApiError> {
    if headers.contains_key(auth::HEADER_AGENT_ORIGIN_TOKEN)
        || !device.is_some_and(|device| {
            state
                .paired_devices()
                .devices
                .iter()
                .any(|d| d.id == device.id)
        })
    {
        return Err(ApiError::forbidden(format!(
            "Only a device paired with this gateway can change {what}."
        )));
    }
    Ok(())
}

fn return_callback_address(
    state: &GatewayState,
    headers: &axum::http::HeaderMap,
) -> Result<String, ApiError> {
    let origin = crate::net_config::external_address(state.gateway_port())
        .origin
        .or_else(|| {
            headers
                .get(axum::http::header::ORIGIN)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        });
    origin.as_deref().and_then(|origin| valid_address(origin).ok()).ok_or_else(|| ApiError::conflict(
        "Set this gateway's External address in Settings, Access so the other gateway can reach it for agent return access."))
}

async fn enable_return_access(
    state: &GatewayState,
    remote: RemoteGateway,
    device: Option<&auth::AuthenticatedDevice>,
    headers: &axum::http::HeaderMap,
) -> Result<Option<RemoteGateway>, ApiError> {
    let name = remote.name.clone();
    let address = return_callback_address(state, headers)?;
    let health = state
        .peer_connection()
        .health_with_headers(&remote.address, &remote.headers)
        .await
        .map_err(ApiError::bad_gateway)?;
    let protocol = health["gateway_protocol"]
        .as_u64()
        .and_then(|p| u32::try_from(p).ok())
        .unwrap_or_default();
    if protocol < crate::gateway_protocol::OPERATIONS_PROTOCOL {
        return Err(ApiError::conflict(format!(
            "Update {} before enabling agent return access.",
            remote.name
        )));
    }
    let identity = state.peer_connection().destination_identity(&remote).await.map_err(ApiError::bad_gateway)?
        .ok_or_else(|| ApiError::conflict("Update the remote gateway to establish its identity before enabling return access."))?;
    let remote = state
        .remote_gateways()
        .confirm_destination(&remote, &identity)
        .map_err(ApiError::conflict)?;
    state
        .remote_gateways()
        .set_protocol(&name, protocol)
        .map_err(ApiError::internal)?;
    let grant = crate::return_access::ReturnGrant {
        id: auth::mint_credential().map_err(|e| ApiError::internal(e.to_string()))?,
    };
    let route = crate::return_access::ReturnRoute {
        grant_id: grant.id.clone(),
        gateway_id: state.gateway_id().into(),
        address,
    };
    let response = state
        .peer_connection()
        .request(
            &remote,
            reqwest::Method::PUT,
            &format!(
                "{}/return-route",
                crate::gateway_protocol::GATEWAY_API_PREFIX
            ),
        )
        .map_err(ApiError::bad_gateway)?
        .timeout(std::time::Duration::from_secs(10))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&route).map_err(|e| ApiError::internal(e.to_string()))?)
        .send()
        .await
        .map_err(|_| {
            ApiError::bad_gateway(
                "Could not register return access with the remote gateway. Access was not enabled.",
            )
        })?;
    if !response.status().is_success() {
        return Err(ApiError::bad_gateway(format!("The remote gateway refused return-access registration with HTTP {}. Access was not enabled.", response.status().as_u16())));
    }
    require_owner_device(state, device, headers, RETURN_ACCESS)?;
    state
        .remote_gateways()
        .persist_return_grant(&name, Some(grant))
        .map_err(ApiError::internal)
}

async fn update_return_access_handler(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    device: Option<Extension<auth::AuthenticatedDevice>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<ToggleRequest>,
) -> Result<Json<RemoteGatewayRow>, ApiError> {
    let device = device.as_ref().map(|Extension(device)| device);
    require_owner_device(&state, device, &headers, RETURN_ACCESS)?;
    let _operation = state.remote_gateways().operations.lock().await;
    let remote = state
        .remote_gateways()
        .list()
        .into_iter()
        .find(|g| g.name == name)
        .ok_or_else(|| ApiError::not_found("Remote gateway not found."))?;
    let mut warning = None;
    let updated = if request.enabled {
        enable_return_access(&state, remote, device, &headers).await?
    } else {
        let updated = state
            .remote_gateways()
            .persist_return_grant(&name, None)
            .map_err(ApiError::internal)?;
        let cleared = state
            .peer_connection()
            .request(
                &remote,
                reqwest::Method::PUT,
                &format!(
                    "{}/return-route",
                    crate::gateway_protocol::GATEWAY_API_PREFIX
                ),
            )
            .map_err(ApiError::bad_gateway)?
            .timeout(std::time::Duration::from_secs(10))
            .header("content-type", "application/json")
            .body("null")
            .send()
            .await;
        if !matches!(cleared, Ok(ref response) if response.status().is_success()) {
            warning = Some(format!("Return access is off. {} could not remove its saved route; requests through it are refused.", remote.name));
        }
        updated
    };
    let mut row: RemoteGatewayRow = updated
        .ok_or_else(|| ApiError::not_found("Remote gateway no longer exists."))?
        .into();
    row.return_access_warning = warning;
    Ok(Json(row))
}

async fn update_trusted_pairing_handler(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    device: Option<Extension<auth::AuthenticatedDevice>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<ToggleRequest>,
) -> Result<Json<RemoteGatewayRow>, ApiError> {
    let device = device.as_ref().map(|Extension(device)| device);
    require_owner_device(&state, device, &headers, "trusted pairing")?;
    let _operation = state.remote_gateways().operations.lock().await;
    state
        .remote_gateways()
        .set_trusted_pairing(&name, request.enabled)
        .map_err(ApiError::internal)?
        .map(|g| Json(g.into()))
        .ok_or_else(|| ApiError::not_found("Remote gateway not found."))
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairRequest {
    code: String,
}

async fn repair_handler(
    State(state): State<GatewayState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    device: Option<Extension<auth::AuthenticatedDevice>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<RepairRequest>,
) -> Result<Json<RemoteGatewayRow>, AddError> {
    let _operation = state.remote_gateways().operations.lock().await;
    let (renewed, replaced) = repair(
        state.remote_gateways(),
        state.peer_connection(),
        crate::server::instance_id(),
        &own_label(),
        &name,
        &request.code,
    )
    .await?;
    let _ = leave_pairing(&state, &replaced).await;
    let device = device.as_ref().map(|Extension(device)| device);
    renew_return_access(&state, renewed, &replaced, device, &headers)
        .await
        .map(Json)
}

async fn renew_return_access(
    state: &GatewayState,
    renewed: RemoteGateway,
    replaced: &RemoteGateway,
    device: Option<&auth::AuthenticatedDevice>,
    headers: &axum::http::HeaderMap,
) -> Result<RemoteGatewayRow, AddError> {
    if replaced.return_access.is_none() {
        return Ok(renewed.into());
    }
    let mut row: RemoteGatewayRow = renewed.clone().into();
    let renewed_access = match require_owner_device(state, device, headers, RETURN_ACCESS) {
        Ok(()) => enable_return_access(state, renewed, device, headers).await,
        Err(refused) => Err(refused),
    };
    match renewed_access {
        Ok(Some(updated)) => row = updated.into(),
        Ok(None) => return Err(AddError::Missing),
        Err(refused) => {
            row.return_access_warning = Some(format!(
                "Paired again, but agent return access is off: {}. Turn it on again.",
                refused.message().trim_end_matches('.')
            ));
        }
    }
    Ok(row)
}

#[derive(Serialize)]
struct Removed {
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
    let left = leave_pairing(&state, &gateway).await;
    Ok(Json(Removed {
        remote_forgot: left.is_ok(),
        detail: left.err(),
    }))
}

async fn leave_pairing(state: &GatewayState, gateway: &RemoteGateway) -> Result<(), String> {
    if let Some(candidate) = &gateway.rotation.pending {
        let mut proposed = gateway.clone();
        proposed.credential = candidate.clone();
        if state.peer_connection().leave(&proposed).await.is_ok() {
            return Ok(());
        }
    }
    state.peer_connection().leave(gateway).await
}

fn register_secrets(data: &RemoteGateways) {
    for remote in &data.gateways {
        lucidos_diagnostic_log::sanitize::register_secret_values(
            std::iter::once(remote.credential.clone())
                .chain(remote.rotation.pending.clone())
                .chain(remote.headers.0.values().cloned()),
        );
    }
}

#[cfg(test)]
#[path = "remote_gateways_tests.rs"]
pub(crate) mod tests;
