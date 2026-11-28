use crate::engine::http::workspace_resolver::CrossWorkspaceTarget;
use crate::engine::thread_events::{ActorMode, SpawnVerdict};
use reqwest::Client;
use serde_json::Value;
use std::sync::LazyLock;
use std::time::Duration;
use uuid::Uuid;

/// INVARIANT: Per-request timeout for cross-workspace POSTs. A peer engine that accepts
/// the connection and never responds would otherwise pin the calling thread
/// until the surrounding turn is canceled. Generous enough to cover a peer
/// under load setting up a `/api/v1/chat/stream` response, but bounded.
const CROSS_WORKSPACE_TIMEOUT: Duration =
    Duration::from_secs(crate::core::event_hooks::TIMEOUT_CEILING_SECS + 10);

pub(crate) fn build_cross_workspace_client(timeout: Duration) -> reqwest::Result<Client> {
    lucidos_local_token::client::client_builder()
        .timeout(timeout)
        .build()
}

pub fn cross_workspace_http_client() -> &'static Client {
    static CLIENT: LazyLock<Client> = LazyLock::new(|| {
        build_cross_workspace_client(CROSS_WORKSPACE_TIMEOUT)
            .expect("cross-workspace HTTP client builder")
    });
    &CLIENT
}

#[derive(Debug, Clone)]
pub struct WorkspaceCallCtx {
    pub self_workspace: String,
    pub source_thread_id: Option<Uuid>,
    pub source_event_id: Option<Uuid>,
    pub mode: ActorMode,
}

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
    obj.insert("mode".into(), Value::String(ctx.mode.as_str().into()));
    body
}

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
        .header(
            crate::core::subprocess_origin::HEADER_TARGET_WORKSPACE,
            target_workspace,
        )
        .json(&merged)
        .send()
        .await?)
}

#[derive(Debug)]
pub(crate) struct GatewayReply {
    pub(crate) status: reqwest::StatusCode,
    pub(crate) content_type: Option<reqwest::header::HeaderValue>,
    pub(crate) body: bytes::Bytes,
}

