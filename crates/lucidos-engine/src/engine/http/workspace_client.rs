//! Outbound HTTP from one Lucidos workspace to another.
//!
//! Merges `caller_workspace` / `caller_thread_id` / `caller_event_id` / `mode`
//! into the JSON body of the outbound POST so the receiving engine can build
//! a `MessageOrigin::Workspace` and the route panel can show "from workspace
//! 'myws'" with traceable IDs back to the source thread / event.
//!
//! Note: the receiving engine treats these caller fields as a display hint
//! only — they are user-controllable and MUST NOT be used for authorization.

use crate::engine::http::workspace_resolver::CrossWorkspaceTarget;
use crate::engine::thread_events::ActorMode;
use reqwest::Client;
use serde_json::Value;
use std::sync::LazyLock;
use std::time::Duration;
use uuid::Uuid;

/// Per-request timeout for cross-workspace POSTs. A peer engine that accepts
/// the connection and never responds would otherwise pin the calling thread
/// until the surrounding turn is canceled. Generous enough to cover a peer
/// under load setting up a `/api/v1/chat/stream` response, but bounded.
const CROSS_WORKSPACE_TIMEOUT: Duration = Duration::from_secs(30);

/// Build the reqwest client used for cross-workspace POSTs. Extracted so the
/// timeout / TLS knobs live in one place and tests can verify the timeout
/// fires against a hung peer without paying the production 30-second cost.
///
/// - `no_proxy()` + `danger_accept_invalid_certs(true)` is the loopback pair
///   every other engine/gateway client uses (`api/proxy.rs`, `api/history.rs`,
///   `boot_report.rs`, `boot_failure.rs`, the gateway's `proxy.rs` / `stack.rs`)
///   and the one `.claude/rules/rust.md` prescribes: each workspace engine
///   ships a self-signed cert, and every target here is `localhost`, so an
///   `HTTP_PROXY` / `HTTPS_PROXY` in the environment must never be consulted.
/// - `timeout(timeout)` bounds each request so a hung peer can't pin the
///   calling thread (see `harden-engine-workspace-client-no-timeout` in the
///   work tracker).
pub(crate) fn build_cross_workspace_client(timeout: Duration) -> reqwest::Result<Client> {
    Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .timeout(timeout)
        .build()
}

/// Shared HTTP client for cross-workspace POSTs. Lazily built once so
/// repeated calls reuse the connection pool and the rustls context —
/// `Client::builder().build()` is non-trivial. Production callers (chat
/// `run_coding_agent` cross-workspace spawn, in-tree tests against a local
/// axum listener) all go through this so any TLS / timeout policy lives in
/// one place. See `build_cross_workspace_client` for the knobs.
pub fn cross_workspace_http_client() -> &'static Client {
    static CLIENT: LazyLock<Client> = LazyLock::new(|| {
        build_cross_workspace_client(CROSS_WORKSPACE_TIMEOUT)
            .expect("cross-workspace HTTP client builder")
    });
    &CLIENT
}

/// Source context for an outbound workspace call. The receiving engine uses
/// these to populate `MessageOrigin::Workspace` on the resulting
/// `MessageReceived` event.
#[derive(Debug, Clone)]
pub struct WorkspaceCallCtx {
    /// Name of the calling workspace (e.g. "dev", "myws").
    pub self_workspace: String,
    /// Thread in the calling workspace that initiated this call.
    pub source_thread_id: Option<Uuid>,
    /// Event in the calling workspace that initiated this call (e.g. the
    /// `ToolCalled` event for a cross-workspace `lucidos spawn-thread`).
    pub source_event_id: Option<Uuid>,
    /// Upstream actor mode (Human/Agent/Engine). Becomes the `mode` field on
    /// the receiving `MessageOrigin::Workspace`.
    pub mode: ActorMode,
}

