//! HTTP surface for pairing, plus the middleware that enforces authorization.
//!
//! Policy and storage live in [`crate::auth`], which has no HTTP in it and is
//! unit-tested on its own. This module is the wiring.
//!
//! # What stays reachable without a credential
//!
//! [`is_public_path`] is the whole exemption list, and it is deliberately
//! short. The picker's shell and assets are public because an unpaired browser
//! needs a surface to pair *from*: gating them would answer a new phone with a
//! bare 401 and no way forward. Every API under `/~/api/` is gated except the
//! two pairing calls and health.
//!
//! An unauthenticated *navigation* is answered with the pairing screen, at the
//! URL it asked for. Anything else gets 401. [`crate::server::serve_pairing_shell`]
//! records why the screen is served in place rather than redirected to.

use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::{self, Authorization};
use crate::error::ApiError;
use crate::pairing_qr;
use crate::server::GatewayState;

pub fn router() -> Router<GatewayState> {
    Router::new()
        .route("/session", get(session))
        .route("/pairing-code", post(pairing_code))
        .route("/pair", post(pair))
        .route("/devices", get(list_devices))
        .route("/devices/:id", axum::routing::delete(revoke_device))
}

/// May this path be served with no credential at all?
///
/// Exact-matched, never prefix-matched, so a future `/~/api/v1/health/secrets`
/// cannot inherit the exemption its parent has.
pub fn is_public_path(path: &str) -> bool {
    // A dot segment is never public. Paths arrive un-normalized, so without
    // this a request like `/~/assets/../api/v1/control/...` reads as a picker
    // asset. Nothing downstream currently routes such a path to the control
    // plane, but the exemption list must not be the thing standing in the way.
    if path.split('/').any(|seg| seg == ".." || seg == ".") {
        return false;
    }
    if path == "/" {
        return true;
    }
    let Some(rest) = path.strip_prefix("/~/") else {
        return false;
    };
    if !rest.starts_with("api/") {
        // The picker shell and its bundled assets. Static files only: no
        // workspace data and no control surface is served from here.
        return true;
    }
    matches!(
        rest,
        "api/v1/health" | "api/v1/auth/pair" | "api/v1/auth/session"
    )
}

/// Is this request a top-level page load, as opposed to a fetch?
///
/// `Sec-Fetch-Mode` is set by the browser and cannot be forged from script,
/// which is why it is read first. The `Accept` fallback covers clients that
/// send no fetch metadata at all.
///
/// Deliberately NOT `server::is_document_navigation`, which answers a related
/// question and disagrees on the case that matters here. That one reaches
/// `Accept` whenever the fetch metadata is merely not a navigation. So a script
/// `fetch` asking for HTML reads to it as a page load. Waking a stopped
/// workspace for one is harmless. Handing it a pairing screen is not: the
/// caller cannot use it, and its JSON parse fails.
fn wants_html(headers: &HeaderMap) -> bool {
    if let Some(mode) = headers.get("sec-fetch-mode").and_then(|v| v.to_str().ok()) {
        return mode == "navigate";
    }
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/html"))
}

