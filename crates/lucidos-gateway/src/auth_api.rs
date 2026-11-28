use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use lucidos_frame_capability as frame_capability;

use crate::auth::{self, Authorization};
use crate::error::ApiError;
use crate::pairing_qr;
use crate::registry::SIGIL;
use crate::server::GatewayState;
use lucidos_gateway_tls as tls;

pub fn router() -> Router<GatewayState> {
    Router::new()
        .merge(crate::gateway_protocol::management_router())
        .route("/pairing-code", post(pairing_code))
        .merge(device_routes())
        .merge(crate::users_api::routes())
        .merge(crate::users_api::legacy_routes())
        .merge(crate::device_names_api::import_routes())
        .merge(crate::pairing_requests::routes())
        .route_layer(middleware::from_fn(crate::control::control_authz))
        .route("/session", get(session))
        .route("/pair", post(pair))
}

pub(crate) fn device_routes() -> Router<GatewayState> {
    Router::new()
        .route("/devices", get(list_devices))
        .route("/devices/:id", axum::routing::delete(revoke_device))
        .merge(crate::device_names_api::routes())
}

pub fn is_public_path(path: &str) -> bool {
    if carries_a_dot_segment(path) {
        return false;
    }
    if path == "/" {
        return true;
    }
    let Some(rest) = path.strip_prefix("/~/") else {
        return is_public_app_asset(path);
    };
    if rest == "updates" || rest.starts_with("updates/") {
        return false;
    }
    if !rest.starts_with("api/") {
        return true;
    }
    matches!(
        rest,
        "api/v1/health" | "api/v1/auth/pair" | "api/v1/auth/session"
    )
}

pub(crate) fn carries_a_dot_segment(path: &str) -> bool {
    path.split(['/', '\\']).any(is_dot_segment)
}

fn is_dot_segment(segment: &str) -> bool {
    if segment == "." || segment == ".." {
        return true;
    }
    if !segment.contains('%') {
        return false;
    }
    let spelled_out = segment.to_ascii_lowercase().replace("%2e", ".");
    spelled_out == "." || spelled_out == ".."
}

const APP_ASSET_FILES: [&str; 4] = [
    "/sdk.js",
    "/sdk-prefs.js",
    "/sdk-iframe.css",
    "/sdk-iframe-audio.js",
];

const APP_ASSET_TREES: [&str; 1] = ["/fonts/"];

pub(crate) fn is_public_app_asset(path: &str) -> bool {
    let Some((_, rest)) = split_workspace_path(path) else {
        return false;
    };
    let Some(asset) = rest.strip_prefix("/api/v1").filter(|a| a.starts_with('/')) else {
        return false;
    };
    APP_ASSET_FILES.contains(&asset) || APP_ASSET_TREES.iter().any(|t| asset.starts_with(t))
}

fn split_workspace_path(path: &str) -> Option<(&str, &str)> {
    let after = path.strip_prefix('/')?;
    let cut = after.find('/')?;
    let (slug, rest) = after.split_at(cut);
    if slug.is_empty() {
        return None;
    }
    Some((slug, rest))
}

pub(crate) fn frame_capability_admits(state: &GatewayState, req: &Request) -> bool {
    let Some((slug, rest)) = split_workspace_path(req.uri().path()) else {
        return false;
    };
    let Some((token, target)) = frame_capability::split(rest) else {
        return false;
    };
    if carries_a_dot_segment(req.uri().path())
        || !frame_capability::method_is_read_only(req.method().as_str())
        || req.headers().contains_key(header::UPGRADE)
        || slug.strip_prefix(SIGIL).is_some_and(str::is_empty)
    {
        return false;
    }
    let key = frame_capability::derive_key(state.local_token());
    frame_capability::verify(&key, token, slug, chrono::Utc::now().timestamp())
        .is_some_and(|app_id| frame_capability::admits(target, &app_id))
}

pub(crate) fn path_carries_frame_capability(path: &str) -> bool {
    split_workspace_path(path).is_some_and(|(_, rest)| frame_capability::split(rest).is_some())
}

fn wants_html(headers: &HeaderMap) -> bool {
    if let Some(mode) = headers.get("sec-fetch-mode").and_then(|v| v.to_str().ok()) {
        return mode == "navigate";
    }
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/html"))
}

pub(crate) fn is_top_level_navigation(headers: &HeaderMap) -> bool {
    match headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok()) {
        Some(dest) => dest == "document",
        None => wants_html(headers),
    }
}

const REFUSED_FRAME_PAGE: &str = concat!(
    "<!doctype html><meta charset=\"utf-8\">",
    "<title>Lucidos could not load this</title>",
    "<body style=\"margin:0;display:grid;place-items:center;min-height:100vh;",
    "font:14px/1.5 system-ui,sans-serif;color:#7a828c;text-align:center;padding:1rem\">",
    "<p>Lucidos could not load this file. The app's pass to it expired, ",
    "or this device is not paired.<br>Reload the app to try again.</p>"
);