/// Merge the `caller_*` + `mode` fields into a JSON object body.
///
/// Body must serialize to a JSON object (top-level `{...}`); panics otherwise
/// — cross-workspace POSTs always send objects.
pub(crate) fn merge_caller_fields(mut body: Value, ctx: &WorkspaceCallCtx) -> Value {
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
/// the body so the receiver can capture a `MessageOrigin::Workspace`, and
/// asserting `target_workspace` so a stale or mis-resolved port cannot land
/// this write on somebody else's engine (`api::target_workspace`).
///
/// The assertion is a required parameter rather than something the one caller
/// remembers to add, because forgetting it fails silently in exactly the way
/// the header exists to prevent: the request succeeds against the wrong
/// workspace and looks like a delivery.
pub async fn workspace_post<B: serde::Serialize>(
    client: &Client,
    url: &str,
    body: &B,
    ctx: &WorkspaceCallCtx,
    target_workspace: &str,
) -> Result<reqwest::Response, Box<dyn std::error::Error + Send + Sync>> {
    let body_json = serde_json::to_value(body)?;
    let merged = merge_caller_fields(body_json, ctx);
    Ok(client
        .post(url)
        .header(crate::api::actor::HEADER_TARGET_WORKSPACE, target_workspace)
        .json(&merged)
        .send()
        .await?)
}

/// The user-facing parameters for spawning a coding-agent thread in another
/// workspace: the prompt plus the optional title / target folder / backend /
/// model / effort. Grouped because they always travel together from the caller
/// down to the request-body builder; passing them as a unit keeps the spawn
/// helper under clippy's argument-count limit without an artificial wrapper.
pub struct CrossWorkspaceSpawn<'a> {
    /// The instruction sent to the spawned coding-agent thread.
    pub prompt: &'a str,
    /// Optional thread title; omitted from the body when `None`/empty.
    pub title: Option<&'a str>,
    /// Deprecated alias for `folder` (a registered repo id/name).
    pub repo: Option<&'a str>,
    /// Canonical target folder for the spawn (`data/apps/<id>/` or a repo root).
    pub folder: Option<&'a str>,
    /// Which backend drives the thread; receiver defaults to Claude Code.
    pub coding_agent: Option<crate::runtime::CodingAgent>,
    /// Model pin, already validated against `coding_agent`'s picker by the
    /// caller. Sent as `cc_model`, the field the receiving engine's
    /// `ChatRequest` reads for a coding-agent thread. Omitted when `None`, so
    /// the receiver applies its own default.
    pub model: Option<&'a str>,
    /// Thinking-budget pin, sent as `reasoning_effort`. Omitted when `None`.
    pub reasoning_effort: Option<&'a str>,
}

/// Build the `/api/v1/chat/stream` body for a cross-workspace coding-agent spawn.
///
/// Pre-generates `thread_id` (caller-supplied) so the caller can return a
/// workspace-qualified link without a second request. Mirrors `lucidos
/// spawn-thread --cc` / `--codex` body shape (see
/// `crates/lucidos-cli/src/spawn_thread.rs`) so both paths land equivalent
/// threads in the receiving engine.
pub(crate) fn build_cross_workspace_coding_agent_body(
    cc_thread_id: Uuid,
    spawn: &CrossWorkspaceSpawn<'_>,
) -> Value {
    let mut body = serde_json::json!({
        "message": spawn.prompt,
        "thread_id": cc_thread_id.to_string(),
        "use_coding_agent": true,
    });
    let obj = body.as_object_mut().expect("json! built an object");
    // `cc_model` rather than `model`: on the receiving `ChatRequest`, `model` is
    // the CHAT model and `cc_model` is the coding-agent one. Sending the wrong
    // key would set a chat model on a thread that never runs a chat turn, and
    // the session would take the backend default with nothing to show for it.
    for (key, value) in [
        ("title", spawn.title),
        ("repo_id", spawn.repo),
        ("folder", spawn.folder),
        ("cc_model", spawn.model),
        ("reasoning_effort", spawn.reasoning_effort),
    ] {
        if let Some(v) = value.filter(|s| !s.is_empty()) {
            obj.insert(key.into(), Value::String(v.to_string()));
        }
    }
    if let Some(agent) = spawn.coding_agent {
        obj.insert(
            "coding_agent".into(),
            Value::String(agent.as_str().to_string()),
        );
    }
    body
}

