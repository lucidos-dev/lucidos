//! The gateway control plane — `/~/api/v1/control/*`.
//!
//! Lives behind the reserved sigil namespace (ADR 0014 §2) so it can never
//! collide with a workspace slug. Serves the workspace picker's CRUD: list (with
//! per-workspace health), create (provision a stack), rename (registry-only
//! edit), delete-to-trash, and a manual restart for an unhealthy stack.

use crate::error::ApiError;
use crate::net_config;
use crate::server::{GatewayState, RestoreStatus};
use axum::extract::{DefaultBodyLimit, Multipart, Path, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<GatewayState> {
    Router::new()
        .route("/workspaces", get(list).post(create))
        // Fresh aggregate unread total across running workspaces, computed on
        // demand. The desktop dock badge reads this on its nudge (and periodic
        // tick) so a just-read notification reflects immediately, rather than
        // waiting for the supervise loop's cached `last_unread` to catch up.
        .route("/unread-total", get(unread_total))
        // Restore a local backup archive into a new workspace (picker upload).
        // The body is a multipart upload of a potentially multi-GB `.enc`, so the
        // default 2 MB extractor limit is lifted for this route only.
        .route(
            "/workspaces/restore",
            post(restore).layer(DefaultBodyLimit::disable()),
        )
        .route("/restore-status", get(restore_status).delete(clear_restore))
        // Gateway self-update: is a rebuilt binary waiting, and adopt it (re-exec).
        .route("/gateway/status", get(gateway_status))
        .route("/gateway/reload", post(gateway_reload))
        // Machine-global network bind (the gateway's own bind + the engine
        // inherit toggle) — the picker's Network access control writes
        // ~/.lucidos/network.toml here.
        .route(
            "/network-config",
            get(network_config).put(set_network_config),
        )
        .route("/workspaces/:id/rename", post(rename))
        .route("/workspaces/:id/restart", post(restart))
        .route("/workspaces/:id/stop", post(stop))
        .route("/workspaces/:id/autostart", post(set_autostart))
        // A booting engine reports its current phase here (best-effort) so the
        // boot splash can narrate the wait. Called by the engine during its own
        // startup, before its HTTP server is up — see the engine's
        // `report_boot_phase`.
        .route("/workspaces/:id/boot-phase", post(set_boot_phase))
        // ...and reports here when that startup DIES in a way no retry can fix
        // (chiefly a database migrated by a newer Lucidos). Unlike the phase
        // report this one is awaited by the engine, because the process exits
        // immediately after — see the engine's `boot_failure`.
        .route("/workspaces/:id/boot-failure", post(set_boot_failure))
        .route("/workspaces/:id", delete(delete_workspace))
        // Request-level authorization for the whole destructive control plane.
        .layer(middleware::from_fn(control_authz))
}

// ── Control-plane request authorization ────────────────────────────────────
//
// The control plane (workspace create/restart/stop/delete-to-trash/restore,
// gateway reload) lives at `/~/api/v1/control/*` on the GATEWAY origin. App UIs
// are served same-origin at `/<slug>/app/<id>/` with `allow-same-origin`, so
// without a gate an app's JS could `fetch('/~/api/v1/control/workspaces/<slug>/
// stop', {method:'POST'})` and stop/delete the workspace it runs in.
//
// This is an EXPLICIT, documented gate (see ADR 0014). What it closes and the
// one residual it cannot (same-origin app iframes share the gateway origin, so
// no header is a perfect discriminator):
//
//   * Non-browser clients (the dev launcher / `stop.sh` curl, the engine→gateway
//     boot-phase report, the packaged smoke test) send NO fetch metadata — they
//     are allowed, protected instead by the gateway's loopback bind (default;
//     opened only by explicit `LUCIDOS_GATEWAY_BIND_ALL`).
//   * Cross-site / cross-origin browser requests are rejected via the
//     forge-proof `Sec-Fetch-Site` + `Origin`/`Host` checks (a page cannot set
//     these via `fetch()`). This fully closes the classic CSRF vector.
//   * Browser requests whose `Referer` is an app-iframe document
//     (`/<slug>/app/...`) are rejected. The picker (`/~/...`) and the workspace
//     shell (`/<slug>/...`, no `app` segment) pass.
//
// RESIDUAL: a deliberately malicious *same-origin* app could influence its own
// `Referer` (the fetch `referrer` option accepts same-origin URLs), so the
// Referer block is strong defense-in-depth, not an absolute boundary. The
// complete fix is to serve app iframes from a DISTINCT origin (then they are
// cross-origin and the forge-proof Sec-Fetch/Origin checks alone suffice); that
// is recorded as future work in ADR 0014 and is out of scope for this change.

/// Axum middleware: reject a control-plane request that a browser app iframe (or
/// a cross-site page) originated. Allows non-browser clients and same-origin
/// picker / workspace-shell requests. See the module note above.
async fn control_authz(req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| req.uri().authority().map(|a| a.as_str().to_string()));
    if control_request_allowed(req.headers(), host.as_deref()) {
        next.run(req).await
    } else {
        (
            StatusCode::FORBIDDEN,
            "control plane is not reachable from app iframes or cross-origin requests",
        )
            .into_response()
    }
}