/// Refuse a request that proved nothing.
///
/// Applied in front of everything: the proxy into each workspace, the control
/// plane and the picker's own API.
pub async fn enforce(State(state): State<GatewayState>, mut req: Request, next: Next) -> Response {
    if is_public_path(req.uri().path()) {
        return next.run(req).await;
    }
    match state.authorize(req.headers()) {
        Authorization::Device { id, .. } => {
            // Tell the proxy who this is. The engine keys push, preferences and
            // actor attribution on the same id. So the device the gateway let
            // in and the device the workspace knows are one row, not two.
            req.extensions_mut()
                .insert(auth::AuthenticatedDevice(id.clone()));
            // A day since we last saw this device means two things at once: a
            // fresh liveness stamp for the devices list, and a fresh cookie so
            // a device in use never reaches its `Max-Age`. Both ride the same
            // beat, so an active device pays one whole-file save per day.
            let refresh = state
                .touch_device(&id, chrono::Utc::now())
                .then(|| refreshed_credential_cookie(&state, req.headers()))
                .flatten();
            let mut response = next.run(req).await;
            if let Some(cookie) = refresh {
                if !response_speaks_for_the_credential(&response) {
                    // Appended, never inserted. `Set-Cookie` is multi-valued, and
                    // this wraps every handler, so replacing the header would
                    // drop any other cookie the response set.
                    response.headers_mut().append(header::SET_COOKIE, cookie);
                }
            }
            return response;
        }
        Authorization::LocalProcess => return next.run(req).await,
        Authorization::Unauthorized => {}
    }
    if wants_html(req.headers()) {
        // Show the pairing screen here, rather than a bare 401 with no
        // affordance. In place rather than redirected: see `serve_pairing_shell`.
        // Hand the screen the code the caller arrived with: a `pair` query does
        // reach a gated path, and `serve_pairing_shell` says how.
        let pair_code = pairing_qr::pairing_code_in_query(req.uri().query());
        return crate::server::serve_pairing_shell(&state, pair_code);
    }
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": "this device is not paired with Lucidos",
            "pair_at": "/~/",
        })),
    )
        .into_response()
}

/// Did the handler already say something about the credential cookie?
///
/// Then the refresh stands down. `revoke_device` is the case that matters. It
/// clears the caller's own cookie, and it runs behind this middleware. So a
/// revoke landing on the day a restamp is due would otherwise hand the browser
/// a fresh credential instead of clearing it.
fn response_speaks_for_the_credential(response: &Response) -> bool {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.trim_start().starts_with(auth::COOKIE_DEVICE_CREDENTIAL))
}

/// The same credential this request carried, in a cookie with a fresh window.
///
/// `None` when the request carried no readable credential, or the header will
/// not build. Both leave the existing cookie alone, which is the safe miss: the
/// device stays paired either way, and the server never checks the window.
fn refreshed_credential_cookie(
    state: &GatewayState,
    headers: &HeaderMap,
) -> Option<axum::http::HeaderValue> {
    let credential = auth::presented_credential(headers)?;
    let secure = auth::request_is_secure(headers, state.serves_tls());
    auth::credential_cookie(credential, secure).parse().ok()
}

#[derive(Serialize)]
struct SessionBody {
    paired: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_label: Option<String>,
    /// True when the caller is a local process, which is what may mint the very
    /// first pairing code. No shipped client branches on it: a browser is never
    /// local, and the desktop app mints through its own Rust side instead.
    local: bool,
}

/// Who is calling? Public, so an unpaired client can ask before it is refused.
async fn session(State(state): State<GatewayState>, headers: HeaderMap) -> Json<SessionBody> {
    Json(match state.authorize(&headers) {
        Authorization::LocalProcess => SessionBody {
            paired: true,
            device_id: None,
            device_label: None,
            local: true,
        },
        Authorization::Device { id, label } => SessionBody {
            paired: true,
            device_id: Some(id),
            device_label: Some(label),
            local: false,
        },
        Authorization::Unauthorized => SessionBody {
            paired: false,
            device_id: None,
            device_label: None,
            local: false,
        },
    })
}

/// What the mint call accepts.
///
/// `label` names the device that redeems the code, so `lucidos pair --label`
/// does more than print a name in the terminal. `origin` asks for a QR, and is
/// the address the new device should open. Only the caller knows which of this
/// machine's addresses another device can reach.
#[derive(Deserialize, Default)]
struct PairingCodeQuery {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    origin: Option<String>,
}

/// The minted code, plus the QR when the caller asked for one.
///
/// With no `origin` sent, both extra fields are omitted rather than null. A
/// client that never heard of them sees exactly the body it always did.
#[derive(Serialize)]
struct PairingCodeBody {
    code: String,
    expires_in_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pair_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    qr_svg: Option<String>,
}

