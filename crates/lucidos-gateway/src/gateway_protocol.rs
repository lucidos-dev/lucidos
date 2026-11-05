//! The gateway-to-gateway protocol, from the side that grants: gateway B.
//!
//! B mints a *gateway pairing secret*, gateway A redeems it, and A then holds a
//! gateway credential for B. ADR 0590 records why this is a grant of its own
//! rather than one more device. A's half lives in [`crate::remote_gateways`].
//!
//! # Two pairing kinds, never crossing
//!
//! Gateway pairings live in [`PairedGatewayStore`], apart from the device
//! pairings in `GatewayInner::pending_pairings`. A device secret never pairs a
//! gateway, and a gateway secret never pairs a device.
//!
//! # The path and the version only add
//!
//! Every call lives under [`GATEWAY_API_PREFIX`]. The device gate in
//! [`crate::auth_api::enforce`] stands aside there, and [`router`] checks the
//! gateway credential itself. B's public health names [`GATEWAY_PROTOCOL`], so
//! A can hide what an older B lacks.

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
use crate::pairing::{self, PairingChannel, PairingKind, PendingPairings, Redemption};
use crate::server::GatewayState;
use crate::stack::Health;

/// The protocol version B names in its public health. Bumped only when a call
/// is ADDED; nothing under [`GATEWAY_API_PREFIX`] is ever removed or changed.
pub const GATEWAY_PROTOCOL: u32 = 2;

pub const ROTATE_PATH: &str = "/rotate";

/// Where every gateway-to-gateway call lives.
pub const GATEWAY_API_PREFIX: &str = "/~/api/gateway/v1";

/// The namespace [`router`] is nested at. Everything under it skips the device
/// gate, so anything this router does not route must answer 404 here.
pub const GATEWAY_API_ROOT: &str = "/~/api/gateway";

/// The header that carries A's gateway credential.
pub const HEADER_GATEWAY_CREDENTIAL: &str = "x-lucidos-gateway-credential";

/// Redeem a gateway pairing secret, relative to [`GATEWAY_API_PREFIX`].
pub const PAIR_PATH: &str = "/pair";

/// Read or end the caller's own pairing, relative to [`GATEWAY_API_PREFIX`].
pub const PAIRING_PATH: &str = "/pairing";

/// List B's workspaces, relative to [`GATEWAY_API_PREFIX`].
pub const WORKSPACES_PATH: &str = "/workspaces";

/// Is this path in the gateway-to-gateway namespace?
pub fn is_gateway_api_path(path: &str) -> bool {
    path.strip_prefix(GATEWAY_API_ROOT)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// One gateway B has granted access to, as B persists it.
///
/// A digest of the credential, never the credential, for the reason
/// `auth::PairedDevice` gives: a leaked file must not be standing access.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedGateway {
    /// The pairing id. B namespaces relayed devices under it (ADR 0591).
    pub id: String,
    pub label: String,
    pub credential_digest: String,
    pub paired_at: String,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub previous_credential: Option<PreviousCredential>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreviousCredential {
    pub digest: String,
    pub expires_at: i64,
}

/// B's paired-gateway file, `<data dir>/paired-gateways.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedGateways {
    #[serde(default)]
    pub gateways: Vec<PairedGateway>,
}

/// Per gateway, beside the device store and never shared (ADR 0132).
pub fn paired_gateways_path(app_data: &Path) -> PathBuf {
    app_data.join("paired-gateways.json")
}

/// The gateway A became by redeeming a secret, stamped on the request by
/// [`require_gateway`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedGateway {
    pub pairing_id: String,
    pub label: String,
}

/// What one redemption produced.
#[derive(Debug, PartialEq, Eq)]
pub enum GatewayRedemption {
    Paired {
        pairing_id: String,
        credential: String,
    },
    Rejected,
}

/// B's paired gateways and its outstanding gateway pairings.
///
/// The same locking shape as the device store in `server.rs`. The data lock is
/// held only to read or mutate, and `writing` orders the file writes. Every
/// write publishes current memory, so a slow writer cannot restore a pairing
/// an Unpair removed.
pub struct PairedGatewayStore {
    path: PathBuf,
    gateways: Mutex<PairedGateways>,
    writing: Mutex<()>,
    /// Never persisted, like device pairings: a pairing lives five minutes.
    pending: Mutex<PendingPairings>,
}

impl PairedGatewayStore {
    pub fn load(path: PathBuf) -> Result<Self, crate::BoxError> {
        let gateways = auth::load_store(&path)?;
        Ok(Self {
            path,
            gateways: Mutex::new(gateways),
            writing: Mutex::new(()),
            pending: Mutex::new(PendingPairings::new(PairingKind::Gateway)),
        })
    }

