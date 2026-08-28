//! Reading and granting handshake-script approvals (ADR 0144).
//!
//! Two routes with deliberately different gates. Reading is safe for anyone
//! who can reach the API, and the Files panel needs it to warn at edit time.
//! Approving is the act that lets a file run as the engine user, so it refuses
//! a browser-shaped caller and belongs to `lucidos handshake approve`.
//!
//! **Why refusing a browser is sound here, when ADR 0144 rules out telling an
//! app from the shell.** Those are different questions. `Sec-Fetch-*` is set by
//! the browser and page JavaScript cannot suppress it, so "this came from a
//! browser at all" is unforgeable. What no header answers is WHICH document in
//! that browser sent it, and this route never asks.

use super::*;
use crate::core::handshake_approvals::{self, ApprovalSource};

/// Whether this request came from inside a browser.
///
/// Fetch metadata is a forbidden header set, so a page cannot add or remove
/// it, and every current browser sends it. `Origin` catches a
/// pre-fetch-metadata browser. A caller presenting neither is the CLI, the e2e
/// suite, or another process on this machine.
fn is_browser_request(headers: &HeaderMap) -> bool {
    headers.keys().any(|k| k.as_str().starts_with("sec-fetch-"))
        || headers.contains_key(axum::http::header::ORIGIN)
}

/// One handshake script the config names, and whether it may run.
#[derive(Serialize)]
pub(super) struct HandshakeScriptState {
    /// Workspace-relative, e.g. `data/scripts/auth/comfort-cloud.py`.
    path: String,
    /// The file is on disk.
    exists: bool,
    /// Its current bytes are recorded, so a proxy call will run it.
    approved: bool,
}

/// The `data/scripts/`-relative path an approval is keyed by, from whatever the
/// caller typed.
///
/// Three spellings reach here and all mean one file: the `apis.json` value
/// (`scripts/auth/x.py`), the workspace-relative one (`data/scripts/auth/x.py`),
/// and an absolute path inside this workspace.
fn approval_key(workspace_path: &std::path::Path, raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("path is required".to_string());
    }
    let rel = match std::path::Path::new(trimmed).is_absolute() {
        true => {
            handshake_approvals::workspace_relative(workspace_path, std::path::Path::new(trimmed))
                .ok_or_else(|| format!("'{}' is outside this workspace", trimmed))?
        }
        false if trimmed.starts_with("data/") => trimmed.to_string(),
        false => format!("data/{}", trimmed),
    };
    if crate::api::is_path_traversal(rel.strip_prefix("data/").unwrap_or(&rel)) {
        return Err(format!("'{}' must be a relative path with no '..'", raw));
    }
    if !rel.starts_with("data/scripts/") {
        return Err(format!(
            "'{}' is not a handshake script: they live under data/scripts/",
            raw
        ));
    }
    Ok(rel)
}

/// GET /api/v1/handshake-scripts, every script `apis.json` names with its
/// approval state. Read-only, so a browser may ask.
pub(super) async fn list_handshake_scripts(State(state): State<AppState>) -> Response {
    let load = crate::api::load_proxy_config(&state.workspace_path);
    let recorded = handshake_approvals::entries(&state.workspace_path);
    let scripts: Vec<HandshakeScriptState> = load
        .handshake_script_paths()
        .into_iter()
        .map(|path| {
            let abs = state.workspace_path.join(&path);
            let bytes = std::fs::read(&abs).ok();
            HandshakeScriptState {
                approved: bytes.as_ref().is_some_and(|b| {
                    recorded.get(&path) == Some(&handshake_approvals::content_hash(b))
                }),
                exists: bytes.is_some(),
                path,
            }
        })
        .collect();
    Json(serde_json::json!({ "scripts": scripts })).into_response()
}

#[derive(Deserialize)]
pub(super) struct ApproveRequest {
    pub path: String,
}