#[derive(Debug)]
pub(crate) enum ThroughGateway {
    Replied(GatewayReply),
    Unavailable { reason: &'static str },
}

pub(crate) async fn post_through_gateway(
    gateway_port: Option<&str>,
    target_workspace: &str,
    body: Value,
    ctx: &WorkspaceCallCtx,
    verdict: &SpawnVerdict,
) -> Result<ThroughGateway, String> {
    let unavailable = ThroughGateway::Unavailable {
        reason: lucidos_gateway_stamp::REASON_NO_GATEWAY,
    };
    let (Some(port), Some(path)) = (
        gateway_port,
        lucidos_gateway_stamp::workspace_call_path(target_workspace),
    ) else {
        return Ok(unavailable);
    };
    let Some(signed) = crate::api::gateway_stamp::sign_workspace_call(
        target_workspace,
        ctx.source_thread_id,
        verdict,
    ) else {
        return Ok(unavailable);
    };
    let url = format!(
        "{}/~/api/v1{path}",
        lucidos_local_token::client::origin(port)
    );
    let client = crate::gateway_auth::client_builder()
        .timeout(CROSS_WORKSPACE_TIMEOUT)
        .build()
        .map_err(|e| format!("could not build the gateway client: {e}"))?;
    post_signed_call(&client, &url, target_workspace, body, ctx, signed).await
}

async fn post_signed_call(
    client: &Client,
    url: &str,
    target_workspace: &str,
    body: Value,
    ctx: &WorkspaceCallCtx,
    signed: crate::api::gateway_stamp::SignedWorkspaceCall,
) -> Result<ThroughGateway, String> {
    let ctx = WorkspaceCallCtx {
        self_workspace: signed.caller_workspace,
        ..ctx.clone()
    };
    let body = merge_caller_fields(body, &ctx);
    let mut request = client
        .post(url)
        .header(
            crate::core::subprocess_origin::HEADER_TARGET_WORKSPACE,
            target_workspace,
        )
        .json(&body);
    for (name, value) in &signed.headers {
        request = request.header(*name, value);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("cross-workspace call through {url} failed: {e}"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .cloned();
    let body = response
        .bytes()
        .await
        .map_err(|e| format!("reading the answer from {url} failed: {e}"))?;
    let elsewhere = status == reqwest::StatusCode::NOT_FOUND
        && serde_json::from_slice::<Value>(&body).is_ok_and(|answer| {
            answer["reason"] == lucidos_gateway_stamp::REASON_WORKSPACE_NOT_ON_THIS_GATEWAY
        });
    if elsewhere {
        return Ok(ThroughGateway::Unavailable {
            reason: lucidos_gateway_stamp::REASON_WORKSPACE_NOT_ON_THIS_GATEWAY,
        });
    }
    Ok(ThroughGateway::Replied(GatewayReply {
        status,
        content_type,
        body,
    }))
}

pub struct CrossWorkspaceSpawn<'a> {
    pub hook_args: &'a crate::core::event_hooks::HookArgs,
    pub prompt: &'a str,
    pub title: Option<&'a str>,
    pub folder: Option<&'a str>,
    pub coding_agent: Option<crate::runtime::CodingAgent>,
    pub model: Option<&'a str>,
    pub reasoning_effort: Option<&'a str>,
}

/// INVARIANT: Build the `/api/v1/chat/stream` body for a cross-workspace coding-agent spawn.
///
/// Pre-generates `thread_id` (caller-supplied) so the caller can return a
/// workspace-qualified link without a second request. Mirrors the body shape of
/// `lucidos spawn-thread --coding-agent <backend>` (see
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
    for (key, value) in [
        ("title", spawn.title),
        ("folder", spawn.folder),
        ("cc_model", spawn.model),
        ("reasoning_effort", spawn.reasoning_effort),
    ] {
        if let Some(v) = value.filter(|s| !s.is_empty()) {
            obj.insert(key.into(), Value::String(v.to_string()));
        }
    }
    if !spawn.hook_args.is_empty() {
        obj.insert("hook_args".into(), serde_json::json!(spawn.hook_args));
    }
    if let Some(agent) = spawn.coding_agent {
        obj.insert(
            "coding_agent".into(),
            Value::String(agent.as_str().to_string()),
        );
    }
    body
}

/// INVARIANT: Spawn a coding-agent thread in another workspace.
///
/// POSTs to the target workspace's `/api/v1/chat/stream` with `use_coding_agent:
/// true` and the caller_* / mode fields merged in, through the gateway with
/// `verdict` when it can ([`post_through_gateway`]), else via `workspace_post`.
/// Returns the pre-generated `thread_id` on success so the caller can build
/// the user-facing link.
///
/// `gateway_port` names this engine's gateway, as for [`post_through_gateway`].
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
    gateway_port: Option<&str>,
    verdict: &SpawnVerdict,
) -> Result<Uuid, String> {
    let cc_thread_id = Uuid::new_v4();
    let body = build_cross_workspace_coding_agent_body(cc_thread_id, spawn);
    let target_workspace = target
        .workspace_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match post_through_gateway(gateway_port, &target_workspace, body.clone(), ctx, verdict).await? {
        ThroughGateway::Replied(reply) if reply.status.is_success() => return Ok(cc_thread_id),
        ThroughGateway::Replied(reply) => {
            let text = String::from_utf8_lossy(&reply.body);
            return Err(needs_input_result(&text).unwrap_or_else(|| {
                format!(
                    "cross-workspace call to {} through the gateway returned HTTP {}: {}",
                    target_workspace, reply.status, text
                )
            }));
        }
        ThroughGateway::Unavailable { reason } => crate::log_legacy!(
            "[WorkspaceClient] No gateway carries the call to {} ({}): posting directly, \
             where it lands unattested",
            target_workspace,
            reason
        ),
    }
    let url = format!(
        "{}/api/v1/chat/stream",
        lucidos_local_token::client::engine_origin(scheme, target.api_port)
    );
    let resp = workspace_post(client, &url, &body, ctx, &target_workspace)
        .await
        .map_err(|e| {
            format!(
                "cross-workspace POST to {} failed: {}{}",
                url,
                e,
                target.assumed_proto_note()
            )
        })?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(needs_input_result(&text).unwrap_or_else(|| {
            format!(
                "cross-workspace POST to {} returned HTTP {}: {}",
                url, status, text
            )
        }));
    }
    Ok(cc_thread_id)
}