/// Mint a one-time pairing code.
///
/// Gated by [`enforce`], so either a local process or an already-paired device
/// may mint one. A paired device is allowed on purpose: it already holds full
/// authority, so refusing it would add no safety and would strand a user who is
/// away from the machine.
async fn pairing_code(
    State(state): State<GatewayState>,
    axum::extract::Query(query): axum::extract::Query<PairingCodeQuery>,
) -> Result<Json<PairingCodeBody>, ApiError> {
    let label = query
        .label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // Validated BEFORE the code is minted. A bad origin is the caller's
    // mistake, and it must not burn a code the user then has to re-request.
    let origin = match query
        .origin
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => Some(pairing_qr::valid_origin(raw).ok_or_else(|| {
            ApiError::bad_request("origin must be a bare http(s) origin, such as https://host:port")
        })?),
        None => None,
    };
    let code = state.mint_pairing_code(label).map_err(ApiError::internal)?;
    let pair_url = origin.map(|o| pairing_qr::pair_url(o, &code));
    // A `None` here means the URL fits in no QR at all, which the length cap in
    // `valid_origin` already rules out. The code is still good, so the response
    // carries it and the page falls back to showing the digits.
    let qr_svg = pair_url.as_deref().and_then(pairing_qr::qr_svg);
    Ok(Json(PairingCodeBody {
        expires_in_secs: auth::pairing_code_ttl_secs(),
        code,
        pair_url,
        qr_svg,
    }))
}

#[derive(Deserialize)]
struct PairRequest {
    code: String,
    /// What to call this device in the paired list. Optional, because a phone
    /// typing a code should not also be made to name itself.
    #[serde(default)]
    label: Option<String>,
}

/// Redeem a pairing code and become a paired device.
///
/// Public by necessity: the caller has no credential yet, which is the point.
/// The code is the only thing standing here, so it is single use and expires.
async fn pair(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<PairRequest>,
) -> Result<Response, ApiError> {
    let credential = state
        .redeem_pairing_code(&body.code, body.label.as_deref())
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::bad_request("that pairing code is not valid or has expired"))?;
    let secure = auth::request_is_secure(&headers, state.serves_tls());
    Ok((
        StatusCode::OK,
        [(
            header::SET_COOKIE,
            auth::credential_cookie(&credential, secure),
        )],
        Json(serde_json::json!({ "paired": true })),
    )
        .into_response())
}

#[derive(Serialize)]
struct DeviceRow {
    id: String,
    label: String,
    paired_at: String,
    /// Omitted for a device paired before the field existed, so the list can
    /// say nothing rather than guess. It fills in on that device's next
    /// request. Never the credential, and never anything finer than a day.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen_at: Option<String>,
}

async fn list_devices(State(state): State<GatewayState>) -> Result<Json<Vec<DeviceRow>>, ApiError> {
    let devices = state.paired_devices();
    Ok(Json(
        devices
            .devices
            .into_iter()
            .map(|d| DeviceRow {
                id: d.id,
                label: d.label,
                paired_at: d.paired_at,
                last_seen_at: d.last_seen_at,
            })
            .collect(),
    ))
}