pub async fn enforce(State(state): State<GatewayState>, mut req: Request, next: Next) -> Response {
    if is_public_path(req.uri().path())
        || frame_capability_admits(&state, &req)
        || crate::relay::carries_a_remote_frame_capability(&req)
    {
        return next.run(req).await;
    }
    if crate::gateway_protocol::is_gateway_api_path(req.uri().path())
        && !carries_a_dot_segment(req.uri().path())
    {
        return next.run(req).await;
    }
    if crate::remote_browser::foreign_private_request(&req) {
        return (
            StatusCode::FORBIDDEN,
            "cross-origin access to private gateway routes is not allowed",
        )
            .into_response();
    }
    let form = auth::CookieForm::of_request(&req);
    let (decision, matched) = state.authorize_with_match(req.headers(), form);
    match decision {
        Authorization::Device { id, label } => {
            let renaming = matched.is_some_and(auth::PresentedCredential::is_renamed);
            let page_load = wants_html(req.headers());
            let due = state.touch_device(&id, chrono::Utc::now());
            let refresh = matched
                .filter(|_| renaming || page_load || due)
                .and_then(|p| refreshed_credential_cookie(&state, form, p));
            req.extensions_mut()
                .insert(auth::AuthenticatedDevice { id, label });
            let cookie_name = form.name(state.device_cookie_name());
            let mut response = next.run(req).await;
            if let Some(cookie) = refresh {
                if !response_speaks_for_the_credential(&response, &cookie_name) {
                    response.headers_mut().append(header::SET_COOKIE, cookie);
                }
            }
            return response;
        }
        Authorization::LocalProcess => {
            req.extensions_mut().insert(auth::AuthenticatedLocalProcess);
            return next.run(req).await;
        }
        Authorization::Unauthorized if admits_agent_restore_caller(&req) => {
            req.extensions_mut().insert(auth::AgentRestoreCaller);
            return next.run(req).await;
        }
        Authorization::Unauthorized => {}
    }
    state.log_device_refusal(req.headers(), form);
    if is_top_level_navigation(req.headers()) {
        return crate::server::serve_pairing_shell(&state);
    }
    if is_nested_frame(req.headers()) {
        return (
            StatusCode::UNAUTHORIZED,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            REFUSED_FRAME_PAGE,
        )
            .into_response();
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

fn admits_agent_restore_caller(req: &Request) -> bool {
    let loopback = req
        .extensions()
        .get::<crate::pairing::Peer>()
        .is_some_and(|peer| !peer.public_listener && peer.remote.ip().to_canonical().is_loopback());
    loopback
        && !crate::pairing::is_forwarded(req.headers())
        && req
            .headers()
            .get(auth::HEADER_AGENT_ORIGIN_TOKEN)
            .is_some_and(|token| !token.is_empty())
        && is_agent_restore_route(req.method(), req.uri().path())
}

fn is_agent_restore_route(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;
    use lucidos_gateway_constants::is_valid_workspace_id;
    let segments: Vec<&str> = path.split('/').collect();
    match (method, segments.as_slice()) {
        (
            &Method::POST,
            ["", "~", "api", "v1", "control", "workspaces", id, "snapshot-restore-caller"],
        )
        | (
            &Method::GET,
            ["", "~", "api", "v1", "control", "workspaces", id, "snapshot-restore-status"],
        ) => is_valid_workspace_id(id),
        (&Method::POST, ["", target, "api", "v1", "snapshots", handle, "restore"]) => {
            is_valid_workspace_id(target)
                && !handle.is_empty()
                && handle.len() <= 128
                && handle
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        }
        _ => false,
    }
}

pub(crate) fn is_nested_frame(headers: &HeaderMap) -> bool {
    frame_capability::is_nested_frame_dest(
        headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok()),
    )
}

fn response_speaks_for_the_credential(response: &Response, name: &str) -> bool {
    let prefix = format!("{name}=");
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.trim_start().starts_with(&prefix))
}

fn refreshed_credential_cookie(
    state: &GatewayState,
    form: auth::CookieForm,
    presented: auth::PresentedCredential<'_>,
) -> Option<axum::http::HeaderValue> {
    auth::credential_cookie(state.device_cookie_name(), presented.value(), form)
        .parse()
        .ok()
}

#[derive(Serialize)]
struct SessionBody {
    paired: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_label: Option<String>,
    local: bool,
}

async fn session(
    State(state): State<GatewayState>,
    connection: tls::Connection,
    headers: HeaderMap,
) -> Json<SessionBody> {
    Json(
        match state.authorize(&headers, auth::CookieForm::of(&headers, &connection)) {
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
        },
    )
}

#[derive(Deserialize, Default)]
struct PairingCodeQuery {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    origin: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    operator: Option<String>,
}

#[derive(Serialize)]
struct PairingCodeBody {
    code: String,
    secret: String,
    expires_in_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pair_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    qr_svg: Option<String>,
}

async fn pairing_code(
    State(state): State<GatewayState>,
    device: Option<axum::Extension<auth::AuthenticatedDevice>>,
    axum::extract::Query(query): axum::extract::Query<PairingCodeQuery>,
) -> Result<Json<PairingCodeBody>, ApiError> {
    let label = query
        .label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
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
    let user_id =
        crate::users_api::requested_user(query.user.as_deref(), query.operator.as_deref())?
            .map(str::to_string);
    if let Some(id) = &user_id {
        if state.directory().user(id).is_none() {
            return Err(ApiError::bad_request("no user with that id"));
        }
    }
    let minted = state
        .mint_pairing(crate::pairing::PairingIntent {
            label,
            user_id,
            grants: Default::default(),
            minter: crate::pairing_requests::minter(device.as_ref()),
        })
        .map_err(ApiError::internal)?;
    let code = minted
        .code
        .ok_or_else(|| ApiError::internal("a device pairing minted no short code"))?;
    let pair_url = origin.map(|o| pairing_qr::pair_url(o, &minted.secret));
    let qr_svg = pair_url.as_deref().and_then(pairing_qr::qr_svg);
    Ok(Json(PairingCodeBody {
        expires_in_secs: crate::pairing::pairing_ttl_secs(),
        code,
        secret: minted.secret,
        pair_url,
        qr_svg,
    }))
}

