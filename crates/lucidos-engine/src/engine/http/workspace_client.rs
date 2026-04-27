//! Outbound HTTP from one Lucidos workspace to another.
//!
//! Merges `caller_workspace` / `caller_thread_id` / `caller_event_id` / `mode`
//! into the JSON body of the outbound POST so the receiving engine can build
//! a `MessageOrigin::Workspace` and the route panel can show "from workspace
//! 'personal'" with traceable IDs back to the source thread / event.
//!
//! Note: the receiving engine treats these caller fields as a display hint
//! only — they are user-controllable and MUST NOT be used for authorization.

use crate::engine::thread_events::ActorMode;
use reqwest::Client;
use serde_json::Value;
use uuid::Uuid;

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
    /// `ToolCalled` event for a cross-workspace `lucidos send-thread`).
    pub source_event_id: Option<Uuid>,
    /// Upstream actor mode (Human/Agent/Engine). Becomes the `mode` field on
    /// the receiving `MessageOrigin::Workspace`.
    pub mode: ActorMode,
}

/// Merge the `caller_*` + `mode` fields into a JSON object body.
///
/// Body must serialize to a JSON object (top-level `{...}`); panics otherwise
/// — cross-workspace POSTs always send objects.
pub fn merge_caller_fields(mut body: Value, ctx: &WorkspaceCallCtx) -> Value {
    let obj = body
        .as_object_mut()
        .expect("merge_caller_fields requires the body to serialize to a JSON object");
    obj.insert(
        "caller_workspace".into(),
        Value::String(ctx.self_workspace.clone()),
    );
    if let Some(t) = ctx.source_thread_id {
        obj.insert("caller_thread_id".into(), Value::String(t.to_string()));
    }
    if let Some(e) = ctx.source_event_id {
        obj.insert("caller_event_id".into(), Value::String(e.to_string()));
    }
    // `mode` is mandatory in the receiver — overwrite if the caller already
    // set one, since `ctx.mode` is the authoritative upstream actor.
    obj.insert("mode".into(), Value::String(ctx.mode.as_str().into()));
    body
}

/// POST `body` as JSON to `url`, merging the caller_* fields from `ctx` into
/// the body so the receiver can capture a `MessageOrigin::Workspace`.
pub async fn workspace_post<B: serde::Serialize>(
    client: &Client,
    url: &str,
    body: &B,
    ctx: &WorkspaceCallCtx,
) -> Result<reqwest::Response, Box<dyn std::error::Error + Send + Sync>> {
    let body_json = serde_json::to_value(body)?;
    let merged = merge_caller_fields(body_json, ctx);
    Ok(client.post(url).json(&merged).send().await?)
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
    fn merge_inserts_all_caller_fields_when_ids_present() {
        let body = serde_json::json!({"message": "hi"});
        let merged = merge_caller_fields(body, &ctx());
        assert_eq!(merged["caller_workspace"], "dev");
        assert_eq!(
            merged["caller_thread_id"],
            "00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(
            merged["caller_event_id"],
            "00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(merged["mode"], "human");
        // Existing fields preserved.
        assert_eq!(merged["message"], "hi");
    }

    #[test]
    fn merge_omits_optional_ids_when_none() {
        let body = serde_json::json!({"message": "hi"});
        let ctx = WorkspaceCallCtx {
            self_workspace: "dev".into(),
            source_thread_id: None,
            source_event_id: None,
            mode: ActorMode::Human,
        };
        let merged = merge_caller_fields(body, &ctx);
        assert_eq!(merged["caller_workspace"], "dev");
        assert!(merged.get("caller_thread_id").is_none());
        assert!(merged.get("caller_event_id").is_none());
        assert_eq!(merged["mode"], "human");
    }

    #[test]
    fn merge_overwrites_existing_mode_field() {
        // The caller's `mode` is authoritative — even if the body already
        // contained a different mode, the ctx wins.
        let body = serde_json::json!({"message": "hi", "mode": "human"});
        let ctx = WorkspaceCallCtx {
            self_workspace: "dev".into(),
            source_thread_id: None,
            source_event_id: None,
            mode: ActorMode::Agent,
        };
        let merged = merge_caller_fields(body, &ctx);
        assert_eq!(merged["mode"], "agent");
    }

    #[test]
    fn merge_writes_engine_mode() {
        let body = serde_json::json!({});
        let ctx = WorkspaceCallCtx {
            self_workspace: "dev".into(),
            source_thread_id: None,
            source_event_id: None,
            mode: ActorMode::Engine,
        };
        let merged = merge_caller_fields(body, &ctx);
        assert_eq!(merged["mode"], "engine");
    }

    #[tokio::test]
    async fn workspace_post_sends_caller_fields_in_body() {
        use std::sync::{Arc, Mutex};
        let captured: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
        let captured_for_handler = captured.clone();

        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move |body: axum::Json<Value>| {
                let captured = captured_for_handler.clone();
                async move {
                    *captured.lock().unwrap() = Some(body.0);
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
        let resp = workspace_post(&client, &url, &serde_json::json!({"message": "hi"}), &ctx())
            .await
            .expect("post should succeed");
        assert!(resp.status().is_success());

        let recv = captured.lock().unwrap().clone().expect("body received");
        assert_eq!(recv["caller_workspace"], "dev");
        assert_eq!(
            recv["caller_thread_id"],
            "00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(
            recv["caller_event_id"],
            "00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(recv["mode"], "human");
        assert_eq!(recv["message"], "hi");
    }
}
