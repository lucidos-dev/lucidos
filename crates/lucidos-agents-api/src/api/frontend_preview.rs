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
use crate::engine::frontend_preview::FrontendPreviewStatus;

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

#[derive(Serialize)]
struct PreviewResponse {
    #[serde(flatten)]
    status: FrontendPreviewStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

impl PreviewResponse {
    fn for_request(status: FrontendPreviewStatus, headers: &HeaderMap) -> Self {
        let url = status.port.and_then(|port| {
            preview_url_for_host(
                headers
                    .get(axum::http::header::HOST)
                    .and_then(|v| v.to_str().ok()),
                port,
                status.loopback_only,
            )
        });
        Self { status, url }
    }
}

async fn get_status(State(state): State<AppState>, headers: HeaderMap) -> Json<serde_json::Value> {
    let status = state.engine.frontend_preview_status().await;
    Json(json_of(PreviewResponse::for_request(status, &headers)))
}

async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<StartRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let status = state
        .engine
        .start_frontend_preview(req.thread_id)
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(json_of(PreviewResponse::for_request(
        status, &headers,
    ))))
}

async fn stop(State(state): State<AppState>, headers: HeaderMap) -> Json<serde_json::Value> {
    let status = state.engine.stop_frontend_preview().await;
    Json(json_of(PreviewResponse::for_request(status, &headers)))
}

/// INVARIANT: The preview's URL as seen by whoever asked, built by swapping the port of
/// the `Host` they reached us on.
///
/// The engine cannot know this on its own: the same workspace is `localhost`
/// from the laptop and a Tailscale name from the phone, and a `localhost` link
/// handed to a phone is dead. So each request computes it from its own `Host`.
/// The SSE event carries only the port, and the page builds the same URL from
/// its `location`.
///
/// `None` for a missing or bracket-mismatched `Host`; the caller reports the
/// port instead of guessing a hostname. Always https: the preview never serves
/// plaintext. A `loopback_only` preview answers only `localhost`, the one name
/// its leaf carries, so any other `Host` gets `None` rather than a dead link.
fn preview_url_for_host(host: Option<&str>, port: u16, loopback_only: bool) -> Option<String> {
    let host = host.map(str::trim).filter(|h| !h.is_empty())?;
    let hostname = if let Some(rest) = host.strip_prefix('[') {
        let close = rest.find(']')?;
        &host[..close + 2]
    } else {
        host.split(':').next()?
    };
    if hostname.is_empty() {
        return None;
    }
    if loopback_only {
        let loopback = ["localhost", "127.0.0.1", "[::1]"].contains(&hostname);
        return loopback.then(|| format!("https://localhost:{port}/"));
    }
    Some(format!("https://{hostname}:{port}/"))
}

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
            loopback_only: false,
        };
        let json = json_of(PreviewResponse::for_request(
            status,
            &headers_with_host("phone.tailnet.ts.net:5173"),
        ));
        assert_eq!(json["running"], true);
        assert_eq!(json["port"], 6173);
        assert_eq!(json["url"], "https://phone.tailnet.ts.net:6173/");
    }

    #[test]
    fn a_stopped_preview_answers_with_no_url_to_open() {
        let json = json_of(PreviewResponse::for_request(
            FrontendPreviewStatus::stopped(),
            &headers_with_host("localhost:5173"),
        ));
        assert_eq!(json, serde_json::json!({ "running": false }));
    }

    #[test]
    fn a_running_preview_reached_without_a_host_reports_the_port_alone() {
        let status = FrontendPreviewStatus {
            running: true,
            thread_id: Some(Uuid::nil()),
            port: Some(6173),
            started_at: None,
            worktree: None,
            loopback_only: false,
        };
        let json = json_of(PreviewResponse::for_request(status, &HeaderMap::new()));
        assert_eq!(json["port"], 6173);
        assert!(json.get("url").is_none());
    }

    #[test]
    fn the_preview_url_keeps_the_host_the_requester_used() {
        assert_eq!(
            preview_url_for_host(Some("my-laptop.tailnet.ts.net:5173"), 6173, false).as_deref(),
            Some("https://my-laptop.tailnet.ts.net:6173/")
        );
        assert_eq!(
            preview_url_for_host(Some("localhost:5173"), 6173, false).as_deref(),
            Some("https://localhost:6173/")
        );
        assert_eq!(
            preview_url_for_host(Some("lucidos.local"), 6173, false).as_deref(),
            Some("https://lucidos.local:6173/")
        );
    }

    #[test]
    fn an_ipv6_host_keeps_its_brackets_and_loses_only_the_port() {
        assert_eq!(
            preview_url_for_host(Some("[::1]:5173"), 6173, false).as_deref(),
            Some("https://[::1]:6173/")
        );
        assert_eq!(
            preview_url_for_host(Some("[fd7a:115c::1]"), 6173, false).as_deref(),
            Some("https://[fd7a:115c::1]:6173/")
        );
    }

    #[test]
    fn an_unusable_host_yields_no_url_rather_than_a_guess() {
        assert_eq!(preview_url_for_host(None, 6173, false), None);
        assert_eq!(preview_url_for_host(Some("   "), 6173, false), None);
        assert_eq!(preview_url_for_host(Some(":5173"), 6173, false), None);
        assert_eq!(preview_url_for_host(Some("[::1"), 6173, false), None);
    }

    #[test]
    fn a_loopback_only_preview_links_localhost_and_nothing_else() {
        for host in ["localhost:5173", "127.0.0.1:5173", "[::1]:5173"] {
            assert_eq!(
                preview_url_for_host(Some(host), 6173, true).as_deref(),
                Some("https://localhost:6173/"),
                "{host}"
            );
        }
        let phone = Some("my-laptop.tailnet.ts.net:5173");
        assert_eq!(preview_url_for_host(phone, 6173, true), None);
    }
}