/// Read a header as a trimmed, non-empty `&str`.
fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Pure authorization decision for a control-plane request (extracted so the
/// policy is exhaustively unit-tested). See the module note for the full model.
fn control_request_allowed(headers: &HeaderMap, host: Option<&str>) -> bool {
    let sec_fetch_site = header_str(headers, "sec-fetch-site");
    let origin = header_str(headers, "origin");
    let referer = header_str(headers, "referer");

    // Non-browser client (CLI launcher, engine→gateway report, curl): no fetch
    // metadata at all → allow. Protected by the loopback bind topology.
    if sec_fetch_site.is_none() && origin.is_none() && referer.is_none() {
        return true;
    }

    // Browser-originated. Require same-origin (forge-proof: `fetch()` cannot set
    // `Sec-Fetch-*` or `Origin`). Reject cross-site / same-site.
    if let Some(site) = sec_fetch_site {
        if !matches!(site, "same-origin" | "none") {
            return false;
        }
    }
    // When Origin is present, its authority must match the request Host (covers
    // browsers without Sec-Fetch metadata; another CSRF guard).
    if let (Some(origin), Some(host)) = (origin, host) {
        if !origin_matches_host(origin, host) {
            return false;
        }
    }

    // Reject requests originating from an APP IFRAME document. Defense in depth
    // (see RESIDUAL in the module note).
    if let Some(referer) = referer {
        if referer_is_app_iframe(referer) {
            return false;
        }
    }

    true
}

/// Whether `origin` (e.g. `https://localhost:5251`) has the same authority as
/// the request `host` (e.g. `localhost:5251`). Compares the host:port only.
fn origin_matches_host(origin: &str, host: &str) -> bool {
    let origin_authority = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin)
        .trim_end_matches('/');
    origin_authority.eq_ignore_ascii_case(host)
}

