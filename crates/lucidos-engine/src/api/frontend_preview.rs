//! HTTP surface for the frontend preview (`engine::frontend_preview`): the
//! supervised Vite dev server that shows a coding-agent worktree's frontend
//! before Apply.
//!
//! - `GET  /api/v1/frontend-preview`       what is running, if anything.
//! - `POST /api/v1/frontend-preview/start` `{ "thread_id": "<uuid>" }`.
//! - `POST /api/v1/frontend-preview/stop`
//!
//! Every response carries `url` when a preview is running, built from the
//! `Host` header of THIS request. The engine has no other way to know it: the
//! same workspace is `localhost` from the laptop and a Tailscale name from the
//! phone, and handing a phone a `localhost` link is handing it nothing.
//!
//! Deliberately NOT in the capability parity manifest (ADR 0018). The preview
//! is a development affordance with a live process behind it, not a workspace
//! capability, so it has no LLM tool and no SDK facade. The `lucidos
//! frontend-preview` CLI subcommand is hand-written against these routes.

use axum::{
    extract::State,
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::error::ApiError;
use super::AppState;
use crate::engine::frontend_preview::{preview_url_for_host, FrontendPreviewStatus};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/frontend-preview", get(get_status))
        .route("/frontend-preview/start", post(start))
        .route("/frontend-preview/stop", post(stop))
}

#[derive(Debug, Deserialize)]
struct StartRequest {
    thread_id: Uuid,
}

/// The status, plus the URL this particular requester should open.
#[derive(Serialize)]
struct PreviewResponse {
    #[serde(flatten)]
    status: FrontendPreviewStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

impl PreviewResponse {
    fn build(status: FrontendPreviewStatus, headers: &HeaderMap) -> Self {
        let url = status.port.and_then(|port| {
            preview_url_for_host(
                headers
                    .get(axum::http::header::HOST)
                    .and_then(|v| v.to_str().ok()),
                crate::net_config::tls_scheme(),
                port,
            )
        });
        Self { status, url }
    }
}

async fn get_status(State(state): State<AppState>, headers: HeaderMap) -> Json<serde_json::Value> {
    let status = state.engine.frontend_preview_status().await;
    Json(json_of(PreviewResponse::build(status, &headers)))
}

async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<StartRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Every refusal from the engine is a caller problem it can act on (wrong
    // thread, reclaimed worktree, packaged build), and each one names the path
    // or the missing file, so it is forwarded verbatim rather than flattened
    // into a generic 500.
    let status = state
        .engine
        .start_frontend_preview(req.thread_id)
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(json_of(PreviewResponse::build(status, &headers))))
}

async fn stop(State(state): State<AppState>, headers: HeaderMap) -> Json<serde_json::Value> {
    let status = state.engine.stop_frontend_preview().await;
    Json(json_of(PreviewResponse::build(status, &headers)))
}

/// `PreviewResponse` is a `#[serde(flatten)]` wrapper, which `Json` can only
/// serialize through a self-describing value. Failing to encode it would be a
/// bug in this file, not a runtime condition, so it degrades to a stopped
/// status rather than propagating an error the caller cannot act on.
fn json_of(resp: PreviewResponse) -> serde_json::Value {
    serde_json::to_value(&resp).unwrap_or_else(|_| serde_json::json!({ "running": false }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with_host(host: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::HOST,
            axum::http::HeaderValue::from_str(host).unwrap(),
        );
        h
    }

    #[test]
    fn a_running_preview_answers_with_a_url_on_the_requesters_own_host() {
        let status = FrontendPreviewStatus {
            running: true,
            thread_id: Some(Uuid::nil()),
            port: Some(6173),
            started_at: None,
            worktree: Some("/ws/.lucidos/worktrees/thread-abc12345".into()),
        };
        let json = json_of(PreviewResponse::build(
            status,
            &headers_with_host("phone.tailnet.ts.net:5173"),
        ));
        assert_eq!(json["running"], true);
        assert_eq!(json["port"], 6173);
        assert!(json["url"].as_str().unwrap().starts_with("http")); // scheme follows the engine's own TLS config
        assert!(json["url"]
            .as_str()
            .unwrap()
            .contains("phone.tailnet.ts.net:6173"));
    }

    #[test]
    fn a_stopped_preview_answers_with_no_url_to_open() {
        let json = json_of(PreviewResponse::build(
            FrontendPreviewStatus::stopped(),
            &headers_with_host("localhost:5173"),
        ));
        assert_eq!(json, serde_json::json!({ "running": false }));
    }

    #[test]
    fn a_running_preview_reached_without_a_host_reports_the_port_alone() {
        // A caller with no Host (an HTTP/1.0 client, a hand-rolled probe) still
        // learns where the preview is; it just has to build the URL itself.
        let status = FrontendPreviewStatus {
            running: true,
            thread_id: Some(Uuid::nil()),
            port: Some(6173),
            started_at: None,
            worktree: None,
        };
        let json = json_of(PreviewResponse::build(status, &HeaderMap::new()));
        assert_eq!(json["port"], 6173);
        assert!(json.get("url").is_none());
    }
}
