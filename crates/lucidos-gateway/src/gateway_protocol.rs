use std::path::{Path, PathBuf};
use std::sync::Mutex;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::{self, ct_eq};
use crate::error::ApiError;
use crate::pairing::{
    self, Minter, OwnerGate, PairingAnswer, PairingChannel, PairingKind, PairingRequest,
    PendingPairings, Redemption,
};
use crate::server::GatewayState;
use crate::stack::Health;
use lucidos_gateway_stamp::remote_start::RemoteStartSetting;
use lucidos_gateway_stamp::WORKSPACE_CALLS_PATH;

pub const GATEWAY_PROTOCOL: u32 = 7;

pub const OPERATIONS_PROTOCOL: u32 = 5;

pub const MANAGE_PROTOCOL: u32 = 4;

/// The first protocol with the control hop, [`crate::gateway_hop::CONTROL`].
pub const CONTROL_PROTOCOL: u32 = 6;

/// The first protocol with the admin hop, [`crate::gateway_hop::ADMIN`].
pub const ADMIN_PROTOCOL: u32 = 7;

pub const ROTATE_PATH: &str = "/rotate";

pub const GATEWAY_API_PREFIX: &str = "/~/api/gateway/v1";

pub const GATEWAY_API_ROOT: &str = "/~/api/gateway";

pub const HEADER_GATEWAY_CREDENTIAL: &str = "x-lucidos-gateway-credential";

pub const PAIR_PATH: &str = "/pair";

pub const PAIRING_PATH: &str = "/pairing";

pub const WORKSPACES_PATH: &str = "/workspaces";

pub const MANAGE_PATH: &str = "/manage";

pub const CONTROL_PATH: &str = "/control";

pub const ADMIN_PATH: &str = "/admin";