/// Whether a `Referer` URL points at an app-iframe document. App UIs are served
/// at `/<slug>/app/<id>/…`, so the second path segment is `app`. The picker
/// (`/~/…`) and the workspace shell (`/<slug>/…`, no `app` segment) are not.
fn referer_is_app_iframe(referer: &str) -> bool {
    // Strip scheme://authority to get the path; tolerate a bare path too.
    let after_scheme = referer.split_once("://").map(|(_, r)| r).unwrap_or(referer);
    let path = match after_scheme.find('/') {
        // Referer had an authority — path starts at the first '/'.
        Some(idx) if referer.contains("://") => &after_scheme[idx..],
        // No authority (already a path) — use as-is.
        _ if referer.starts_with('/') => referer,
        _ => return false,
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let mut segments = path.split('/').filter(|s| !s.is_empty());
    let _slug = segments.next();
    matches!(segments.next(), Some("app"))
}

/// Reject a malformed workspace id (defense in depth — the path-segment lookup
/// already only matches registered slugs, but a clean 400 beats a 404 for a
/// non-slug input, and guards the trash-path construction in delete).
fn reject_invalid_id(id: &str) -> Result<(), ApiError> {
    if crate::registry::is_valid_id(id) {
        Ok(())
    } else {
        Err(ApiError::bad_request("invalid workspace id"))
    }
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
}

#[derive(Deserialize)]
struct RenameBody {
    name: String,
}

#[derive(Deserialize)]
struct AutostartBody {
    enabled: bool,
}

#[derive(Deserialize)]
struct BootPhaseBody {
    /// Kebab-case phase name (see [`crate::boot_phase::BootPhase::from_wire`]).
    /// An unrecognized value is accepted and ignored (forward-compatible).
    phase: String,
}

#[derive(Deserialize)]
struct BootFailureBody {
    /// The engine's user-facing explanation of why this boot cannot succeed,
    /// rendered verbatim (HTML-escaped) on the splash. The gateway deliberately
    /// does not classify it — the engine is the only side that knows which
    /// migrations it carries.
    message: String,
}

#[derive(Deserialize, Default)]
struct DeleteBody {
    /// Type-the-name confirmation. When present it must match the workspace's
    /// current display name (defense in depth behind the picker's confirm).
    #[serde(default)]
    confirm: Option<String>,
}

async fn list(State(state): State<GatewayState>) -> Json<Value> {
    Json(json!({ "workspaces": state.list_status().await }))
}

/// Fresh aggregate unread total across running workspaces (see
/// [`GatewayState::fresh_unread_total`]). Read by the Tauri desktop dock-badge
/// loop on its nudge + periodic tick.
async fn unread_total(State(state): State<GatewayState>) -> Json<Value> {
    Json(json!({ "total": state.fresh_unread_total().await }))
}

async fn create(
    State(state): State<GatewayState>,
    Json(body): Json<CreateBody>,
) -> Result<Json<Value>, ApiError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("workspace name must not be empty"));
    }
    // Picker "+ New": auto-start off by default — the user opens it now; whether
    // it auto-starts on a future gateway boot is their per-workspace toggle.
    // The gateway returns a typed error: a duplicate display name is a 409 the
    // user can act on, not a 500.
    let status = state.create_workspace(name).await?;
    Ok(Json(json!({ "workspace": status })))
}

/// Restore a local encrypted backup archive into a NEW workspace. Multipart body:
/// `file` (the `.enc`), `key` (base64 backup key), and optional `name` (sent only
/// when the derived name collides with an existing workspace). Streams the upload
/// to a temp file, then hands off to the gateway's restore flow (which validates,
/// provisions, shells out to the engine, and registers the workspace). Returns
/// 200 `{id, name}` once the background restore has started — the picker polls
/// `GET /restore-status` for progress.
async fn restore(
    State(state): State<GatewayState>,
    mut multipart: Multipart,
) -> Result<Json<Value>, ApiError> {
    // Reject a concurrent restore before consuming a (possibly multi-GB) upload.
    if matches!(state.restore_status(), RestoreStatus::Running { .. }) {
        return Err(ApiError::conflict("A restore is already in progress"));
    }

    // The streamed temp archive is removed on ANY early return from here on — a
    // connection dropped mid-upload, a malformed trailing field, or a missing
    // key — via this guard, so a (possibly multi-GB) `.enc` is never orphaned in
    // the temp dir. It's disarmed only when ownership passes to
    // `restore_workspace` (which then owns cleanup).
    let mut guard: Option<TempFileGuard> = None;
    let mut filename = String::new();
    let mut key: Option<String> = None;
    let mut name: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("upload error: {e}")))?
    {
        // Capture the field name as owned so the borrow ends before we move the
        // field into the streaming helper.
        let field_name = field.name().map(|s| s.to_string());
        match field_name.as_deref() {
            Some("file") => {
                filename = field.file_name().unwrap_or_default().to_string();
                let path = std::env::temp_dir().join(format!(
                    "lucidos-restore-{}.enc",
                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
                ));
                let g = TempFileGuard::arm(path);
                stream_field_to_file(field, g.path()).await?;
                guard = Some(g);
            }
            Some("key") => {
                key = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad key field: {e}")))?,
                )
            }
            Some("name") => {
                name = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("bad name field: {e}")))?,
                )
            }
            _ => {}
        }
    }

    let guard = guard.ok_or_else(|| ApiError::bad_request("missing 'file' field"))?;
    let key = key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing backup key"))?;
    let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());

    // Hand the temp file to the restore flow; it removes it when the background
    // restore finishes, or on its own error paths.
    let tmp = guard.disarm();
    let (id, ws_name) = state.restore_workspace(tmp, filename, key, name).await?;
    Ok(Json(json!({ "id": id, "name": ws_name })))
}