    /// Mint a gateway pairing secret, carrying the label B's user suggested.
    pub fn mint_secret(&self, label: Option<String>) -> Result<String, String> {
        self.pending
            .lock()
            .map_err(|_| "gateway pairing state is unavailable".to_string())?
            .mint(label)
            .map(|minted| minted.secret)
            .map_err(|e| format!("could not mint a gateway pairing secret: {e}"))
    }

    /// Redeem `secret` and record a paired gateway.
    ///
    /// Never throttled: the secret is long enough that no flood can guess it,
    /// so no flood can spend a budget that stands before the right one.
    ///
    /// The label B's user gave the secret wins here, unlike a device. B's user
    /// is naming a machine they chose to trust, and A only offers its own
    /// hostname.
    pub fn redeem(
        &self,
        secret: &str,
        offered_label: Option<&str>,
    ) -> Result<GatewayRedemption, String> {
        let minted_label = {
            let mut pending = self
                .pending
                .lock()
                .map_err(|_| "gateway pairing state is unavailable".to_string())?;
            match pending.redeem(secret, PairingChannel::Public) {
                Redemption::Redeemed(label) => label,
                Redemption::Rejected | Redemption::LocalOnly | Redemption::Throttled => {
                    return Ok(GatewayRedemption::Rejected)
                }
            }
        };
        let label = minted_label
            .or_else(|| {
                offered_label
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
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
        };
        crate::log!("[Gateway] paired with a remote gateway: {}", gateway.label);
        self.write(|store| store.gateways.push(gateway))?;
        Ok(GatewayRedemption::Paired {
            pairing_id,
            credential,
        })
    }

    /// The paired gateway whose credential this request carries.
    ///
    /// Only the dedicated header counts. A cookie is never read here, so a
    /// browser's jar cannot stand in for a gateway. A poisoned lock fails
    /// closed.
    pub fn authorize(&self, headers: &HeaderMap) -> Result<Option<AuthenticatedGateway>, String> {
        let Some(digest) = presented_digest(headers) else {
            return Ok(None);
        };
        self.with_authenticated(&digest, |gateway, current| {
            // First successful use of the new key durably ends the overlap.
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
            })
        })
    }

    /// Install A's durably sealed candidate. Repeating an intent is idempotent.
    /// A chooses the key before contacting B so a lost response is recoverable.
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
            // An old overlap key cannot race the current key into a third key.
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

    /// Forget a paired gateway. `false` when none had that id. Its next call
    /// is refused, because [`Self::authorize`] reads memory.
    pub fn unpair(&self, id: &str) -> Result<bool, String> {
        let mut removed = false;
        self.write(|store| {
            let before = store.gateways.len();
            store.gateways.retain(|g| g.id != id);
            removed = store.gateways.len() != before;
        })?;
        if removed {
            crate::log!("[Gateway] unpaired a remote gateway");
        }
        Ok(removed)
    }

    fn write(&self, mutate: impl FnOnce(&mut PairedGateways)) -> Result<(), String> {
        fn unavailable<T>(_: T) -> String {
            "paired gateway store is unavailable".to_string()
        }
        let _writing = self.writing.lock().map_err(unavailable)?;
        // Save first, publish after: a failed save changes nothing anyone sees.
        let mut next = self.gateways.lock().map_err(unavailable)?.clone();
        mutate(&mut next);
        auth::save_store(&next, &self.path)
            .map_err(|e| format!("could not save the paired gateway store: {e}"))?;
        *self.gateways.lock().map_err(unavailable)? = next;
        Ok(())
    }
}

/// The gateway-to-gateway router, nested at [`GATEWAY_API_ROOT`].
///
/// Every route but [`PAIR_PATH`] sits behind [`require_gateway`]. The fallback
/// answers 404 rather than letting the picker's fallback serve this namespace.
pub fn router(state: GatewayState) -> Router<GatewayState> {
    let v1 = Router::new()
        .route(PAIRING_PATH, get(own_pairing).delete(leave))
        .route(WORKSPACES_PATH, get(list_workspaces))
        .route(crate::nudge::NUDGE_PATH, post(crate::nudge::receive))
        .route(
            &format!("{}/:slug", crate::relay::RELAY_PATH),
            any(crate::relay::receive),
        )
        // A wildcard never matches an empty tail, so the workspace root
        // `/relay/<slug>/` needs a route of its own.
        .route(
            &format!("{}/:slug/", crate::relay::RELAY_PATH),
            any(crate::relay::receive),
        )
        .route(
            &format!("{}/:slug/*rest", crate::relay::RELAY_PATH),
            any(crate::relay::receive),
        )
        .route_layer(middleware::from_fn_with_state(state, require_gateway))
        // Pair is the call A makes before it holds a credential. The long secret
        // is the only thing standing here: single use, expiring, and too long
        // for any flood to guess, so nothing here is throttled.
        .route(PAIR_PATH, post(pair))
        // Authenticate atomically with rotation, without consuming overlap first.
        .route(ROTATE_PATH, post(rotate));
    let version = GATEWAY_API_PREFIX
        .strip_prefix(GATEWAY_API_ROOT)
        .expect("the versioned prefix sits under the root");
    Router::new().nest(version, v1).fallback(not_found)
}