pub(crate) const PUBLIC_SHORT_CODE_REFUSAL: &str =
    "A typed code works only from another device on this machine's local network or tailnet. \
     Lucidos could not verify this request came from one. \
     Scan the QR code, open the pairing link or use the Pairing secret or link field.";

pub(crate) const REJECTED_PAIRING_SECRET: &str =
    "That pairing secret is invalid, expired or already used. \
     Create a new pairing code and try again.";

#[derive(Deserialize)]
struct PairRequest {
    code: String,
    #[serde(default)]
    label: Option<String>,
}

async fn pair(
    State(state): State<GatewayState>,
    connection: tls::Connection,
    peer: Option<axum::Extension<crate::pairing::Peer>>,
    headers: HeaderMap,
    Json(body): Json<PairRequest>,
) -> Result<Response, ApiError> {
    let channel = crate::pairing::classify(peer.map(|p| p.0), &headers, |local| {
        state.certs().is_tailnet_address(local)
    });
    let credential = match state
        .redeem_pairing(&body.code, channel, body.label.as_deref())
        .map_err(ApiError::internal)?
    {
        crate::server::PairingOutcome::Paired(credential) => credential,
        crate::server::PairingOutcome::Rejected => {
            let message = if crate::pairing::valid_pairing_secret(
                &body.code,
                crate::pairing::PairingKind::Device,
            )
            .is_some()
            {
                REJECTED_PAIRING_SECRET
            } else {
                "that pairing code is not valid or has expired"
            };
            return Err(ApiError::bad_request(message));
        }
        crate::server::PairingOutcome::LocalOnly => {
            return Err(ApiError::bad_request(PUBLIC_SHORT_CODE_REFUSAL))
        }
        crate::server::PairingOutcome::Throttled => {
            return Err(ApiError::too_many_requests(
                "too many wrong pairing codes from this device just now, so this one was not \
                 checked. Wait a minute and try again, or scan the QR code instead.",
            ))
        }
        crate::server::PairingOutcome::AwaitingOwner => {
            return Ok(crate::pairing_requests::awaiting_owner())
        }
        crate::server::PairingOutcome::Declined => {
            return Err(ApiError::forbidden(crate::pairing_requests::DECLINED))
        }
    };
    let form = auth::CookieForm::of(&headers, &connection);
    Ok((
        StatusCode::OK,
        [(
            header::SET_COOKIE,
            auth::credential_cookie(state.device_cookie_name(), &credential, form),
        )],
        Json(serde_json::json!({ "paired": true })),
    )
        .into_response())
}

#[derive(Serialize)]
struct DeviceRow {
    id: String,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // See "User rename wire aliases" in docs/temporary-measures.md.
    operator_id: Option<String>,
    paired_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen_at: Option<String>,
}

async fn list_devices(State(state): State<GatewayState>) -> Result<Json<Vec<DeviceRow>>, ApiError> {
    let directory = state.directory();
    Ok(Json(
        directory
            .devices
            .devices
            .iter()
            .cloned()
            .map(|d| DeviceRow {
                user_id: directory.user_of_device(&d.id).map(str::to_string),
                operator_id: directory.user_of_device(&d.id).map(str::to_string),
                name: directory.device_names.get(&d.id).cloned(),
                id: d.id,
                label: d.label,
                paired_at: d.paired_at,
                last_seen_at: d.last_seen_at,
            })
            .collect(),
    ))
}