/// Removes its path on drop unless [`disarm`](TempFileGuard::disarm)ed. Guards
/// the uploaded restore archive so a multipart error after the file part was
/// streamed never orphans the (possibly multi-GB) temp file.
struct TempFileGuard(Option<std::path::PathBuf>);

impl TempFileGuard {
    fn arm(path: std::path::PathBuf) -> Self {
        Self(Some(path))
    }
    fn path(&self) -> &std::path::Path {
        self.0.as_deref().expect("guard armed")
    }
    fn disarm(mut self) -> std::path::PathBuf {
        self.0.take().expect("guard armed")
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Current restore-flow state for the picker's poll (idle / running+phase /
/// completed / failed).
async fn restore_status(State(state): State<GatewayState>) -> Json<RestoreStatus> {
    Json(state.restore_status())
}

/// Gateway self-update status for the picker's reload control: this process's
/// build id, whether a newer gateway binary is on disk waiting to be adopted, and
/// whether this is a packaged build (the picker hides the dev-only self-reload
/// control when `packaged`).
async fn gateway_status(State(state): State<GatewayState>) -> Json<Value> {
    Json(json!({
        "build_id": state.build_id(),
        "update_available": state.gateway_update_available().await,
        "packaged": state.packaged(),
    }))
}

/// Adopt the on-disk gateway binary by re-exec'ing this process onto it (same
/// PID, supervisor untouched, running engines re-adopted on boot). Returns 202
/// before the re-exec so the picker's request resolves; the gateway then briefly
/// drops while the new image binds, and the picker's poll reconnects.
async fn gateway_reload(State(state): State<GatewayState>) -> Result<StatusCode, ApiError> {
    state
        .reload_gateway()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(StatusCode::ACCEPTED)
}

/// Body for the machine-global network bind write.
#[derive(Deserialize)]
struct NetworkConfigBody {
    /// `loopback` | `all` | a literal IP. Validated server-side.
    gateway_bind: String,
    /// Whether every workspace engine inherits this gateway bind (vs reads its
    /// own per-workspace `network_bind` preference).
    inherit: bool,
}

/// GET /~/api/v1/control/network-config — the machine-global gateway bind +
/// engine-inherit toggle (from `~/.lucidos/network.toml`) plus a best-effort
/// Tailscale `100.x` hint, for the picker's Network access control.
async fn network_config() -> Json<Value> {
    let net = net_config::read_network_toml();
    Json(json!({
        "gateway_bind": net.gateway_bind.unwrap_or_else(|| "loopback".to_string()),
        "inherit": net.engine_inherit,
        "detected_tailscale_ip": net_config::detect_tailscale_ipv4(),
    }))
}

/// PUT /~/api/v1/control/network-config — write the machine-global config.
/// Validated server-side (loopback / all / a parseable IP); takes effect only
/// after a gateway / engine restart (a live socket cannot be re-bound).
async fn set_network_config(Json(body): Json<NetworkConfigBody>) -> Result<StatusCode, ApiError> {
    net_config::validate_bind_input(&body.gateway_bind).map_err(ApiError::bad_request)?;
    // Normalize keyword case so the stored value is canonical.
    let gateway_bind = match body.gateway_bind.trim().to_ascii_lowercase().as_str() {
        "loopback" => "loopback".to_string(),
        "all" => "all".to_string(),
        _ => body.gateway_bind.trim().to_string(),
    };
    net_config::write_network_toml(&gateway_bind, body.inherit)
        .map_err(|e| ApiError::internal(format!("failed to write network.toml: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Dismiss a terminal restore result (back to Idle). 409 while one is running.
async fn clear_restore(State(state): State<GatewayState>) -> Result<StatusCode, ApiError> {
    state.clear_restore_status()?;
    Ok(StatusCode::NO_CONTENT)
}

/// Stream one multipart field to `path` without buffering the whole upload in
/// memory (a backup archive can be many GB).
async fn stream_field_to_file(
    mut field: axum::extract::multipart::Field<'_>,
    path: &std::path::Path,
) -> Result<(), ApiError> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| ApiError::internal(format!("temp file: {e}")))?;
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|e| ApiError::bad_request(format!("upload read: {e}")))?
    {
        file.write_all(&chunk)
            .await
            .map_err(|e| ApiError::internal(format!("temp write: {e}")))?;
    }
    file.flush()
        .await
        .map_err(|e| ApiError::internal(format!("temp flush: {e}")))?;
    Ok(())
}

async fn rename(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(body): Json<RenameBody>,
) -> Result<StatusCode, ApiError> {
    reject_invalid_id(&id)?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("workspace name must not be empty"));
    }
    state.rename_workspace(&id, name).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn restart(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    reject_invalid_id(&id)?;
    state
        .restart_workspace(&id)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(StatusCode::ACCEPTED)
}

/// Stop a workspace's engine but keep its registry entry (it stays listed in the
/// picker as stopped). The dev `stop.sh` calls this so the shared gateway forgets
/// the stack and its supervisor stops respawning the engine.
async fn stop(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    reject_invalid_id(&id)?;
    state
        .stop_workspace(&id)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(StatusCode::ACCEPTED)
}

/// Record the booting engine's current phase for the boot splash. Best-effort
/// telemetry: an unknown phase string is accepted and ignored (a newer engine
/// may report a phase this gateway doesn't render), and an unknown/healthy
/// workspace is a harmless no-op (the splash only renders for a stopped slug;
/// the next healthy probe clears the phase). 204 on success (400 only for a
/// malformed id, which the engine never sends); either way the engine's
/// fire-and-forget caller ignores the response, so a report can't fail the boot.
async fn set_boot_phase(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(body): Json<BootPhaseBody>,
) -> Result<StatusCode, ApiError> {
    reject_invalid_id(&id)?;
    if let Some(phase) = crate::boot_phase::BootPhase::from_wire(&body.phase) {
        state.set_boot_phase(&id, phase);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Record a TERMINAL boot failure for `id`: the engine has determined this boot
/// cannot succeed and is exiting. Unlike [`set_boot_phase`] this is not telemetry
/// — it changes behavior. The gateway renders the message on the splash instead of
/// "Workspace starting…" and stops auto-respawning the engine, because the
/// canonical cause (a database migrated by a newer Lucidos) is not something a
/// restart can resolve.
///
/// An empty message is ignored rather than rendered as a blank splash. 204 on
/// success; 400 only for a malformed id, which the engine never sends.
async fn set_boot_failure(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(body): Json<BootFailureBody>,
) -> Result<StatusCode, ApiError> {
    reject_invalid_id(&id)?;
    let message = body.message.trim();
    if !message.is_empty() {
        // Always TERMINAL: the engine only reports what it has classified as
        // unfixable (its `boot_failure.rs` stays silent when in doubt, so the
        // supervisor keeps retrying). The gateway's own provisioning failures are
        // the ones that can be merely retrying.
        state.set_boot_failure(&id, crate::boot_failure::BootFailure::terminal(message));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Flip a workspace's auto-start flag (registry only; does not start/stop the
/// engine). Drives the picker's per-workspace auto-start toggle.
async fn set_autostart(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(body): Json<AutostartBody>,
) -> Result<StatusCode, ApiError> {
    reject_invalid_id(&id)?;
    state
        .set_autostart(&id, body.enabled)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_workspace(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    body: Option<Json<DeleteBody>>,
) -> Result<StatusCode, ApiError> {
    reject_invalid_id(&id)?;
    let confirm = body.and_then(|Json(b)| b.confirm);
    state
        .delete_workspace(&id, confirm.as_deref())
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod authz_tests {
    use super::*;

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

    const HOST: &str = "localhost:5251";

    #[test]
    fn no_fetch_metadata_is_allowed() {
        // The dev launcher / stop.sh curl, the engine→gateway boot-phase report,
        // the packaged smoke test: no Origin/Sec-Fetch/Referer → allowed
        // (protected by the loopback bind, not this gate).
        assert!(control_request_allowed(&headers(&[]), Some(HOST)));
        assert!(control_request_allowed(
            &headers(&[("user-agent", "curl/8.0")]),
            Some(HOST)
        ));
    }

    #[test]
    fn picker_same_origin_request_is_allowed() {
        let h = headers(&[
            ("sec-fetch-site", "same-origin"),
            ("origin", "https://localhost:5251"),
            ("referer", "https://localhost:5251/~/"),
        ]);
        assert!(control_request_allowed(&h, Some(HOST)));
    }

    #[test]
    fn workspace_shell_request_is_allowed() {
        // The shell at /<slug>/ (e.g. the workspace switcher hitting gateway
        // status/reload) — no `app` segment → allowed.
        let h = headers(&[
            ("sec-fetch-site", "same-origin"),
            ("origin", "https://localhost:5251"),
            ("referer", "https://localhost:5251/dev/"),
        ]);
        assert!(control_request_allowed(&h, Some(HOST)));
    }

    #[test]
    fn app_iframe_request_is_rejected() {
        // The core finding: an app at /<slug>/app/<id>/ must not drive control.
        for referer in [
            "https://localhost:5251/dev/app/habit-tracker/",
            "https://localhost:5251/dev/app/habit-tracker/index.html",
            "https://localhost:5251/myws/app/demo-director/sub/page?x=1",
        ] {
            let h = headers(&[
                ("sec-fetch-site", "same-origin"),
                ("origin", "https://localhost:5251"),
                ("referer", referer),
            ]);
            assert!(
                !control_request_allowed(&h, Some(HOST)),
                "app-iframe referer must be rejected: {referer}"
            );
        }
    }

    #[test]
    fn cross_site_browser_request_is_rejected() {
        for site in ["cross-site", "same-site"] {
            let h = headers(&[
                ("sec-fetch-site", site),
                ("origin", "https://evil.example"),
                ("referer", "https://evil.example/"),
            ]);
            assert!(
                !control_request_allowed(&h, Some(HOST)),
                "Sec-Fetch-Site {site} must be rejected"
            );
        }
    }

    #[test]
    fn origin_host_mismatch_is_rejected() {
        // A browser without Sec-Fetch metadata but a foreign Origin → CSRF.
        let h = headers(&[("origin", "https://attacker.example")]);
        assert!(!control_request_allowed(&h, Some(HOST)));
    }

    #[test]
    fn origin_matches_host_compares_authority() {
        assert!(origin_matches_host(
            "https://localhost:5251",
            "localhost:5251"
        ));
        assert!(origin_matches_host(
            "http://Localhost:5251/",
            "localhost:5251"
        ));
        assert!(!origin_matches_host(
            "https://localhost:5252",
            "localhost:5251"
        ));
        assert!(!origin_matches_host(
            "https://evil.example",
            "localhost:5251"
        ));
    }

    #[test]
    fn referer_is_app_iframe_detects_app_segment_only() {
        assert!(referer_is_app_iframe("https://h/dev/app/x/"));
        assert!(referer_is_app_iframe(
            "https://h:5251/myws/app/demo/index.html?a=1"
        ));
        assert!(referer_is_app_iframe("/dev/app/x")); // bare path tolerated
                                                      // Not app-iframe documents:
        assert!(!referer_is_app_iframe("https://h/~/")); // picker
        assert!(!referer_is_app_iframe("https://h/dev/")); // workspace shell
        assert!(!referer_is_app_iframe("https://h/dev/api/v1/threads")); // shell API
        assert!(!referer_is_app_iframe("https://h/")); // root
        assert!(!referer_is_app_iframe("https://h/appworkspace/")); // slug literally "appworkspace"
    }
}