fn needs_input_result(text: &str) -> Option<String> {
    let body: Value = serde_json::from_str(text).ok()?;
    (body["reason"] == "hook_needs_input").then(|| {
        serde_json::json!({
            "status": "needs-input",
            "hook_input": body["hook_input"],
        })
        .to_string()
    })
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
                hook_args: &Default::default(),
                prompt: "do the thing",
                title: Some("My title"),
                folder: None,
                coding_agent: Some(crate::runtime::CodingAgent::Codex),
                model: Some("gpt-5.6-luna"),
                reasoning_effort: Some("low"),
            },
        );
        assert_eq!(body["message"], "do the thing");
        assert_eq!(body["cc_model"], "gpt-5.6-luna");
        assert_eq!(body["reasoning_effort"], "low");
        assert!(
            body.get("model").is_none(),
            "a coding-agent spawn must not set the chat model key: {body}"
        );
        assert_eq!(body["thread_id"], tid.to_string());
        assert_eq!(body["use_coding_agent"], true);
        assert_eq!(body["title"], "My title");
        assert_eq!(body["coding_agent"], "codex");
    }

    #[test]
    fn build_coding_agent_body_includes_folder_when_set() {
        let tid = Uuid::new_v4();
        let body = build_cross_workspace_coding_agent_body(
            tid,
            &CrossWorkspaceSpawn {
                hook_args: &Default::default(),
                prompt: "do the thing",
                title: None,
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
    fn build_coding_agent_body_omits_optional_fields_when_unset_or_empty() {
        let tid = Uuid::new_v4();
        let body = build_cross_workspace_coding_agent_body(
            tid,
            &CrossWorkspaceSpawn {
                hook_args: &Default::default(),
                prompt: "x",
                title: None,
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
        let body = build_cross_workspace_coding_agent_body(
            tid,
            &CrossWorkspaceSpawn {
                hook_args: &Default::default(),
                prompt: "x",
                title: Some(""),
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
            "empty fields must not produce repo_id: {:?}",
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
            proto_assumed: false,
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
                hook_args: &Default::default(),
                prompt: "build the feature",
                title: Some("Add feature X"),
                folder: Some("data/apps/habit-tracker"),
                coding_agent: Some(crate::runtime::CodingAgent::Codex),
                model: None,
                reasoning_effort: None,
            },
            &ctx,
            "http",
            None,
            &SpawnVerdict::Allowed,
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
        assert_eq!(body["caller_workspace"], "dev");
        assert_eq!(body["mode"], "agent");
    }

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
                        .get(crate::core::subprocess_origin::HEADER_TARGET_WORKSPACE)
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
            workspace_path: std::path::PathBuf::from("/tmp/workspaces/myws"),
            api_port: port,
            proto: "http".to_string(),
            proto_assumed: false,
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
                hook_args: &Default::default(),
                prompt: "build the feature",
                title: None,
                folder: None,
                coding_agent: None,
                model: None,
                reasoning_effort: None,
            },
            &ctx,
            "http",
            None,
            &SpawnVerdict::Allowed,
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
    async fn spawn_coding_agent_in_workspace_reaches_the_engine_past_an_ipv6_impostor() {
        let engine = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = engine.local_addr().unwrap().port();
        let Ok(impostor) = tokio::net::TcpListener::bind(("::1", port)).await else {
            crate::log!(
                "[WorkspaceClient] Test skipped: this host cannot bind [::1]:{}",
                port
            );
            return;
        };
        let answer = |status| axum::Router::new().fallback(move || async move { status });
        let engine_app = answer(axum::http::StatusCode::OK);
        let impostor_app = answer(axum::http::StatusCode::NOT_FOUND);
        tokio::spawn(async move { axum::serve(engine, engine_app).await.unwrap() });
        tokio::spawn(async move { axum::serve(impostor, impostor_app).await.unwrap() });

        let target = CrossWorkspaceTarget {
            workspace_path: std::path::PathBuf::from("/tmp/workspaces/myws"),
            api_port: port,
            proto: "http".to_string(),
            proto_assumed: false,
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
                hook_args: &Default::default(),
                prompt: "build the feature",
                title: None,
                folder: None,
                coding_agent: None,
                model: None,
                reasoning_effort: None,
            },
            &ctx,
            "http",
            None,
            &SpawnVerdict::Allowed,
        )
        .await
        .expect("the spawn reaches the engine on 127.0.0.1");
    }

    #[tokio::test]
    async fn spawn_coding_agent_in_workspace_propagates_http_error_with_status_and_body() {
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
            proto_assumed: false,
        };
        let client = cross_workspace_http_client();
        let err = spawn_coding_agent_in_workspace(
            client,
            &target,
            &CrossWorkspaceSpawn {
                hook_args: &Default::default(),
                prompt: "x",
                title: None,
                folder: None,
                coding_agent: None,
                model: None,
                reasoning_effort: None,
            },
            &ctx(),
            "http",
            None,
            &SpawnVerdict::Allowed,
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

    #[tokio::test]
    async fn build_cross_workspace_client_times_out_against_hung_peer() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
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

    type Seen = std::sync::Arc<std::sync::Mutex<Option<(axum::http::HeaderMap, Value)>>>;

    async fn stand_in_gateway(status: u16, answer: Value) -> (String, Seen) {
        let seen: Seen = Default::default();
        let record = seen.clone();
        let app = axum::Router::new().fallback(
            move |headers: axum::http::HeaderMap, body: axum::Json<Value>| {
                let record = record.clone();
                let answer = answer.clone();
                async move {
                    *record.lock().unwrap() = Some((headers, body.0));
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        axum::Json(answer),
                    )
                }
            },
        );
        let addr = lucidos_local_token::test_support::serve_https(app).addr;
        (format!("https://{addr}"), seen)
    }

    fn signed(thread: Uuid) -> crate::api::gateway_stamp::SignedWorkspaceCall {
        let _ = crate::api::gateway_stamp::test_support::stamp_for("d1");
        crate::api::gateway_stamp::sign_workspace_call(
            "work2",
            Some(thread),
            &SpawnVerdict::Allowed,
        )
        .expect("the test stamp key is installed")
    }

    #[tokio::test]
    async fn a_signed_call_carries_its_claim_and_the_signing_id() {
        let (base, seen) = stand_in_gateway(200, serde_json::json!({"ok": true})).await;
        let thread = Uuid::new_v4();
        let ctx = WorkspaceCallCtx {
            self_workspace: "dir-name".into(),
            source_thread_id: Some(thread),
            source_event_id: None,
            mode: ActorMode::Agent,
        };
        let outcome = post_signed_call(
            cross_workspace_http_client(),
            &format!("{base}/x"),
            "work2",
            serde_json::json!({"message": "hi"}),
            &ctx,
            signed(thread),
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, ThroughGateway::Replied(ref r) if r.status.is_success()),
            "{outcome:?}"
        );
        let (headers, body) = seen.lock().unwrap().clone().unwrap();
        assert_eq!(body["caller_workspace"], "stamp-test-workspace");
        assert_eq!(body["caller_thread_id"], thread.to_string());
        assert_eq!(body["message"], "hi");
        for name in [
            lucidos_gateway_stamp::HEADER_CALLER_WORKSPACE,
            lucidos_gateway_stamp::HEADER_CALLER_THREAD,
            lucidos_gateway_stamp::HEADER_CALLER_VERDICT,
            lucidos_gateway_stamp::HEADER_GATEWAY_STAMP,
        ] {
            assert!(headers.contains_key(name), "{name} missing");
        }
        assert_eq!(
            headers[crate::core::subprocess_origin::HEADER_TARGET_WORKSPACE],
            "work2"
        );
    }

    #[tokio::test]
    async fn only_a_target_elsewhere_hands_the_call_back() {
        let thread = Uuid::new_v4();
        let ctx = ctx();
        let elsewhere = serde_json::json!({
            "reason": lucidos_gateway_stamp::REASON_WORKSPACE_NOT_ON_THIS_GATEWAY
        });
        let (base, _) = stand_in_gateway(404, elsewhere).await;
        let outcome = post_signed_call(
            cross_workspace_http_client(),
            &base,
            "work2",
            serde_json::json!({}),
            &ctx,
            signed(thread),
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome,
            ThroughGateway::Unavailable {
                reason: lucidos_gateway_stamp::REASON_WORKSPACE_NOT_ON_THIS_GATEWAY
            }
        ));
        let (base, _) = stand_in_gateway(403, serde_json::json!({"error": "no"})).await;
        let outcome = post_signed_call(
            cross_workspace_http_client(),
            &base,
            "work2",
            serde_json::json!({}),
            &ctx,
            signed(thread),
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, ThroughGateway::Replied(ref r) if r.status == 403),
            "{outcome:?}"
        );
    }

    #[tokio::test]
    async fn with_no_gateway_the_call_is_handed_back() {
        let outcome = post_through_gateway(
            None,
            "work2",
            serde_json::json!({}),
            &ctx(),
            &SpawnVerdict::Allowed,
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome,
            ThroughGateway::Unavailable {
                reason: lucidos_gateway_stamp::REASON_NO_GATEWAY
            }
        ));
    }

    #[test]
    fn cross_workspace_http_client_returns_static_client_with_production_timeout() {
        let _ = cross_workspace_http_client();
    }
}
