//! Outbound HTTP from one CognOS workspace to another.
//!
//! Stamps the `X-Cognos-Workspace` / `X-Cognos-Thread-Id` / `X-Cognos-Event-Id`
//! headers so the receiving engine can build a `MessageOrigin::Workspace` and
//! the route panel can show "from workspace 'personal'" with traceable IDs
//! back to the source thread / event.
//!
//! Note: the receiving engine treats these headers as a display hint only —
//! they are user-controllable and MUST NOT be used for authorization.

use crate::engine::thread_events::ActorMode;
use reqwest::Client;
use serde::Serialize;
use uuid::Uuid;

pub const HEADER_WORKSPACE: &str = "x-cognos-workspace";
pub const HEADER_THREAD_ID: &str = "x-cognos-thread-id";
pub const HEADER_EVENT_ID: &str = "x-cognos-event-id";
pub const HEADER_MODE: &str = "x-cognos-mode";

/// Source context for an outbound workspace call. The receiving engine uses
/// these to populate `MessageOrigin::Workspace` on the resulting
/// `MessageReceived` event.
#[derive(Debug, Clone)]
pub struct WorkspaceCallCtx {
    /// Name of the calling workspace (e.g. "dev", "personal").
    pub self_workspace: String,
    /// Thread in the calling workspace that initiated this call.
    pub source_thread_id: Option<Uuid>,
    /// Event in the calling workspace that initiated this call (e.g. the
    /// `ToolCalled` event for a cross-workspace `run_thread`).
    pub source_event_id: Option<Uuid>,
    /// Upstream actor mode (Human/Agent/Engine). Sent as the `X-Cognos-Mode`
    /// header so the receiving engine knows whether the call is human,
    /// LLM-driven, or engine-internal.
    pub mode: ActorMode,
}

/// Add the `X-Cognos-*` source headers to an outbound request.
///
/// Pulled out of `workspace_post` so the header construction can be
/// inspected in unit tests without standing up a mock HTTP server.
pub fn add_workspace_headers(
    builder: reqwest::RequestBuilder,
    ctx: &WorkspaceCallCtx,
) -> reqwest::RequestBuilder {
    let mut b = builder
        .header(HEADER_WORKSPACE, &ctx.self_workspace)
        .header(HEADER_MODE, ctx.mode.as_str());
    if let Some(t) = ctx.source_thread_id {
        b = b.header(HEADER_THREAD_ID, t.to_string());
    }
    if let Some(e) = ctx.source_event_id {
        b = b.header(HEADER_EVENT_ID, e.to_string());
    }
    b
}

/// POST `body` as JSON to `url`, stamping the `X-Cognos-*` source headers
/// from `ctx` so the receiving engine can capture a `MessageOrigin::Workspace`.
pub async fn workspace_post<B: Serialize>(
    client: &Client,
    url: &str,
    body: &B,
    ctx: &WorkspaceCallCtx,
) -> Result<reqwest::Response, Box<dyn std::error::Error + Send + Sync>> {
    let req = add_workspace_headers(client.post(url), ctx).json(body);
    Ok(req.send().await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::thread_events::ActorMode;

    fn ctx() -> WorkspaceCallCtx {
        WorkspaceCallCtx {
            self_workspace: "dev".into(),
            source_thread_id: Some(Uuid::nil()),
            source_event_id: Some(Uuid::nil()),
            mode: ActorMode::Human,
        }
    }

    #[test]
    fn add_headers_sets_all_three_when_ids_present() {
        let client = Client::new();
        let req = add_workspace_headers(client.post("http://example/"), &ctx())
            .build()
            .expect("build request");
        let h = req.headers();
        assert_eq!(h.get(HEADER_WORKSPACE).unwrap(), "dev");
        assert_eq!(
            h.get(HEADER_THREAD_ID).unwrap(),
            "00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(
            h.get(HEADER_EVENT_ID).unwrap(),
            "00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn add_headers_omits_optional_ids_when_none() {
        let client = Client::new();
        let ctx = WorkspaceCallCtx {
            self_workspace: "dev".into(),
            source_thread_id: None,
            source_event_id: None,
            mode: ActorMode::Human,
        };
        let req = add_workspace_headers(client.post("http://example/"), &ctx)
            .build()
            .expect("build request");
        let h = req.headers();
        assert_eq!(h.get(HEADER_WORKSPACE).unwrap(), "dev");
        assert!(h.get(HEADER_THREAD_ID).is_none());
        assert!(h.get(HEADER_EVENT_ID).is_none());
    }

    #[test]
    fn workspace_call_ctx_writes_mode_header_agent() {
        let client = reqwest::Client::new();
        let ctx = WorkspaceCallCtx {
            self_workspace: "dev".into(),
            source_thread_id: None,
            source_event_id: None,
            mode: ActorMode::Agent,
        };
        let req = add_workspace_headers(client.post("http://example/"), &ctx)
            .build()
            .expect("build request");
        assert_eq!(req.headers().get(HEADER_MODE).unwrap(), "agent");
    }

    #[test]
    fn workspace_call_ctx_writes_mode_header_engine() {
        let client = reqwest::Client::new();
        let ctx = WorkspaceCallCtx {
            self_workspace: "dev".into(),
            source_thread_id: None,
            source_event_id: None,
            mode: ActorMode::Engine,
        };
        let req = add_workspace_headers(client.post("http://example/"), &ctx)
            .build()
            .expect("build request");
        assert_eq!(req.headers().get(HEADER_MODE).unwrap(), "engine");
    }

    #[tokio::test]
    async fn workspace_post_sends_headers_to_receiving_server() {
        // Spin up a local axum server that captures inbound headers, then
        // assert the helper actually transmits them.
        use std::sync::{Arc, Mutex};

        let captured: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_for_handler = captured.clone();

        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move |headers: axum::http::HeaderMap| {
                let captured = captured_for_handler.clone();
                async move {
                    let mut c = captured.lock().unwrap();
                    for name in [HEADER_WORKSPACE, HEADER_THREAD_ID, HEADER_EVENT_ID] {
                        if let Some(v) = headers.get(name) {
                            c.push((name.to_string(), v.to_str().unwrap().to_string()));
                        }
                    }
                    axum::http::StatusCode::OK
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let url = format!("http://{}/", addr);
        let client = Client::new();
        let resp = workspace_post(&client, &url, &serde_json::json!({"x": 1}), &ctx())
            .await
            .expect("post should succeed");
        assert!(resp.status().is_success());

        let recorded = captured.lock().unwrap().clone();
        let names: Vec<&str> = recorded.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&HEADER_WORKSPACE));
        assert!(names.contains(&HEADER_THREAD_ID));
        assert!(names.contains(&HEADER_EVENT_ID));
    }
}