/// Spawn a coding-agent thread in another workspace.
///
/// POSTs to the target workspace's `/api/v1/chat/stream` with `use_coding_agent:
/// true` and the caller_* / mode fields merged in via `workspace_post`.
/// Returns the pre-generated `thread_id` on success so the caller can build
/// the user-facing link.
///
/// `scheme` is never hardcoded: production passes the TARGET's own
/// `CrossWorkspaceTarget::proto`, which `workspace_resolver` reads from that
/// workspace's `.lucidos/ports` (`PROTO=`, defaulting to `https`), so a peer
/// that serves plain HTTP (the packaged gateway strips the TLS env pair) is
/// still reachable. Tests pass "http" to avoid TLS setup against a plain axum
/// listener.
pub async fn spawn_coding_agent_in_workspace(
    client: &Client,
    target: &CrossWorkspaceTarget,
    spawn: &CrossWorkspaceSpawn<'_>,
    ctx: &WorkspaceCallCtx,
    scheme: &str,
) -> Result<Uuid, String> {
    let cc_thread_id = Uuid::new_v4();
    let body = build_cross_workspace_coding_agent_body(cc_thread_id, spawn);
    let url = format!(
        "{}://localhost:{}/api/v1/chat/stream",
        scheme, target.api_port
    );
    // Assert the workspace this spawn is FOR, taken from the resolved target's
    // own path rather than from anything the caller typed, so the assertion and
    // the port it was resolved alongside always name the same workspace.
    let target_workspace = target
        .workspace_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let resp = workspace_post(client, &url, &body, ctx, &target_workspace)
        .await
        .map_err(|e| format!("cross-workspace POST to {} failed: {}", url, e))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "cross-workspace POST to {} returned HTTP {}: {}",
            url, status, text
        ));
    }
    Ok(cc_thread_id)
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
        let client = cross_workspace_http_client();
        let resp = workspace_post(
            client,
            &url,
            &serde_json::json!({"message": "hi"}),
            &ctx(),
            "target-ws",
        )
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

    #[test]
    fn build_coding_agent_body_includes_required_fields_and_optional_when_set() {
        let tid = Uuid::new_v4();
        let body = build_cross_workspace_coding_agent_body(
            tid,
            &CrossWorkspaceSpawn {
                prompt: "do the thing",
                title: Some("My title"),
                repo: Some("Lucidos"),
                folder: None,
                coding_agent: Some(crate::runtime::CodingAgent::Codex),
                model: Some("gpt-5.6-luna"),
                reasoning_effort: Some("low"),
            },
        );
        assert_eq!(body["message"], "do the thing");
        // `cc_model`, never `model`: the receiver reads the latter as the CHAT
        // model, which a coding-agent thread never uses.
        assert_eq!(body["cc_model"], "gpt-5.6-luna");
        assert_eq!(body["reasoning_effort"], "low");
        assert!(
            body.get("model").is_none(),
            "a coding-agent spawn must not set the chat model key: {body}"
        );
        assert_eq!(body["thread_id"], tid.to_string());
        assert_eq!(body["use_coding_agent"], true);
        assert_eq!(body["title"], "My title");
        assert_eq!(body["repo_id"], "Lucidos");
        assert_eq!(body["coding_agent"], "codex");
    }

    #[test]
    fn build_coding_agent_body_includes_folder_when_set() {
        let tid = Uuid::new_v4();
        let body = build_cross_workspace_coding_agent_body(
            tid,
            &CrossWorkspaceSpawn {
                prompt: "do the thing",
                title: None,
                repo: None,
                folder: Some("data/apps/habit-tracker"),
                coding_agent: Some(crate::runtime::CodingAgent::Codex),
                model: None,
                reasoning_effort: None,
            },
        );
        assert!(
            body.get("cc_model").is_none() && body.get("reasoning_effort").is_none(),
            "an unpinned spawn must omit both, so the receiver applies its own \
             default rather than being handed an empty string: {body}"
        );
        assert_eq!(body["folder"], "data/apps/habit-tracker");
        assert_eq!(body["coding_agent"], "codex");
        assert!(
            body.get("repo_id").is_none(),
            "folder body must not also carry repo_id: {:?}",
            body
        );
    }

    #[test]
    fn build_coding_agent_body_omits_title_and_repo_when_unset_or_empty() {
        let tid = Uuid::new_v4();
        let body = build_cross_workspace_coding_agent_body(
            tid,
            &CrossWorkspaceSpawn {
                prompt: "x",
                title: None,
                repo: None,
                folder: None,
                coding_agent: None,
                model: None,
                reasoning_effort: None,
            },
        );
        assert!(
            body.get("title").is_none(),
            "title must be omitted: {:?}",
            body
        );
        assert!(
            body.get("repo_id").is_none(),
            "repo_id must be omitted: {:?}",
            body
        );
        assert!(
            body.get("coding_agent").is_none(),
            "coding_agent must be omitted when unset: {:?}",
            body
        );
        assert!(
            body.get("folder").is_none(),
            "folder must be omitted when unset: {:?}",
            body
        );
        // Empty strings are treated as absent so the receiver doesn't see "".
        let body = build_cross_workspace_coding_agent_body(
            tid,
            &CrossWorkspaceSpawn {
                prompt: "x",
                title: Some(""),
                repo: Some(""),
                folder: Some(""),
                coding_agent: None,
                model: None,
                reasoning_effort: None,
            },
        );
        assert!(
            body.get("title").is_none(),
            "empty title must be omitted: {:?}",
            body
        );
        assert!(
            body.get("repo_id").is_none(),
            "empty repo must be omitted: {:?}",
            body
        );
        assert!(
            body.get("folder").is_none(),
            "empty folder must be omitted: {:?}",
            body
        );
    }

    #[tokio::test]
    async fn spawn_coding_agent_in_workspace_posts_body_with_caller_context() {
        // End-to-end shape check: the helper must POST to <port>/api/v1/chat/stream
        // with use_coding_agent=true and the caller_* + mode fields merged in.
        // Without these, the receiving engine rejects Agent-mode messages
        // with 400 (no provenance).
        use std::sync::{Arc, Mutex};
        let captured: Arc<Mutex<Option<(String, Value)>>> = Arc::new(Mutex::new(None));
        let captured_for_handler = captured.clone();

        let app = axum::Router::new().route(
            "/api/v1/chat/stream",
            axum::routing::post(move |body: axum::Json<Value>| {
                let captured = captured_for_handler.clone();
                async move {
                    *captured.lock().unwrap() = Some(("/api/v1/chat/stream".into(), body.0));
                    axum::http::StatusCode::OK
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let target = CrossWorkspaceTarget {
            workspace_path: std::path::PathBuf::from("/tmp/fake-ws"),
            api_port: port,
            proto: "http".to_string(),
        };
        let ctx = WorkspaceCallCtx {
            self_workspace: "dev".into(),
            source_thread_id: Some(Uuid::nil()),
            source_event_id: Some(Uuid::nil()),
            mode: ActorMode::Agent,
        };
        let client = cross_workspace_http_client();
        let cc_thread_id = spawn_coding_agent_in_workspace(
            client,
            &target,
            &CrossWorkspaceSpawn {
                prompt: "build the feature",
                title: Some("Add feature X"),
                repo: None,
                folder: Some("data/apps/habit-tracker"),
                coding_agent: Some(crate::runtime::CodingAgent::Codex),
                model: None,
                reasoning_effort: None,
            },
            &ctx,
            "http",
        )
        .await
        .expect("cross-workspace spawn must succeed");

        let (path, body) = captured
            .lock()
            .unwrap()
            .clone()
            .expect("receiver must have captured a request");
        assert_eq!(path, "/api/v1/chat/stream");
        assert_eq!(body["message"], "build the feature");
        assert_eq!(body["thread_id"], cc_thread_id.to_string());
        assert_eq!(body["use_coding_agent"], true);
        assert_eq!(body["coding_agent"], "codex");
        assert_eq!(body["title"], "Add feature X");
        assert_eq!(body["folder"], "data/apps/habit-tracker");
        assert!(body.get("repo_id").is_none());
        // Cross-workspace provenance: receiver MUST see caller_workspace +
        // mode=agent or it rejects the request as malformed.
        assert_eq!(body["caller_workspace"], "dev");
        assert_eq!(body["mode"], "agent");
    }

    /// The spawn must assert WHICH workspace it is for, taken from the resolved
    /// target's own path. Without it, a stale or mis-resolved port lands the
    /// spawn on a different workspace's engine and looks like a success:
    /// `api::target_workspace` can only refuse an assertion that was made.
    #[tokio::test]
    async fn spawn_coding_agent_in_workspace_asserts_the_target_workspace() {
        use std::sync::{Arc, Mutex};
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let captured_for_handler = captured.clone();

        let app = axum::Router::new().route(
            "/api/v1/chat/stream",
            axum::routing::post(move |headers: axum::http::HeaderMap| {
                let captured = captured_for_handler.clone();
                async move {
                    *captured.lock().unwrap() = headers
                        .get(crate::api::actor::HEADER_TARGET_WORKSPACE)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    axum::http::StatusCode::OK
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let target = CrossWorkspaceTarget {
            // The assertion comes off this path's basename, never off anything
            // the caller typed, so it always names the workspace whose ports
            // file supplied the port beside it.
            workspace_path: std::path::PathBuf::from("/tmp/workspaces/myws"),
            api_port: port,
            proto: "http".to_string(),
        };
        let ctx = WorkspaceCallCtx {
            self_workspace: "dev".into(),
            source_thread_id: None,
            source_event_id: None,
            mode: ActorMode::Agent,
        };
        spawn_coding_agent_in_workspace(
            cross_workspace_http_client(),
            &target,
            &CrossWorkspaceSpawn {
                prompt: "build the feature",
                title: None,
                repo: None,
                folder: None,
                coding_agent: None,
                model: None,
                reasoning_effort: None,
            },
            &ctx,
            "http",
        )
        .await
        .expect("cross-workspace spawn must succeed");

        assert_eq!(
            captured.lock().unwrap().clone().as_deref(),
            Some("myws"),
            "the target workspace assertion must name the TARGET, not the caller"
        );
    }

    #[tokio::test]
    async fn spawn_coding_agent_in_workspace_propagates_http_error_with_status_and_body() {
        // 4xx/5xx from the receiver must surface as Err so the LLM sees the
        // failure instead of the helper falsely reporting success.
        let app = axum::Router::new().route(
            "/api/v1/chat/stream",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    "Thread is locked to Lucidos mode".to_string(),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let target = CrossWorkspaceTarget {
            workspace_path: std::path::PathBuf::from("/tmp/fake"),
            api_port: port,
            proto: "http".to_string(),
        };
        let client = cross_workspace_http_client();
        let err = spawn_coding_agent_in_workspace(
            client,
            &target,
            &CrossWorkspaceSpawn {
                prompt: "x",
                title: None,
                repo: None,
                folder: None,
                coding_agent: None,
                model: None,
                reasoning_effort: None,
            },
            &ctx(),
            "http",
        )
        .await
        .expect_err("4xx must surface as Err");
        assert!(err.contains("400"), "error must include status: {}", err);
        assert!(
            err.contains("locked to Lucidos mode"),
            "error must include the receiver's body so the LLM can act on it: {}",
            err
        );
    }

    /// Regression guard for `harden-engine-workspace-client-no-timeout`.
    /// Before the fix, `cross_workspace_http_client` built its reqwest client
    /// without a timeout; a peer workspace that accepted the TCP connection
    /// and never wrote a response would pin the calling thread until the
    /// surrounding turn was canceled.
    ///
    /// We exercise `build_cross_workspace_client` directly with a short
    /// timeout against a `TcpListener` that accepts and holds the connection
    /// without responding — the production path uses the same builder, just
    /// with `CROSS_WORKSPACE_TIMEOUT`. The assertion: the request errors out
    /// within twice the configured timeout and the error reports a timeout.
    #[tokio::test]
    async fn build_cross_workspace_client_times_out_against_hung_peer() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Accept connections but hold the stream so the OS doesn't see a
        // graceful close (which would surface as `Connection reset` /
        // `IncompleteMessage` instead of a timeout).
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });

        let timeout = Duration::from_millis(200);
        let client = build_cross_workspace_client(timeout).expect("client builds");
        let url = format!("http://{}/api/v1/chat/stream", addr);
        let body = serde_json::json!({"message": "hi"});
        let start = std::time::Instant::now();
        let result = workspace_post(&client, &url, &body, &ctx(), "target-ws").await;
        let elapsed = start.elapsed();

        let err = result.expect_err("hung peer must surface as Err");
        // The boxed error's outer Display just says "error sending request for
        // url (…)"; the timeout indicator lives on the wrapped reqwest::Error
        // (via source() chain). Walk the chain so we catch both formats.
        let mut chain: Vec<String> = Vec::new();
        let mut current: Option<&dyn std::error::Error> = Some(err.as_ref());
        while let Some(e) = current {
            chain.push(format!("{}", e));
            current = e.source();
        }
        let joined = chain.join(" -> ").to_lowercase();
        assert!(
            joined.contains("timed out") || joined.contains("timeout"),
            "expected timeout error, got chain: {}",
            chain.join(" -> ")
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "timeout fired but took {:?} — should be near {:?}",
            elapsed,
            timeout
        );
    }

    #[test]
    fn cross_workspace_http_client_returns_static_client_with_production_timeout() {
        // Smoke test: the LazyLock initializer doesn't panic.
        let _ = cross_workspace_http_client();
    }
}