async fn not_found() -> ApiError {
    ApiError::not_found("no such gateway-to-gateway call")
}

/// Refuse a call without a current pairing's credential.
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
    /// The gateway pairing secret. Named `code` on the wire, which only adds.
    code: String,
    /// What A calls itself. B's user may already have named the secret.
    #[serde(default)]
    label: Option<String>,
}

/// What A learns by redeeming a secret.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairAnswer {
    pub pairing_id: String,
    pub credential: String,
    pub gateway_protocol: u32,
    /// B's ADR 0550 external address, when it is `https`. Told only to a
    /// gateway holding a secret, never in public health.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_address: Option<String>,
}

async fn pair(
    State(state): State<GatewayState>,
    Json(body): Json<PairRequest>,
) -> Result<Json<PairAnswer>, ApiError> {
    let redemption = state
        .paired_gateways()
        .redeem(&body.code, body.label.as_deref())
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
    };
    let external_address = crate::net_config::external_address(state.gateway_port())
        .origin
        .filter(|origin| origin.starts_with("https://"));
    Ok(Json(PairAnswer {
        pairing_id,
        credential,
        gateway_protocol: GATEWAY_PROTOCOL,
        external_address,
    }))
}

#[derive(Serialize)]
struct OwnPairing {
    pairing_id: String,
    label: String,
    gateway_protocol: u32,
}

/// The caller's own pairing, so A can tell a live grant from a revoked one.
async fn own_pairing(Extension(gateway): Extension<AuthenticatedGateway>) -> Json<OwnPairing> {
    Json(OwnPairing {
        pairing_id: gateway.pairing_id,
        label: gateway.label,
        gateway_protocol: GATEWAY_PROTOCOL,
    })
}

/// One of B's workspaces, as a paired gateway lists it.
///
/// No port and no directory: A reaches the workspace through the relay, so
/// neither means anything on A's side.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListedWorkspace {
    pub id: String,
    pub name: String,
    pub health: Health,
    /// B's own reason, as its picker shows it. `"not started"` tells a stopped
    /// workspace from a failed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// B's workspaces, for A's Workspaces list.
async fn list_workspaces(State(state): State<GatewayState>) -> Json<Vec<ListedWorkspace>> {
    Json(
        state
            .list_status()
            .await
            .into_iter()
            .map(|w| ListedWorkspace {
                id: w.id,
                name: w.name,
                health: w.health,
                last_error: w.last_error,
            })
            .collect(),
    )
}

/// A removed B, so B forgets the grant too.
async fn leave(
    State(state): State<GatewayState>,
    Extension(gateway): Extension<AuthenticatedGateway>,
) -> Result<StatusCode, ApiError> {
    state
        .paired_gateways()
        .unpair(&gateway.pairing_id)
        .map_err(ApiError::internal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// B's management routes, merged into `auth_api::router` behind its gates.
pub fn management_router() -> Router<GatewayState> {
    Router::new()
        .route("/gateway-pairing-code", post(gateway_pairing_code))
        .route("/gateways", get(list_gateways))
        .route("/gateways/:id", axum::routing::delete(unpair_gateway))
}

#[derive(Deserialize, Default)]
struct GatewayCodeQuery {
    #[serde(default)]
    label: Option<String>,
}

#[derive(Serialize)]
struct GatewayCodeBody {
    /// The gateway pairing secret. Named `code` on the wire, which only adds.
    code: String,
    expires_in_secs: u64,
}

async fn gateway_pairing_code(
    State(state): State<GatewayState>,
    axum::extract::Query(query): axum::extract::Query<GatewayCodeQuery>,
) -> Result<Json<GatewayCodeBody>, ApiError> {
    let label = query
        .label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let secret = state
        .paired_gateways()
        .mint_secret(label)
        .map_err(ApiError::internal)?;
    Ok(Json(GatewayCodeBody {
        code: secret,
        expires_in_secs: pairing::pairing_ttl_secs(),
    }))
}

#[derive(Serialize)]
struct GatewayRow {
    id: String,
    label: String,
    paired_at: String,
    last_used_at: Option<String>,
}

async fn list_gateways(State(state): State<GatewayState>) -> Json<Vec<GatewayRow>> {
    Json(
        state
            .paired_gateways()
            .list()
            .into_iter()
            .map(|g| GatewayRow {
                id: g.id,
                label: g.label,
                paired_at: g.paired_at,
                last_used_at: g.last_used_at,
            })
            .collect(),
    )
}

async fn unpair_gateway(
    State(state): State<GatewayState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<StatusCode, ApiError> {
    if state
        .paired_gateways()
        .unpair(&id)
        .map_err(ApiError::internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("no paired gateway with that id"))
    }
}

#[cfg(test)]
#[path = "gateway_protocol_tests.rs"]
mod tests;