/// POST /api/v1/handshake-scripts/approve, recording a script's current bytes
/// as approved. Non-browser callers only: this is what
/// `lucidos handshake approve` calls.
pub(super) async fn approve_handshake_script(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ApproveRequest>,
) -> Response {
    if is_browser_request(&headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "approving a handshake script is not something a page can do. \
                          Run `lucidos handshake approve <path>`, or ask the Lucidos Agent \
                          to make the edit"
            })),
        )
            .into_response();
    }
    let key = match approval_key(&state.workspace_path, &body.path) {
        Ok(k) => k,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response()
        }
    };
    let abs = state.workspace_path.join(&key);
    let bytes = match std::fs::read(&abs) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": format!("{}: {}", key, e) })),
            )
                .into_response()
        }
    };
    match handshake_approvals::record(&state.workspace_path, &key, &bytes) {
        Ok(changed) => {
            if changed {
                state
                    .engine
                    .event_bus
                    .emit_user_system(
                        &headers,
                        &state.pool,
                        "[Proxy] HandshakeScriptApproved",
                        |actor| crate::engine::event_bus::SystemEvent::HandshakeScriptApproved {
                            path: key.clone(),
                            source: ApprovalSource::Approved,
                            actor,
                        },
                    )
                    .await;
            }
            Json(serde_json::json!({ "approved": true, "path": key, "changed": changed }))
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/handshake-scripts", get(list_handshake_scripts))
        .route("/handshake-scripts/approve", post(approve_handshake_script))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    fn hm(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    /// The gate the approve route runs. An app UI cannot strip fetch metadata,
    /// so it cannot present itself as the CLI.
    #[test]
    fn a_browser_is_recognised_however_it_asks() {
        assert!(is_browser_request(&hm(&[(
            "sec-fetch-site",
            "same-origin"
        )])));
        assert!(is_browser_request(&hm(&[("sec-fetch-mode", "cors")])));
        assert!(is_browser_request(&hm(&[(
            "origin",
            "https://localhost:5251"
        )])));
        // The CLI, the API e2e suite, another process on this machine.
        assert!(!is_browser_request(&HeaderMap::new()));
        assert!(!is_browser_request(&hm(&[("user-agent", "lucidos-cli")])));
    }

    #[test]
    fn every_spelling_of_one_script_resolves_to_the_same_key() {
        let ws = std::path::Path::new("/ws");
        for spelling in [
            "scripts/auth/x.py",
            "data/scripts/auth/x.py",
            "/ws/data/scripts/auth/x.py",
            "  scripts/auth/x.py  ",
        ] {
            assert_eq!(
                approval_key(ws, spelling).unwrap(),
                "data/scripts/auth/x.py",
                "{spelling}"
            );
        }
    }

    /// Approval follows authorship, so there is no tool for it. Otherwise an
    /// app wanting a script blessed would only have to talk the agent into
    /// pressing the button. Knowhow it wrote is one way to try.
    ///
    /// The agent CAN still make a script runnable, by writing the content
    /// itself through `write_file` or `edit_file`. That is the same power as
    /// `run_python`, which it already has, so it is not what this guards.
    #[test]
    fn the_llm_tool_surface_knows_nothing_about_approving() {
        let offenders: Vec<String> = crate::test_support::source_scan::production_sources()
            .into_iter()
            .filter(|(rel, text)| {
                rel.starts_with("llm/tools/") && text.to_lowercase().contains("handshake approve")
            })
            .map(|(rel, _)| rel)
            .collect();
        assert!(
            offenders.is_empty(),
            "an approve tool would let an app launder a script through the agent: {offenders:?}"
        );
    }

    #[test]
    fn a_path_outside_the_script_tree_is_refused() {
        let ws = std::path::Path::new("/ws");
        for bad in [
            "artifacts/notes.md",
            "config/apis.json",
            "scripts/../config/apis.json",
            "/etc/passwd",
            "",
        ] {
            assert!(approval_key(ws, bad).is_err(), "{bad} must be refused");
        }
    }
}