/// Revoke a device. Revoking the caller's own device clears its cookie in the
/// same response, so the browser does not keep sending a credential that no
/// longer resolves.
async fn revoke_device(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    // Resolve the caller BEFORE revoking. Afterwards its credential matches no
    // stored device, so `authorize` returns `Unauthorized`. A device revoking
    // itself would then never be recognised as having done so, and would keep
    // sending a credential that no longer resolves.
    let revoked_self = matches!(state.authorize(&headers), Authorization::Device { id: caller, .. } if caller == id);
    let removed = state.revoke_device(&id).map_err(ApiError::internal)?;
    if !removed {
        return Err(ApiError::bad_request("no paired device with that id"));
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    if revoked_self {
        if let Ok(value) = auth::cleared_credential_cookie().parse() {
            response.headers_mut().insert(header::SET_COOKIE, value);
        }
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picker_shell_is_public_so_an_unpaired_phone_can_pair() {
        assert!(is_public_path("/~/"));
        assert!(is_public_path("/~/index.html"));
        assert!(is_public_path("/~/assets/index-abc123.js"));
        assert!(is_public_path("/~/sw.js"));
        assert!(is_public_path("/"));
    }

    #[test]
    fn only_the_two_pairing_calls_and_health_are_public_apis() {
        assert!(is_public_path("/~/api/v1/health"));
        assert!(is_public_path("/~/api/v1/auth/pair"));
        assert!(is_public_path("/~/api/v1/auth/session"));

        assert!(!is_public_path("/~/api/v1/auth/pairing-code"));
        assert!(!is_public_path("/~/api/v1/auth/devices"));
        assert!(!is_public_path("/~/api/v1/control/workspaces"));
        assert!(!is_public_path("/~/api/v1/control/workspaces/dev"));
    }

    #[test]
    fn a_code_minted_without_an_origin_serializes_to_the_body_it_always_had() {
        // `lucidos pair` sends no origin and parses this body. A `null` field
        // is a contract change to a caller that may be older than the gateway.
        let body = PairingCodeBody {
            code: "01234567".into(),
            expires_in_secs: 300,
            pair_url: None,
            qr_svg: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert_eq!(json, r#"{"code":"01234567","expires_in_secs":300}"#);
    }

    #[test]
    fn a_workspace_path_is_never_public() {
        assert!(!is_public_path("/dev/"));
        assert!(!is_public_path("/dev/api/v1/threads/list"));
        assert!(!is_public_path("/dev/app/habit-tracker/"));
        assert!(!is_public_path("/personal/data/artifacts/notes.md"));
    }

    #[test]
    fn a_dot_segment_is_never_public() {
        // Paths are not normalized before they reach here, so a traversal must
        // not be able to wear a picker-asset prefix into the exemption list.
        assert!(!is_public_path("/~/assets/../api/v1/control/workspaces"));
        assert!(!is_public_path("/~/.."));
        assert!(!is_public_path("/~/./index.html"));
        assert!(!is_public_path("/../~/index.html"));
        // A dot INSIDE a segment is an ordinary filename, not a traversal.
        assert!(is_public_path("/~/assets/index-abc123.js"));
        assert!(is_public_path("/~/..well-known"));
    }

    #[test]
    fn an_exemption_is_exact_and_is_not_inherited_by_children() {
        // The trap this guards: prefix-matching `api/v1/health` would exempt
        // anything someone later mounts underneath it.
        assert!(!is_public_path("/~/api/v1/health/detail"));
        assert!(!is_public_path("/~/api/v1/auth/pair/steal"));
        assert!(!is_public_path("/~/api/v1/authx"));
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn a_navigation_is_recognised_from_forge_proof_fetch_metadata() {
        assert!(wants_html(&headers(&[("sec-fetch-mode", "navigate")])));
        assert!(!wants_html(&headers(&[("sec-fetch-mode", "cors")])));
        // Fetch metadata wins over Accept: a script `fetch` asking for HTML is
        // still a fetch, and must get 401 rather than a page it cannot use.
        let both = headers(&[("sec-fetch-mode", "cors"), ("accept", "text/html")]);
        assert!(!wants_html(&both));
    }

    #[test]
    fn a_client_with_no_fetch_metadata_falls_back_to_accept() {
        assert!(wants_html(&headers(&[("accept", "text/html,*/*")])));
        assert!(!wants_html(&headers(&[("accept", "application/json")])));
        assert!(!wants_html(&headers(&[])));
    }

    // ── `enforce`, driven through a real router ─────────────────────────────

    /// A gateway with a frontend on disk, so the pairing shell is a real body
    /// rather than "no frontend configured".
    fn state_with_frontend(dir: &std::path::Path) -> GatewayState {
        std::fs::write(
            dir.join("index.html"),
            "<html><head></head><body></body></html>",
        )
        .unwrap();
        GatewayState::for_tests_with_static_dir(Some(dir.to_path_buf()))
    }

    /// The gated surface, behind the real middleware. The inner handler must
    /// never run for an unauthorized caller, so it answers a teapot: seeing one
    /// means `enforce` let the request through.
    fn gated_router(state: GatewayState) -> Router {
        Router::new()
            .fallback(|| async { StatusCode::IM_A_TEAPOT })
            .layer(axum::middleware::from_fn_with_state(state.clone(), enforce))
            .with_state(state)
    }

    async fn get(state: &GatewayState, uri: &str, hdrs: &[(&str, &str)]) -> Response {
        use tower::ServiceExt as _;
        let mut builder = axum::extract::Request::builder().method("GET").uri(uri);
        for (k, v) in hdrs {
            builder = builder.header(*k, *v);
        }
        gated_router(state.clone())
            .oneshot(builder.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn an_unpaired_navigation_gets_the_pairing_screen_where_it_stands() {
        // The migration invariant. A 3xx makes an already-installed PWA serve
        // its stale cached shell instead, and while unpaired it cannot update
        // the worker that would fix that.
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_frontend(dir.path());
        for path in ["/dev/", "/dev/some/deep/route", "/personal/"] {
            let response = get(&state, path, &[("sec-fetch-mode", "navigate")]).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert!(
                response.headers().get(header::LOCATION).is_none(),
                "{path} must be answered in place, never redirected"
            );
            assert_eq!(
                response
                    .headers()
                    .get(crate::server::PAIRING_SHELL_HEADER)
                    .and_then(|v| v.to_str().ok()),
                Some("1"),
                "{path} must mark itself as the pairing screen"
            );
        }
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// The pairing screen served for a request that arrived with a code.
    async fn pairing_shell_for(query: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_frontend(dir.path());
        let uri = format!("/dev/{query}");
        let response = get(&state, &uri, &[("sec-fetch-mode", "navigate")]).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        body_text(response).await
    }

    #[tokio::test]
    async fn an_unpaired_navigation_carries_its_code_into_the_manifest_link() {
        // A scan lands on the public `/~/?pair=`, but the picker's cold-start
        // fast path redirects a remembered workspace and takes the query along.
        // The code has to survive that, or the install it feeds pairs nothing.
        let body = pairing_shell_for("?pair=01234567").await;
        assert!(body.contains("manifest.json?pair=01234567"), "{body}");
    }

    #[tokio::test]
    async fn a_pairing_screen_asked_for_no_code_stamps_none() {
        let body = pairing_shell_for("").await;
        assert!(!body.contains("?pair="), "{body}");
    }

    #[tokio::test]
    async fn nothing_but_a_minted_shape_reaches_the_stamped_link() {
        // The query is caller-supplied and lands in an HTML attribute. One
        // grammar governs it, and it is `valid_pairing_code`.
        for query in [
            "?pair=abc",
            "?pair=0123456",
            "?pair=012345678",
            "?pair=",
            "?pair=0123456%22%3E%3Cscript%3E",
        ] {
            let body = pairing_shell_for(query).await;
            assert!(!body.contains("?pair="), "{query} was echoed: {body}");
        }
    }

    /// A state holding one paired device, and the credential that reaches it.
    fn state_with_device(dir: &std::path::Path, last_seen_at: Option<&str>) -> GatewayState {
        let state = state_with_frontend(dir);
        let credential = "cred-abc";
        state
            .write_paired_devices_for_test(|paired| {
                paired.devices.push(auth::PairedDevice {
                    id: "device-1".into(),
                    label: "My iPhone".into(),
                    credential_digest: auth::digest(credential),
                    paired_at: "2020-01-01T00:00:00Z".into(),
                    last_seen_at: last_seen_at.map(str::to_string),
                });
            })
            .unwrap();
        state
    }

    const DEVICE_COOKIE: (&str, &str) = ("cookie", "lucidos_device=cred-abc");

    #[tokio::test]
    async fn a_device_unseen_for_a_day_is_restamped_and_gets_a_fresh_cookie() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_device(dir.path(), Some("2020-01-02T00:00:00Z"));

        let response = get(&state, "/dev/api/v1/threads/list", &[DEVICE_COOKIE]).await;
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);

        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("a restamped device is handed a fresh window");
        // The same credential, and every attribute the original carried. A
        // refresh that dropped HttpOnly would hand app iframes the credential.
        assert!(cookie.contains("lucidos_device=cred-abc"), "{cookie}");
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Lax"), "{cookie}");
        assert!(cookie.contains("Max-Age="), "{cookie}");

        assert!(
            state.paired_devices().devices[0].last_seen_at.as_deref() > Some("2026-01-01"),
            "the stamp must move to now"
        );
    }

    #[tokio::test]
    async fn a_device_seen_today_is_not_rewritten_and_gets_no_cookie() {
        // The throttle. Without it every authorized request rewrites the whole
        // store and re-sets the cookie, on a path that runs constantly.
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let state = state_with_device(dir.path(), Some(&now));

        let response = get(&state, "/dev/api/v1/threads/list", &[DEVICE_COOKIE]).await;
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        assert_eq!(
            state.paired_devices().devices[0].last_seen_at.as_deref(),
            Some(now.as_str()),
            "a device seen today must not be restamped"
        );
    }

    #[tokio::test]
    async fn a_refresh_never_overwrites_what_the_handler_said_about_the_cookie() {
        // `revoke_device` clears the caller's own cookie and runs behind this
        // middleware. A revoke landing on a day the device was also due a
        // restamp must still clear, not be handed a fresh credential.
        let mut cleared = StatusCode::NO_CONTENT.into_response();
        cleared.headers_mut().insert(
            header::SET_COOKIE,
            auth::cleared_credential_cookie().parse().unwrap(),
        );
        assert!(response_speaks_for_the_credential(&cleared));

        // Another handler's unrelated cookie is not ours, so the refresh rides
        // alongside it rather than standing down or replacing it.
        let mut other = StatusCode::OK.into_response();
        other
            .headers_mut()
            .insert(header::SET_COOKIE, "theme=dark; Path=/".parse().unwrap());
        assert!(!response_speaks_for_the_credential(&other));

        assert!(!response_speaks_for_the_credential(
            &StatusCode::OK.into_response()
        ));
    }

    #[tokio::test]
    async fn a_device_last_seen_years_ago_still_authorizes() {
        // Revocation-only. Age is a liveness hint for the list, and never an
        // input to the auth decision.
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_device(dir.path(), Some("2020-01-02T00:00:00Z"));
        let response = get(&state, "/dev/api/v1/threads/list", &[DEVICE_COOKIE]).await;
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    }

    #[tokio::test]
    async fn a_local_process_is_never_stamped_and_is_handed_no_cookie() {
        // The local token names no device, so there is no row to touch and
        // nothing that should acquire a browser credential.
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_device(dir.path(), Some("2020-01-02T00:00:00Z"));
        let response = get(
            &state,
            "/dev/api/v1/threads/list",
            &[(auth::HEADER_LOCAL_TOKEN, "test-local-token")],
        )
        .await;
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        assert_eq!(
            state.paired_devices().devices[0].last_seen_at.as_deref(),
            Some("2020-01-02T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn a_gateway_with_no_frontend_does_not_call_its_error_the_pairing_screen() {
        // The marker tells a service worker "show this, cache nothing". Putting
        // it on a 404 would make the worker show that instead of the cached
        // shell it falls back to for every other failure.
        let state = GatewayState::for_tests();
        let response = get(&state, "/dev/", &[("sec-fetch-mode", "navigate")]).await;
        assert!(!response.status().is_success());
        assert!(response
            .headers()
            .get(crate::server::PAIRING_SHELL_HEADER)
            .is_none());
    }

    #[tokio::test]
    async fn an_unpaired_fetch_still_gets_a_bare_401() {
        // A pairing screen would be unusable here, and would fail the caller's
        // JSON parse on the way.
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_frontend(dir.path());
        let response = get(
            &state,
            "/dev/api/v1/threads/list",
            &[("sec-fetch-mode", "cors")],
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response
            .headers()
            .get(crate::server::PAIRING_SHELL_HEADER)
            .is_none());
    }

    #[tokio::test]
    async fn a_public_path_and_an_authorized_caller_both_reach_the_handler() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_frontend(dir.path());
        let public = get(&state, "/~/api/v1/health", &[("sec-fetch-mode", "cors")]).await;
        assert_eq!(public.status(), StatusCode::IM_A_TEAPOT);

        let local = get(
            &state,
            "/dev/api/v1/threads/list",
            &[
                ("sec-fetch-mode", "cors"),
                (auth::HEADER_LOCAL_TOKEN, "test-local-token"),
            ],
        )
        .await;
        assert_eq!(local.status(), StatusCode::IM_A_TEAPOT);
    }
}