async fn revoke_device(
    State(state): State<GatewayState>,
    connection: tls::Connection,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let form = auth::CookieForm::of(&headers, &connection);
    let revoked_self = matches!(state.authorize(&headers, form), Authorization::Device { id: caller, .. } if caller == id);
    let removed = state.revoke_device(&id).map_err(ApiError::internal)?;
    if !removed {
        return Err(ApiError::bad_request("no paired device with that id"));
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    if revoked_self {
        if let Ok(value) = auth::cleared_credential_cookie(state.device_cookie_name(), form).parse()
        {
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
    fn a_code_minted_without_an_origin_only_adds_the_secret() {
        let body = PairingCodeBody {
            code: "01234567".into(),
            secret: "ldp_abcdefghijklmnopqrstuvwxyz".into(),
            expires_in_secs: 300,
            pair_url: None,
            qr_svg: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert_eq!(
            json,
            r#"{"code":"01234567","secret":"ldp_abcdefghijklmnopqrstuvwxyz","expires_in_secs":300}"#
        );
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
        assert!(!is_public_path("/~/assets/../api/v1/control/workspaces"));
        assert!(!is_public_path("/~/.."));
        assert!(!is_public_path("/~/./index.html"));
        assert!(!is_public_path("/../~/index.html"));
        assert!(is_public_path("/~/assets/index-abc123.js"));
        assert!(is_public_path("/~/..well-known"));
    }

    #[test]
    fn an_app_frame_s_own_tags_are_public_under_a_workspace_slug() {
        for path in [
            "/dev/api/v1/sdk.js",
            "/dev/api/v1/sdk-prefs.js",
            "/dev/api/v1/sdk-iframe.css",
            "/dev/api/v1/sdk-iframe-audio.js",
            "/dev/api/v1/fonts/fira-code.css",
            "/myws/api/v1/sdk.js",
        ] {
            assert!(is_public_path(path), "{path}");
        }
    }

    #[test]
    fn nothing_beside_an_app_tag_inherits_the_exemption() {
        for path in [
            "/dev/api/v1/threads/list",
            "/dev/api/v1/data/artifacts/notes.md",
            "/dev/app/habit-tracker/style.css",
            "/dev/api/v1/sdk.js.map",
            "/dev/api/v1/sdk.jsx",
            "/dev/api/v1/sdk-iframe.css.map",
            "/dev/api/v1/fonts",
            "/dev/api/v1/fonts-admin/list",
            "/dev/api/v1/static/html2canvas.min.js",
            "/dev/api/v1beta/sdk.js",
            "/api/v1/sdk.js",
            "//api/v1/sdk.js",
            "/dev",
            "/dev/api/v1/../control/x",
            "/dev/api/v1/./sdk.js",
            "/../dev/api/v1/sdk.js",
            "/~/api/v1/sdk.js",
            "/~/api/v1/fonts/fira-code.css",
        ] {
            assert!(!is_public_path(path), "{path}");
        }
    }

    #[test]
    fn a_tree_exemption_cannot_be_walked_out_of() {
        for path in [
            r"/dev/api/v1/fonts/%2e%2e/threads/list",
            r"/dev/api/v1/fonts/%2E%2E/threads/list",
            r"/dev/api/v1/fonts/.%2e/threads/list",
            r"/dev/api/v1/fonts/%2e./threads/list",
            r"/dev/api/v1/fonts/%2e/%2e%2e/threads/list",
            r"/dev/api/v1/fonts/a/%2e%2e/%2e%2e/credentials",
            r"/dev/api/v1/fonts/..\threads/list",
            r"/dev/api/v1/fonts/%2e%2e\threads/list",
        ] {
            assert!(!is_public_path(path), "{path}");

            let rest = path.strip_prefix("/dev").expect("a slug-prefixed path");
            let upstream =
                reqwest::Url::parse(&format!("http://127.0.0.1:0{rest}")).expect("a parsable url");
            assert!(
                !upstream.path().starts_with("/api/v1/fonts/"),
                "{path} was chosen because it escapes its tree, and it resolved to {}",
                upstream.path()
            );
        }
    }

    #[test]
    fn a_dot_is_a_dot_in_every_spelling_the_parser_collapses() {
        for segment in [".", "..", "%2e", "%2E", "%2e%2e", "%2E%2e", ".%2e", "%2e."] {
            assert!(is_dot_segment(segment), "{segment}");
        }
        for segment in ["...", "%2e%2e%2e", "%2e%2e%2f", "a%2eb", "", "%2f"] {
            assert!(!is_dot_segment(segment), "{segment}");
        }
    }

    fn engine_app_asset_literals() -> Vec<String> {
        let engine = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the gateway crate sits in crates/")
            .join("lucidos-agents-api/src/api/browser_origin.rs");
        let source = std::fs::read_to_string(&engine)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", engine.display()));
        let body = source
            .split_once("fn is_public_app_asset")
            .expect("the engine still defines is_public_app_asset")
            .1;
        let body = body.split_once("\n}").expect("a closed function body").0;
        body.split('"')
            .skip(1)
            .step_by(2)
            .filter(|literal| *literal != "/api/v1")
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn the_two_halves_of_the_app_asset_exemption_agree() {
        let engine = engine_app_asset_literals();
        assert!(
            engine.len() >= APP_ASSET_FILES.len(),
            "the scan read {engine:?}, which is too little to be the real list"
        );

        let (mut trees, mut files): (Vec<&str>, Vec<&str>) = engine
            .iter()
            .map(String::as_str)
            .partition(|literal| literal.ends_with('/'));
        files.sort_unstable();
        trees.sort_unstable();
        let mut want_files = APP_ASSET_FILES;
        let mut want_trees = APP_ASSET_TREES;
        want_files.sort_unstable();
        want_trees.sort_unstable();

        assert_eq!(
            files, want_files,
            "the exact-matched names differ; reconcile with browser_origin.rs"
        );
        assert_eq!(
            trees, want_trees,
            "the prefix-matched trees differ; reconcile with browser_origin.rs"
        );
    }

    #[test]
    fn an_exemption_is_exact_and_is_not_inherited_by_children() {
        assert!(!is_public_path("/~/api/v1/health/detail"));
        assert!(!is_public_path("/~/api/v1/auth/pair/steal"));
        assert!(!is_public_path("/~/api/v1/authx"));
    }

    const TEST_COOKIE: &str = "lucidos_device_deadbeef";

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
        let both = headers(&[("sec-fetch-mode", "cors"), ("accept", "text/html")]);
        assert!(!wants_html(&both));
    }

    #[test]
    fn a_client_with_no_fetch_metadata_falls_back_to_accept() {
        assert!(wants_html(&headers(&[("accept", "text/html,*/*")])));
        assert!(!wants_html(&headers(&[("accept", "application/json")])));
        assert!(!wants_html(&headers(&[])));
    }

    fn state_with_frontend(dir: &std::path::Path) -> GatewayState {
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"name":"Lucidos","start_url":"."}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("index.html"),
            "<html><head></head><body></body></html>",
        )
        .unwrap();
        GatewayState::for_tests_with_static_dir(Some(dir.to_path_buf()))
    }

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

    async fn pairing_shell_for(query: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_frontend(dir.path());
        let uri = format!("/dev/{query}");
        let response = get(&state, &uri, &[("sec-fetch-mode", "navigate")]).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        body_text(response).await
    }

    #[tokio::test]
    async fn missing_or_invalid_optional_manifest_does_not_block_pairing() {
        for source in [None, Some("not json"), Some("[]")] {
            let dir = tempfile::tempdir().unwrap();
            let state = state_with_frontend(dir.path());
            let path = dir.path().join("manifest.json");
            match source {
                Some(source) => std::fs::write(path, source).unwrap(),
                None => std::fs::remove_file(path).unwrap(),
            }
            let response = get(&state, "/dev/", &[("sec-fetch-mode", "navigate")]).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(body_text(response)
                .await
                .contains("id=\"lucidos-pairing-manifest\""));
        }
    }

    #[tokio::test]
    async fn an_unpaired_navigation_keeps_credentials_out_of_manifest_requests() {
        let body = pairing_shell_for("?pair=01234567").await;
        assert!(!body.contains("manifest.json?pair=01234567"), "{body}");
        assert!(body.contains("id=\"lucidos-pairing-manifest\""));
    }

    #[tokio::test]
    async fn a_pairing_screen_asked_for_no_code_stamps_none() {
        let body = pairing_shell_for("").await;
        assert!(!body.contains("?pair="), "{body}");
    }

    #[tokio::test]
    async fn invalid_query_input_is_not_echoed_into_the_shell() {
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

    fn state_with_device(dir: &std::path::Path, last_seen_at: Option<&str>) -> GatewayState {
        let state = state_with_frontend(dir);
        let credential = "cred-abc";
        state
            .seed_device_for_test(auth::PairedDevice {
                id: "device-1".into(),
                label: "My iPhone".into(),
                credential_digest: auth::digest(credential),
                paired_at: "2020-01-01T00:00:00Z".into(),
                last_seen_at: last_seen_at.map(str::to_string),
            })
            .unwrap();
        state
    }

    const LEGACY_DEVICE_COOKIE: (&str, &str) = ("cookie", "lucidos_device=cred-abc");

    fn own_device_cookie(state: &GatewayState) -> String {
        format!("{}=cred-abc", state.device_cookie_name())
    }

    #[tokio::test]
    async fn a_device_unseen_for_a_day_is_restamped_and_gets_a_fresh_cookie() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_device(dir.path(), Some("2020-01-02T00:00:00Z"));

        let own = own_device_cookie(&state);
        let response = get(&state, "/dev/api/v1/threads/list", &[("cookie", &own)]).await;
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);

        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("a restamped device is handed a fresh window");
        assert!(cookie.contains(&own), "{cookie}");
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Lax"), "{cookie}");
        assert!(cookie.contains("Max-Age="), "{cookie}");

        assert!(
            state.paired_devices().devices[0].last_seen_at.as_deref() > Some("2026-01-01"),
            "the stamp must move to now"
        );
    }

    #[tokio::test]
    async fn a_refreshed_cookie_is_hardened_only_on_a_tls_connection() {
        use tower::ServiceExt as _;
        for (connection, form) in [
            (tls::Connection::Tls, auth::CookieForm::Hardened),
            (tls::Connection::Plain, auth::CookieForm::Plain),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let state = state_with_device(dir.path(), Some("2020-01-02T00:00:00Z"));
            let name = form.name(state.device_cookie_name());
            let mut request = axum::extract::Request::builder()
                .uri("/dev/api/v1/threads/list")
                .header("cookie", format!("{name}=cred-abc"))
                .body(axum::body::Body::empty())
                .unwrap();
            request.extensions_mut().insert(connection.clone());
            let response = gated_router(state).oneshot(request).await.unwrap();
            let cookie = set_cookie(&response);
            assert!(
                cookie.starts_with(&format!("{name}=cred-abc;")),
                "{connection:?}: a stale device gets a fresh cookie: {cookie}"
            );
            let hardened = form == auth::CookieForm::Hardened;
            for attribute in ["; Secure", "; Partitioned"] {
                assert_eq!(
                    cookie.contains(attribute),
                    hardened,
                    "{connection:?}: {cookie}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_cookie_of_the_other_form_does_not_authorize() {
        use tower::ServiceExt as _;
        for (connection, other) in [
            (tls::Connection::Tls, auth::CookieForm::Plain),
            (tls::Connection::Plain, auth::CookieForm::Hardened),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let state = state_with_device(dir.path(), None);
            let name = other.name(state.device_cookie_name());
            let mut request = axum::extract::Request::builder()
                .uri("/dev/api/v1/threads/list")
                .header("cookie", format!("{name}=cred-abc"))
                .header("sec-fetch-mode", "cors")
                .body(axum::body::Body::empty())
                .unwrap();
            request.extensions_mut().insert(connection.clone());
            let response = gated_router(state).oneshot(request).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{connection:?} with a {other:?} cookie"
            );
        }
    }

    #[tokio::test]
    async fn a_fetch_by_a_device_seen_today_is_not_rewritten_and_gets_no_cookie() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let state = state_with_device(dir.path(), Some(&now));

        let own = own_device_cookie(&state);
        let response = get(
            &state,
            "/dev/api/v1/threads/list",
            &[("cookie", &own), ("sec-fetch-mode", "cors")],
        )
        .await;
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        assert_eq!(
            state.paired_devices().devices[0].last_seen_at.as_deref(),
            Some(now.as_str()),
            "a device seen today must not be restamped"
        );
    }

    #[tokio::test]
    async fn a_legacy_cookie_still_authorizes_and_is_moved_to_this_gateway_s_name() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let state = state_with_device(dir.path(), Some(&now));

        let response = get(&state, "/dev/api/v1/threads/list", &[LEGACY_DEVICE_COOKIE]).await;
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);

        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("a legacy credential is migrated on sight");
        assert!(cookie.starts_with(state.device_cookie_name()), "{cookie}");
        assert!(cookie.contains("cred-abc"), "{cookie}");
    }

    #[tokio::test]
    async fn a_credential_in_a_second_cookie_field_never_meets_the_pairing_screen() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let state = state_with_device(dir.path(), Some(&now));

        let own = own_device_cookie(&state);
        let response = get(
            &state,
            "/dev/",
            &[
                ("cookie", "theme=dark"),
                ("cookie", &own),
                ("sec-fetch-mode", "navigate"),
            ],
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::IM_A_TEAPOT,
            "the teapot is the handler, so `enforce` let this device through"
        );
        assert!(
            response
                .headers()
                .get(crate::server::PAIRING_SHELL_HEADER)
                .is_none(),
            "a device the store holds must never be asked to pair again"
        );
    }

    #[tokio::test]
    async fn two_gateways_on_one_host_do_not_share_a_cookie_name() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let a = state_with_device(dir_a.path(), None);
        let b = state_with_device(dir_b.path(), None);
        assert_ne!(a.device_cookie_name(), b.device_cookie_name());

        let both = format!(
            "{}=cred-abc; {}=cred-abc",
            a.device_cookie_name(),
            b.device_cookie_name()
        );
        for state in [&a, &b] {
            let response = get(state, "/dev/api/v1/threads/list", &[("cookie", &both)]).await;
            assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
        }
    }

    #[tokio::test]
    async fn a_refresh_never_overwrites_what_the_handler_said_about_the_cookie() {
        for form in [auth::CookieForm::Hardened, auth::CookieForm::Plain] {
            let mut cleared = StatusCode::NO_CONTENT.into_response();
            cleared.headers_mut().insert(
                header::SET_COOKIE,
                auth::cleared_credential_cookie(TEST_COOKIE, form)
                    .parse()
                    .unwrap(),
            );
            assert!(
                response_speaks_for_the_credential(&cleared, &form.name(TEST_COOKIE)),
                "{form:?}"
            );
        }

        let mut other = StatusCode::OK.into_response();
        other
            .headers_mut()
            .insert(header::SET_COOKIE, "theme=dark; Path=/".parse().unwrap());
        assert!(!response_speaks_for_the_credential(&other, TEST_COOKIE));

        assert!(!response_speaks_for_the_credential(
            &StatusCode::OK.into_response(),
            TEST_COOKIE
        ));
    }

    fn set_cookie(response: &Response) -> String {
        response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    #[tokio::test]
    async fn every_page_load_restarts_the_credential_s_window() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let state = state_with_device(dir.path(), Some(&now));
        let own = own_device_cookie(&state);

        let response = get(
            &state,
            "/dev/",
            &[("cookie", &own), ("sec-fetch-mode", "navigate")],
        )
        .await;
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
        let cookie = set_cookie(&response);
        assert!(cookie.contains(&own), "{cookie}");
        assert!(cookie.contains("Max-Age="), "{cookie}");
        assert!(cookie.contains("HttpOnly"), "{cookie}");

        let devices = state.paired_devices().devices;
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].last_seen_at.as_deref(), Some(now.as_str()));
    }

    #[tokio::test]
    async fn a_cookie_left_by_a_data_dir_move_is_migrated_rather_than_refused() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let state = state_with_device(dir.path(), Some(&now));
        let before = auth::device_cookie_name(std::path::Path::new("/tmp/its-old-data-dir"));
        assert_ne!(before, state.device_cookie_name());

        let response = get(
            &state,
            "/dev/api/v1/threads/list",
            &[("cookie", &format!("{before}=cred-abc"))],
        )
        .await;
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
        let cookie = set_cookie(&response);
        assert!(cookie.starts_with(state.device_cookie_name()), "{cookie}");
        assert!(cookie.contains("cred-abc"), "{cookie}");
    }

    #[tokio::test]
    async fn a_dead_value_in_our_own_slot_does_not_lock_the_device_out() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let state = state_with_device(dir.path(), Some(&now));
        let sent = format!(
            "{}=long-dead; lucidos_device=cred-abc",
            state.device_cookie_name()
        );

        let response = get(&state, "/dev/api/v1/threads/list", &[("cookie", &sent)]).await;
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
        let cookie = set_cookie(&response);
        assert!(cookie.starts_with(state.device_cookie_name()), "{cookie}");
        assert!(cookie.contains("cred-abc"), "{cookie}");
    }

    #[tokio::test]
    async fn a_device_last_seen_years_ago_still_authorizes() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_device(dir.path(), Some("2020-01-02T00:00:00Z"));
        let own = own_device_cookie(&state);
        let response = get(&state, "/dev/api/v1/threads/list", &[("cookie", &own)]).await;
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    }

    #[tokio::test]
    async fn a_local_process_is_never_stamped_and_is_handed_no_cookie() {
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
        let state = GatewayState::for_tests();
        let response = get(&state, "/dev/", &[("sec-fetch-mode", "navigate")]).await;
        assert!(!response.status().is_success());
        assert!(response
            .headers()
            .get(crate::server::PAIRING_SHELL_HEADER)
            .is_none());
    }

    #[tokio::test]
    async fn an_unpaired_websocket_upgrade_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_frontend(dir.path());
        let response = get(
            &state,
            "/dev/api/v1/ws-echo",
            &[
                ("connection", "keep-alive, Upgrade"),
                ("upgrade", "websocket"),
                ("sec-websocket-version", "13"),
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response
            .headers()
            .get(crate::server::PAIRING_SHELL_HEADER)
            .is_none());
    }

    #[tokio::test]
    async fn an_unpaired_fetch_still_gets_a_bare_401() {
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
    async fn an_app_frame_loads_its_tags_with_no_cookie_and_reaches_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_frontend(dir.path());
        let as_a_tag = [
            ("sec-fetch-mode", "no-cors"),
            ("sec-fetch-site", "cross-site"),
            ("sec-fetch-dest", "style"),
        ];
        for path in [
            "/dev/api/v1/sdk-iframe.css",
            "/dev/api/v1/sdk-prefs.js?device=abc123",
            "/dev/api/v1/sdk.js",
            "/dev/api/v1/sdk-iframe-audio.js",
            "/dev/api/v1/fonts/fira-code.css",
        ] {
            let response = get(&state, path, &as_a_tag).await;
            assert_eq!(
                response.status(),
                StatusCode::IM_A_TEAPOT,
                "{path} must reach the workspace with no credential"
            );
        }

        let refused = get(&state, "/dev/api/v1/threads/list", &as_a_tag).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
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

    #[test]
    fn only_the_address_bar_counts_as_a_top_level_navigation() {
        assert!(is_top_level_navigation(&headers(&[(
            "sec-fetch-dest",
            "document"
        )])));
        let nested = headers(&[("sec-fetch-mode", "navigate"), ("sec-fetch-dest", "iframe")]);
        assert!(wants_html(&nested), "the older question still says yes");
        assert!(!is_top_level_navigation(&nested));
        for dest in [
            "iframe", "frame", "script", "style", "image", "font", "empty",
        ] {
            let h = headers(&[("sec-fetch-dest", dest), ("accept", "text/html")]);
            assert!(!is_top_level_navigation(&h), "{dest}");
        }
    }

    #[test]
    fn a_client_with_no_fetch_metadata_is_judged_exactly_as_before() {
        assert!(is_top_level_navigation(&headers(&[(
            "accept",
            "text/html,*/*"
        )])));
        assert!(!is_top_level_navigation(&headers(&[(
            "accept",
            "application/json"
        )])));
        assert!(!is_top_level_navigation(&headers(&[])));
    }

    #[tokio::test]
    async fn a_refused_nested_frame_gets_a_short_page_and_never_the_shell() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_frontend(dir.path());
        for dest in ["iframe", "frame"] {
            let response = get(
                &state,
                "/dev/data/artifacts/web/lucidos-me/index.html",
                &[("sec-fetch-mode", "navigate"), ("sec-fetch-dest", dest)],
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{dest}");
            assert!(
                response
                    .headers()
                    .get(crate::server::PAIRING_SHELL_HEADER)
                    .is_none(),
                "{dest} must not be handed the pairing screen"
            );
            let body = body_text(response).await;
            assert!(!body.contains("boot-splash"), "{dest}: {body}");
            assert!(body.len() < 1000, "{dest} got {} bytes", body.len());
            assert!(body.contains("Reload the app"), "{dest}: {body}");
        }
    }

    #[tokio::test]
    async fn a_refused_subresource_still_gets_json() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_frontend(dir.path());
        for dest in ["script", "style", "image", "font", "empty"] {
            let response = get(
                &state,
                "/dev/app/site-publisher/style.css",
                &[("sec-fetch-dest", dest)],
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{dest}");
            let body = body_text(response).await;
            assert!(body.contains("not paired with Lucidos"), "{dest}: {body}");
        }
    }

    #[tokio::test]
    async fn a_real_page_load_still_gets_the_pairing_screen() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_frontend(dir.path());
        let response = get(
            &state,
            "/dev/",
            &[
                ("sec-fetch-mode", "navigate"),
                ("sec-fetch-dest", "document"),
            ],
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(crate::server::PAIRING_SHELL_HEADER)
                .and_then(|v| v.to_str().ok()),
            Some("1"),
        );
    }

    fn capability(slug: &str, app_id: &str) -> String {
        let key = frame_capability::derive_key("test-local-token");
        let now = chrono::Utc::now().timestamp();
        frame_capability::mint(&key, slug, app_id, now, frame_capability::TTL_SECS)
            .expect("a slug-shaped app id mints")
    }

    fn expired_capability(slug: &str, app_id: &str) -> String {
        let key = frame_capability::derive_key("test-local-token");
        let then = chrono::Utc::now().timestamp() - frame_capability::TTL_SECS - 1;
        frame_capability::mint(&key, slug, app_id, then, frame_capability::TTL_SECS)
            .expect("a slug-shaped app id mints")
    }

    #[tokio::test]
    async fn a_frame_capability_carries_an_app_to_its_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_frontend(dir.path());
        let cap = capability("dev", "site-publisher");
        for path in [
            format!("/dev/~cap/{cap}/app/site-publisher/style.css"),
            format!("/dev/~cap/{cap}/app/site-publisher/img/logo.png"),
            format!("/dev/~cap/{cap}/app/site-publisher/"),
            format!("/dev/~cap/{cap}/data/artifacts/web/lucidos-me/index.html"),
            format!("/dev/~cap/{cap}/data/artifacts/web/lucidos-me/five-ai-words/index.html"),
        ] {
            let response = get(&state, &path, &[("sec-fetch-dest", "iframe")]).await;
            assert_eq!(
                response.status(),
                StatusCode::IM_A_TEAPOT,
                "{path} must reach the workspace with no credential"
            );
        }
    }

    #[tokio::test]
    async fn a_frame_capability_reaches_nothing_but_those_two_trees() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_frontend(dir.path());
        let cap = capability("dev", "site-publisher");
        for path in [
            format!("/dev/~cap/{cap}/api/v1/threads/list"),
            format!("/dev/~cap/{cap}/api/v1/credentials"),
            format!("/dev/~cap/{cap}/api/v1/data/config/apis.json"),
            format!("/dev/~cap/{cap}/control/workspaces"),
            format!("/dev/~cap/{cap}/app/habit-tracker/index.html"),
            format!("/dev/~cap/{cap}/app/site-publisher-two/style.css"),
            format!("/dev/~cap/{cap}/data/../api/v1/threads/list"),
            format!("/dev/~cap/{cap}/data/%2e%2e/api/v1/threads/list"),
            format!("/personal/~cap/{cap}/data/artifacts/notes.md"),
            format!(
                "/dev/~cap/{}/app/site-publisher/style.css",
                capability("dev", "habit-tracker")
            ),
            format!(
                "/dev/~cap/{}/data/artifacts/notes.md",
                expired_capability("dev", "site-publisher")
            ),
            "/dev/~cap/forged/data/artifacts/notes.md".to_string(),
            "/dev/~cap//data/artifacts/notes.md".to_string(),
            "/dev/data/artifacts/notes.md".to_string(),
        ] {
            let response = get(&state, &path, &[("sec-fetch-dest", "iframe")]).await;
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{path} must stay refused"
            );
        }
    }

    #[tokio::test]
    async fn the_picker_namespace_gets_no_capability_branch() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_frontend(dir.path());
        let cap = capability("~", "site-publisher");
        for path in [
            format!("/~/~cap/{cap}/data/artifacts/notes.md"),
            format!("/~/~cap/{cap}/api/v1/control/workspaces"),
            format!("/~/~cap/{cap}/api/v1/auth/devices"),
        ] {
            let req = axum::extract::Request::builder()
                .method("GET")
                .uri(&path)
                .body(axum::body::Body::empty())
                .unwrap();
            assert!(!frame_capability_admits(&state, &req), "{path}");
        }

        let gated = get(
            &state,
            "/~/api/v1/auth/devices",
            &[("sec-fetch-dest", "empty")],
        )
        .await;
        assert_eq!(gated.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn only_a_read_rides_a_frame_capability() {
        use tower::ServiceExt as _;
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_frontend(dir.path());
        let cap = capability("dev", "site-publisher");
        let uri = format!("/dev/~cap/{cap}/data/artifacts/notes.md");

        for method in ["POST", "PUT", "DELETE", "PATCH"] {
            let request = axum::extract::Request::builder()
                .method(method)
                .uri(&uri)
                .body(axum::body::Body::empty())
                .unwrap();
            let response = gated_router(state.clone()).oneshot(request).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} must not ride a read-only pass"
            );
        }

        let handshake = get(
            &state,
            &uri,
            &[("connection", "Upgrade"), ("upgrade", "websocket")],
        )
        .await;
        assert_eq!(handshake.status(), StatusCode::UNAUTHORIZED);
    }

    async fn auth_plane_call(method: &str, path: &str, hdrs: &[(&str, &str)]) -> StatusCode {
        use tower::ServiceExt as _;
        let state = GatewayState::for_tests();
        let mut builder = axum::extract::Request::builder()
            .method(method)
            .uri(path)
            .header(auth::HEADER_LOCAL_TOKEN, "test-local-token");
        for (k, v) in hdrs {
            builder = builder.header(*k, *v);
        }
        crate::server::gateway_router(state)
            .oneshot(builder.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    const FROM_AN_APP_DOCUMENT: &[(&str, &str)] = &[
        ("sec-fetch-site", "same-origin"),
        ("referer", "https://localhost:5251/dev/app/habit-tracker/"),
    ];

    #[tokio::test]
    async fn an_app_document_cannot_mint_list_or_revoke_credentials() {
        for (method, path) in [
            ("POST", "/~/api/v1/auth/pairing-code"),
            ("GET", "/~/api/v1/auth/devices"),
            ("DELETE", "/~/api/v1/auth/devices/no-such-device"),
            ("GET", "/~/api/v1/auth/users"),
            ("POST", "/~/api/v1/auth/users"),
            ("PUT", "/~/api/v1/auth/devices/no-such-device/user"),
        ] {
            assert_eq!(
                auth_plane_call(method, path, FROM_AN_APP_DOCUMENT).await,
                StatusCode::FORBIDDEN,
                "{method} {path} must refuse an app document"
            );
        }
    }

    #[tokio::test]
    async fn the_picker_and_the_cli_still_reach_the_auth_plane() {
        let from_the_picker = &[
            ("sec-fetch-site", "same-origin"),
            ("referer", "https://localhost:5251/~/"),
        ];
        for hdrs in [&from_the_picker[..], &[]] {
            assert_eq!(
                auth_plane_call("GET", "/~/api/v1/auth/devices", hdrs).await,
                StatusCode::OK
            );
        }
    }

    #[tokio::test]
    async fn the_pre_auth_calls_stay_outside_the_app_document_gate() {
        assert_ne!(
            auth_plane_call("GET", "/~/api/v1/auth/session", FROM_AN_APP_DOCUMENT).await,
            StatusCode::FORBIDDEN
        );
    }
}