pub fn is_gateway_api_path(path: &str) -> bool {
    path.strip_prefix(GATEWAY_API_ROOT)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedGateway {
    pub id: String,
    pub label: String,
    pub credential_digest: String,
    pub paired_at: String,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub previous_credential: Option<PreviousCredential>,
    #[serde(default)]
    pub may_manage: bool,
    #[serde(default)]
    pub may_control: bool,
    #[serde(default)]
    pub may_admin: bool,
    #[serde(default)]
    pub remote_starts: RemoteStartSetting,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) return_route: Option<crate::return_access::ReturnRoute>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreviousCredential {
    pub digest: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedGateways {
    #[serde(default)]
    pub gateways: Vec<PairedGateway>,
}

pub fn paired_gateways_path(app_data: &Path) -> PathBuf {
    app_data.join("paired-gateways.json")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedGateway {
    pub pairing_id: String,
    pub label: String,
    pub may_manage: bool,
    pub may_control: bool,
    pub may_admin: bool,
    pub remote_starts: RemoteStartSetting,
}

impl AuthenticatedGateway {
    /// The admin grant implies the other two (design decision 5).
    pub fn manages(&self) -> bool {
        self.may_manage || self.may_admin
    }

    pub fn controls(&self) -> bool {
        self.may_control || self.may_admin
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HopGrants {
    pub manage: bool,
    pub control: bool,
    pub admin: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HopGrant {
    Manage,
    Control,
    Admin,
}

impl HopGrant {
    fn name(self) -> &'static str {
        match self {
            Self::Manage => "manage",
            Self::Control => "control",
            Self::Admin => "admin",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum GatewayRedemption {
    Paired {
        pairing_id: String,
        credential: String,
    },
    Rejected,
    AwaitingOwner {
        started: bool,
    },
    Declined,
}

pub struct PairedGatewayStore {
    path: PathBuf,
    gateways: Mutex<PairedGateways>,
    writing: Mutex<()>,
    pending: Mutex<PendingPairings>,
    receipts: Mutex<crate::return_access::Receipts>,
    operation_nonces: Mutex<std::collections::HashMap<(String, String), u64>>,
}

impl PairedGatewayStore {
    pub fn load(path: PathBuf) -> Result<Self, crate::BoxError> {
        let gateways = auth::load_store(&path)?;
        Ok(Self {
            path,
            gateways: Mutex::new(gateways),
            writing: Mutex::new(()),
            pending: Mutex::new(PendingPairings::new(PairingKind::Gateway)),
            receipts: Mutex::default(),
            operation_nonces: Mutex::default(),
        })
    }

    pub fn mint_secret(
        &self,
        label: Option<String>,
        grants: HopGrants,
        minter: Minter,
    ) -> Result<String, String> {
        self.pending
            .lock()
            .map_err(|_| "gateway pairing state is unavailable".to_string())?
            .mint(crate::pairing::PairingIntent {
                label,
                user_id: None,
                grants,
                minter,
            })
            .map(|minted| minted.secret)
            .map_err(|e| format!("could not mint a gateway pairing secret: {e}"))
    }

    pub fn redeem(
        &self,
        secret: &str,
        offered_label: Option<&str>,
        gate: OwnerGate<'_>,
    ) -> Result<GatewayRedemption, String> {
        let offered_label = offered_label.map(str::trim).filter(|s| !s.is_empty());
        let intent = {
            let mut pending = self
                .pending
                .lock()
                .map_err(|_| "gateway pairing state is unavailable".to_string())?;
            match pending.redeem(secret, PairingChannel::Public, gate) {
                Redemption::Redeemed(intent) => intent,
                Redemption::AwaitingOwner { started } => {
                    return Ok(GatewayRedemption::AwaitingOwner { started })
                }
                Redemption::Declined => return Ok(GatewayRedemption::Declined),
                Redemption::Rejected | Redemption::LocalOnly | Redemption::Throttled => {
                    return Ok(GatewayRedemption::Rejected)
                }
            }
        };
        let label = intent
            .label
            .or_else(|| offered_label.map(str::to_string))
            .unwrap_or_else(|| "Paired gateway".to_string());
        let credential =
            auth::mint_credential().map_err(|e| format!("could not mint a credential: {e}"))?;
        let pairing_id =
            auth::mint_device_id().map_err(|e| format!("could not mint a pairing id: {e}"))?;
        let gateway = PairedGateway {
            id: pairing_id.clone(),
            label,
            credential_digest: auth::digest(&credential),
            paired_at: chrono::Utc::now().to_rfc3339(),
            last_used_at: None,
            previous_credential: None,
            may_manage: intent.grants.manage,
            may_control: intent.grants.control,
            may_admin: intent.grants.admin,
            remote_starts: RemoteStartSetting::default(),
            return_route: None,
        };
        crate::log_legacy!("[Gateway] paired with a remote gateway: {}", gateway.label);
        self.write(|store| store.gateways.push(gateway))?;
        Ok(GatewayRedemption::Paired {
            pairing_id,
            credential,
        })
    }

    pub fn requests(&self) -> Vec<PairingRequest> {
        self.pending
            .lock()
            .map(|p| p.requests())
            .unwrap_or_default()
    }

    pub fn answer(&self, id: &str, answer: PairingAnswer) -> Result<bool, String> {
        Ok(self
            .pending
            .lock()
            .map_err(|_| "gateway pairing state is unavailable".to_string())?
            .answer(id, answer))
    }

    pub fn authorize(&self, headers: &HeaderMap) -> Result<Option<AuthenticatedGateway>, String> {
        let Some(digest) = presented_digest(headers) else {
            return Ok(None);
        };
        self.with_authenticated(&digest, |gateway, current| {
            if current {
                gateway.previous_credential = None;
            }
            let now = chrono::Utc::now();
            let due = gateway
                .last_used_at
                .as_deref()
                .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                .is_none_or(|at| now.signed_duration_since(at).num_seconds() >= 60);
            if due {
                gateway.last_used_at = Some(now.to_rfc3339());
            }
            Ok(AuthenticatedGateway {
                pairing_id: gateway.id.clone(),
                label: gateway.label.clone(),
                may_manage: gateway.may_manage,
                may_control: gateway.may_control,
                may_admin: gateway.may_admin,
                remote_starts: gateway.remote_starts,
            })
        })
    }

    pub fn rotate(&self, headers: &HeaderMap, candidate: &str) -> Result<bool, String> {
        if candidate.len() != 64
            || !candidate
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(
                "a rotation candidate must be 32 random bytes encoded as lowercase hex".into(),
            );
        }
        let Some(digest) = presented_digest(headers) else {
            return Ok(false);
        };
        let replacement = auth::digest(candidate);
        self.with_authenticated(&digest, |gateway, current| {
            if ct_eq(&gateway.credential_digest, &replacement) {
                return Ok(());
            }
            if !current {
                return Err("a previous credential cannot start another rotation".into());
            }
            gateway.previous_credential = Some(PreviousCredential {
                digest: gateway.credential_digest.clone(),
                expires_at: chrono::Utc::now().timestamp() + 600,
            });
            gateway.credential_digest = replacement;
            Ok(())
        })
        .map(|result| result.is_some())
    }

    fn with_authenticated<T>(
        &self,
        digest: &str,
        mutate: impl FnOnce(&mut PairedGateway, bool) -> Result<T, String>,
    ) -> Result<Option<T>, String> {
        let _writing = self
            .writing
            .lock()
            .map_err(|_| "paired gateway store is unavailable")?;
        let mut store = self
            .gateways
            .lock()
            .map_err(|_| "paired gateway store is unavailable")?;
        let mut next = store.clone();
        let now = chrono::Utc::now().timestamp();
        let Some(gateway) = next.gateways.iter_mut().find(|g| {
            ct_eq(&g.credential_digest, digest)
                || g.previous_credential
                    .as_ref()
                    .is_some_and(|old| old.expires_at > now && ct_eq(&old.digest, digest))
        }) else {
            return Ok(None);
        };
        let current = ct_eq(&gateway.credential_digest, digest);
        let result = mutate(gateway, current)?;
        if next != *store {
            auth::save_store(&next, &self.path)
                .map_err(|e| format!("could not save the paired gateway store: {e}"))?;
            *store = next;
        }
        Ok(Some(result))
    }

    pub fn list(&self) -> Vec<PairedGateway> {
        self.gateways
            .lock()
            .map(|g| g.gateways.clone())
            .unwrap_or_default()
    }

    pub(crate) fn has_no_incoming_pairings(&self) -> bool {
        self.gateways
            .lock()
            .is_ok_and(|store| store.gateways.is_empty())
    }

    pub(crate) fn contains(&self, id: &str) -> bool {
        self.gateways
            .lock()
            .is_ok_and(|store| store.gateways.iter().any(|gateway| gateway.id == id))
    }

    pub fn unpair(&self, id: &str) -> Result<bool, String> {
        let mut removed = false;
        self.write(|store| {
            let before = store.gateways.len();
            store.gateways.retain(|g| g.id != id);
            removed = store.gateways.len() != before;
        })?;
        if removed {
            crate::log_legacy!("[Gateway] unpaired a remote gateway");
        }
        Ok(removed)
    }

    pub fn set_grant(
        &self,
        id: &str,
        grant: HopGrant,
        on: bool,
    ) -> Result<Option<PairedGateway>, String> {
        let mut updated = None;
        self.write(|store| {
            if let Some(gateway) = store.gateways.iter_mut().find(|g| g.id == id) {
                match grant {
                    HopGrant::Manage => gateway.may_manage = on,
                    HopGrant::Control => gateway.may_control = on,
                    HopGrant::Admin => gateway.may_admin = on,
                }
                updated = Some(gateway.clone());
            }
        })?;
        if let Some(gateway) = &updated {
            crate::log_legacy!(
                "[Gateway] {} the {} grant for '{}'",
                if on { "turned on" } else { "turned off" },
                grant.name(),
                gateway.label
            );
        }
        Ok(updated)
    }

    pub fn set_remote_starts(
        &self,
        id: &str,
        setting: RemoteStartSetting,
    ) -> Result<Option<PairedGateway>, String> {
        let mut updated = None;
        self.write(|store| {
            if let Some(gateway) = store.gateways.iter_mut().find(|g| g.id == id) {
                gateway.remote_starts = setting;
                updated = Some(gateway.clone());
            }
        })?;
        if let Some(gateway) = &updated {
            crate::log_legacy!(
                "[Gateway] set remote starts from '{}' to {}",
                gateway.label,
                setting.as_str()
            );
        }
        Ok(updated)
    }

    pub(crate) fn set_return_route(
        &self,
        id: &str,
        route: Option<crate::return_access::ReturnRoute>,
    ) -> Result<bool, String> {
        let mut found = false;
        self.write(|store| {
            if let Some(gateway) = store.gateways.iter_mut().find(|g| g.id == id) {
                gateway.return_route = route;
                found = true;
            }
        })?;
        Ok(found)
    }

    pub(crate) fn accept_operation_nonce(
        &self,
        workspace: &str,
        nonce: &str,
    ) -> Result<(), String> {
        let now = lucidos_gateway_stamp::now_unix();
        let mut seen = self
            .operation_nonces
            .lock()
            .map_err(|_| "Operation replay state is unavailable.")?;
        seen.retain(|_, expires| *expires > now);
        let key = (workspace.to_owned(), nonce.to_owned());
        if seen.len() >= 1024 || seen.contains_key(&key) {
            return Err(
                "This operation was already accepted, or the pending operation limit was reached."
                    .into(),
            );
        }
        seen.insert(
            key,
            now.saturating_add(2 * lucidos_gateway_stamp::WINDOW_SECS + 1),
        );
        Ok(())
    }

    pub(crate) fn with_return_receipts<T>(
        &self,
        pairing: &str,
        grant: &str,
        f: impl FnOnce(&mut crate::return_access::Receipts) -> T,
    ) -> Result<T, String> {
        self.with_pairing_receipts(pairing, Some(grant), f)
    }

    pub(crate) fn discard_return_receipt(&self, receipt: &str) {
        match self.receipts.lock() {
            Ok(mut receipts) => receipts.forget(receipt),
            Err(_) => crate::log!("[Gateway] Return receipt cleanup state is unavailable"),
        }
    }

    pub(crate) fn with_pairing_receipts<T>(
        &self,
        pairing: &str,
        grant: Option<&str>,
        f: impl FnOnce(&mut crate::return_access::Receipts) -> T,
    ) -> Result<T, String> {
        let gateways = self
            .gateways
            .lock()
            .map_err(|_| "Gateway pairings are unavailable.")?;
        if !gateways.gateways.iter().any(|g| {
            g.id == pairing
                && grant.is_none_or(|grant| {
                    g.return_route
                        .as_ref()
                        .is_some_and(|route| route.grant_id == grant)
                })
        }) {
            return Err("This return route is no longer registered on a current pairing.".into());
        }
        let mut receipts = self
            .receipts
            .lock()
            .map_err(|_| "Return receipts are unavailable.")?;
        Ok(f(&mut receipts))
    }

    fn write(&self, mutate: impl FnOnce(&mut PairedGateways)) -> Result<(), String> {
        fn unavailable<T>(_: T) -> String {
            "paired gateway store is unavailable".to_string()
        }
        let _writing = self.writing.lock().map_err(unavailable)?;
        let mut next = self.gateways.lock().map_err(unavailable)?.clone();
        mutate(&mut next);
        auth::save_store(&next, &self.path)
            .map_err(|e| format!("could not save the paired gateway store: {e}"))?;
        *self.gateways.lock().map_err(unavailable)? = next;
        Ok(())
    }
}

pub fn router(state: GatewayState) -> Router<GatewayState> {
    let v1 = Router::new()
        .route(PAIRING_PATH, get(own_pairing).delete(leave))
        .route(WORKSPACES_PATH, get(list_workspaces))
        .route(
            "/operation-challenges",
            post(crate::workspace_operations::challenge),
        )
        .route(
            "/operations",
            post(crate::workspace_operations::receive_peer),
        )
        .route(
            "/return-route",
            axum::routing::put(crate::return_access::register_route),
        )
        .route(
            "/return-receipts/:receipt",
            post(crate::return_access::redeem_receipt),
        )
        .route(crate::nudge::NUDGE_PATH, post(crate::nudge::receive))
        .route(
            &format!("{}/:slug", crate::relay::RELAY_PATH),
            any(crate::relay::receive),
        )
        .route(
            &format!("{}/:slug/", crate::relay::RELAY_PATH),
            any(crate::relay::receive),
        )
        .route(
            &format!("{}/:slug/*rest", crate::relay::RELAY_PATH),
            any(crate::relay::receive),
        )
        .route(
            &format!("{WORKSPACE_CALLS_PATH}/:target"),
            get(crate::workspace_calls::takes_calls_for),
        )
        .route(
            &format!("{WORKSPACE_CALLS_PATH}/:target/chat/stream"),
            post(crate::workspace_calls::receive),
        )
        .route(
            &format!("{WORKSPACE_CALLS_PATH}/:target/diagnostic-bundles"),
            post(crate::diagnostic_transport::receive),
        )
        .nest(
            MANAGE_PATH,
            crate::gateway_hop::receiver(&crate::gateway_hop::MANAGE),
        )
        .nest(
            CONTROL_PATH,
            crate::gateway_hop::receiver(&crate::gateway_hop::CONTROL),
        )
        .nest(
            ADMIN_PATH,
            crate::gateway_hop::receiver(&crate::gateway_hop::ADMIN),
        )
        .route_layer(middleware::from_fn_with_state(state, require_gateway))
        .route(
            "/return-operation",
            post(crate::workspace_operations::receive_return),
        )
        .route(PAIR_PATH, post(pair))
        .route(ROTATE_PATH, post(rotate));
    let version = GATEWAY_API_PREFIX
        .strip_prefix(GATEWAY_API_ROOT)
        .expect("the versioned prefix sits under the root");
    Router::new().nest(version, v1).fallback(not_found)
}

async fn not_found() -> ApiError {
    ApiError::not_found("no such gateway-to-gateway call")
}

async fn require_gateway(
    State(state): State<GatewayState>,
    mut req: Request,
    next: Next,
) -> Response {
    match state.paired_gateways().authorize(req.headers()) {
        Ok(Some(gateway)) => {
            req.extensions_mut().insert(gateway);
            next.run(req).await
        }
        Ok(None) => ApiError::unauthorized("this gateway is not paired here").into_response(),
        Err(error) => ApiError::internal(error).into_response(),
    }
}

fn presented_digest(headers: &HeaderMap) -> Option<String> {
    headers
        .get(HEADER_GATEWAY_CREDENTIAL)?
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(auth::digest)
}

#[derive(Deserialize)]
struct RotateRequest {
    candidate: String,
}

async fn rotate(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<RotateRequest>,
) -> Result<StatusCode, ApiError> {
    match state
        .paired_gateways()
        .rotate(&headers, &body.candidate)
        .map_err(ApiError::internal)?
    {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::unauthorized("this gateway is not paired here")),
    }
}

#[derive(Deserialize)]
struct PairRequest {
    code: String,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairAnswer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_id: Option<String>,
    pub pairing_id: String,
    pub credential: String,
    pub gateway_protocol: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_address: Option<String>,
}

async fn pair(
    State(state): State<GatewayState>,
    Json(body): Json<PairRequest>,
) -> Result<Response, ApiError> {
    let asking_as = body
        .label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Unnamed gateway");
    let redemption = state
        .paired_gateways()
        .redeem(
            &body.code,
            body.label.as_deref(),
            state.owner_gate(asking_as),
        )
        .map_err(ApiError::internal)?;
    let (pairing_id, credential) = match redemption {
        GatewayRedemption::Paired {
            pairing_id,
            credential,
        } => (pairing_id, credential),
        GatewayRedemption::Rejected => {
            return Err(ApiError::bad_request(
                "that gateway pairing secret is not valid or has expired",
            ))
        }
        GatewayRedemption::AwaitingOwner { started } => {
            if started {
                crate::pairing_requests::signal_engines(
                    &state,
                    Some((PairingKind::Gateway, asking_as)),
                );
            }
            return Ok(crate::pairing_requests::awaiting_owner());
        }
        GatewayRedemption::Declined => {
            return Err(ApiError::forbidden(crate::pairing_requests::DECLINED))
        }
    };
    let external_address = crate::net_config::external_address(state.gateway_port())
        .origin
        .filter(|origin| origin.starts_with("https://"));
    Ok(Json(PairAnswer {
        gateway_id: Some(state.gateway_id().to_string()),
        pairing_id,
        credential,
        gateway_protocol: GATEWAY_PROTOCOL,
        external_address,
    })
    .into_response())
}

#[derive(Serialize)]
struct OwnPairing {
    gateway_id: String,
    pairing_id: String,
    label: String,
    gateway_protocol: u32,
}

async fn own_pairing(
    State(state): State<GatewayState>,
    Extension(gateway): Extension<AuthenticatedGateway>,
) -> Json<OwnPairing> {
    Json(OwnPairing {
        gateway_id: state.gateway_id().to_string(),
        pairing_id: gateway.pairing_id,
        label: gateway.label,
        gateway_protocol: GATEWAY_PROTOCOL,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListedWorkspace {
    pub id: String,
    pub name: String,
    pub health: Health,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_count: Option<u64>,
}

impl From<crate::stack::WorkspaceStatus> for ListedWorkspace {
    fn from(w: crate::stack::WorkspaceStatus) -> Self {
        Self {
            id: w.id,
            name: w.name,
            health: w.health,
            last_error: w.last_error,
            unread_count: w.unread_count,
        }
    }
}

pub(crate) async fn list_workspaces(
    State(state): State<GatewayState>,
) -> Json<Vec<ListedWorkspace>> {
    Json(
        state
            .list_status()
            .await
            .into_iter()
            .map(Into::into)
            .collect(),
    )
}

async fn leave(
    State(state): State<GatewayState>,
    Extension(gateway): Extension<AuthenticatedGateway>,
) -> Result<StatusCode, ApiError> {
    end_pairing(&state, &gateway.pairing_id)?;
    Ok(StatusCode::NO_CONTENT)
}

fn end_pairing(state: &GatewayState, id: &str) -> Result<bool, ApiError> {
    let unpaired = state
        .paired_gateways()
        .unpair(id)
        .map_err(ApiError::internal)?;
    state.device_splices().abort_pairing(id);
    state
        .record(|directory| Ok(directory.pairing_forgotten(id).into_iter().collect()))
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    Ok(unpaired)
}

/// The gateway pairing routes the admin hop reaches. No grant switch: the
/// admin grant lives only on this gateway (design decision 3).
pub fn admin_hop_routes() -> Router<GatewayState> {
    Router::new()
        .route("/gateway-pairing-code", post(gateway_pairing_code))
        .route("/gateways", get(list_gateways))
        .route("/gateways/:id", axum::routing::delete(unpair_gateway))
}

pub fn management_router() -> Router<GatewayState> {
    Router::new()
        .route("/gateway-pairing-code", post(gateway_pairing_code))
        .route("/gateways", get(list_gateways))
        .route("/gateways/:id", axum::routing::delete(unpair_gateway))
        .route("/gateways/:id/manage", axum::routing::put(set_manage))
        .route("/gateways/:id/control", axum::routing::put(set_control))
        .route("/gateways/:id/admin", axum::routing::put(set_admin))
        .route(
            "/gateways/:id/remote-starts",
            axum::routing::put(set_remote_starts),
        )
}

#[derive(Deserialize, Default)]
struct GatewayCodeQuery {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    manage: bool,
    /// Start it with the control grant on, as `--control` asks.
    #[serde(default)]
    control: bool,
    /// Start it with the admin grant on, as `--admin` asks.
    #[serde(default)]
    admin: bool,
}

#[derive(Serialize)]
struct GatewayCodeBody {
    code: String,
    expires_in_secs: u64,
}

async fn gateway_pairing_code(
    State(state): State<GatewayState>,
    device: Option<Extension<auth::AuthenticatedDevice>>,
    hop_caller: Option<Extension<crate::gateway_hop::HopCaller>>,
    axum::extract::Query(query): axum::extract::Query<GatewayCodeQuery>,
) -> Result<Json<GatewayCodeBody>, ApiError> {
    // Every grant lives only on this gateway. A grant minted through a hop
    // would outlive the caller's own admin grant (ADR 0830).
    if hop_caller.is_some() && (query.manage || query.control || query.admin) {
        return Err(ApiError::forbidden(
            "only a device paired with this gateway can grant another gateway access",
        ));
    }
    let minter = match &hop_caller {
        Some(Extension(caller)) => {
            crate::log_legacy!(
                "[Gateway] '{}' via '{}' minted a gateway pairing secret",
                caller.device,
                caller.gateway
            );
            pairing::Minter::RemoteDevice
        }
        None => crate::pairing_requests::minter(device.as_ref()),
    };
    let label = query
        .label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let secret = state
        .paired_gateways()
        .mint_secret(
            label,
            HopGrants {
                manage: query.manage,
                control: query.control,
                admin: query.admin,
            },
            minter,
        )
        .map_err(ApiError::internal)?;
    Ok(Json(GatewayCodeBody {
        code: secret,
        expires_in_secs: pairing::pairing_ttl_secs(),
    }))
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatewayRow {
    pub id: String,
    pub label: String,
    pub paired_at: String,
    pub last_used_at: Option<String>,
    pub may_manage: bool,
    pub may_control: bool,
    pub may_admin: bool,
    pub remote_starts: RemoteStartSetting,
}

impl From<PairedGateway> for GatewayRow {
    fn from(g: PairedGateway) -> Self {
        Self {
            id: g.id,
            label: g.label,
            paired_at: g.paired_at,
            last_used_at: g.last_used_at,
            may_manage: g.may_manage,
            may_control: g.may_control,
            may_admin: g.may_admin,
            remote_starts: g.remote_starts,
        }
    }
}

async fn list_gateways(State(state): State<GatewayState>) -> Json<Vec<GatewayRow>> {
    Json(
        state
            .paired_gateways()
            .list()
            .into_iter()
            .map(Into::into)
            .collect(),
    )
}

#[derive(Deserialize)]
struct ManageBody {
    may_manage: bool,
}

async fn set_manage(
    state: State<GatewayState>,
    id: axum::extract::Path<String>,
    Json(body): Json<ManageBody>,
) -> Result<Json<GatewayRow>, ApiError> {
    set_grant(state, id, HopGrant::Manage, body.may_manage)
}

#[derive(Deserialize)]
struct ControlBody {
    may_control: bool,
}

async fn set_control(
    state: State<GatewayState>,
    id: axum::extract::Path<String>,
    Json(body): Json<ControlBody>,
) -> Result<Json<GatewayRow>, ApiError> {
    set_grant(state, id, HopGrant::Control, body.may_control)
}

#[derive(Deserialize)]
struct AdminBody {
    may_admin: bool,
}

async fn set_admin(
    state: State<GatewayState>,
    id: axum::extract::Path<String>,
    Json(body): Json<AdminBody>,
) -> Result<Json<GatewayRow>, ApiError> {
    set_grant(state, id, HopGrant::Admin, body.may_admin)
}

fn set_grant(
    State(state): State<GatewayState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    grant: HopGrant,
    on: bool,
) -> Result<Json<GatewayRow>, ApiError> {
    state
        .paired_gateways()
        .set_grant(&id, grant, on)
        .map_err(ApiError::internal)?
        .map(|g| Json(g.into()))
        .ok_or_else(|| ApiError::not_found("no paired gateway with that id"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteStartsBody {
    remote_starts: RemoteStartSetting,
}

async fn set_remote_starts(
    State(state): State<GatewayState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<RemoteStartsBody>,
) -> Result<Json<GatewayRow>, ApiError> {
    state
        .paired_gateways()
        .set_remote_starts(&id, body.remote_starts)
        .map_err(ApiError::internal)?
        .map(|g| Json(g.into()))
        .ok_or_else(|| ApiError::not_found("no paired gateway with that id"))
}

async fn unpair_gateway(
    State(state): State<GatewayState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<StatusCode, ApiError> {
    if end_pairing(&state, &id)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("no paired gateway with that id"))
    }
}

#[cfg(test)]
#[path = "gateway_protocol_tests.rs"]
mod tests;
