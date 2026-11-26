use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{routing::post, Json, Router};
use lucidos_gateway_stamp::workspace_operation::{
    receive_route, OperationKind, RoutingRequest, SignedOperation, WorkspaceOperation,
};
use tower::ServiceExt;

use super::{actor, error::ApiError, AppState};
use crate::engine::thread_events::{ActorMode, RemoteStartAsk};
use lucidos_gateway_stamp::remote_start::{RemoteStart, RemoteStartSetting, HEADER_REMOTE_START};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RoutedOrigin {
    pub(super) caller_gateway: String,
    pub(super) routing_gateway: String,
    pub(super) remote_start: Option<Box<RemoteStartAsk>>,
}

tokio::task_local! {
    static ROUTED_ORIGIN: RoutedOrigin;
}

pub(super) fn routed_origin() -> Option<RoutedOrigin> {
    ROUTED_ORIGIN.try_with(Clone::clone).ok()
}

fn awaited_pairing(start: Option<RemoteStart>) -> Option<Box<RemoteStartAsk>> {
    let start = start?;
    let pairing = start.pairing?;
    (start.setting == RemoteStartSetting::Ask).then(|| {
        Box::new(RemoteStartAsk {
            pairing_id: pairing.id,
            pairing_label: pairing.label,
        })
    })
}

const ACK_REMOTE_START: &str = "remote_start";

fn validate_spawn_body(body: &serde_json::Map<String, serde_json::Value>) -> Result<(), ApiError> {
    const FIELDS: &[&str] = &[
        "message",
        "mode",
        "thread_id",
        "new_thread",
        "title",
        "images",
        "use_coding_agent",
        "coding_agent",
        "cc_model",
        "model",
        "reasoning_effort",
        "repo_id",
        "folder",
        "integrated",
        "caller_workspace",
        "caller_thread_id",
        "caller_event_id",
    ];
    if let Some(field) = body.keys().find(|key| !FIELDS.contains(&key.as_str())) {
        return Err(ApiError::bad_request(format!(
            "Routed spawns do not support {field}. They create new agent-attributed threads."
        )));
    }
    if body
        .get("thread_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
        .is_none()
    {
        return Err(ApiError::bad_request(
            "A routed spawn needs its new thread's preallocated UUID.",
        ));
    }
    if body
        .get("mode")
        .is_some_and(|mode| !matches!(mode.as_str(), Some("agent" | "engine")))
        || body.get("integrated").is_some_and(|value| value != false)
        || body.get("new_thread").is_some_and(|value| value != true)
    {
        return Err(ApiError::bad_request(
            "Routed spawns create new agent-attributed top-level threads.",
        ));
    }
    Ok(())
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/workspace-operations", post(outbound))
        .route("/workspace-operations/receive", post(receive_read))
        .route("/workspace-operations/receive-start", post(receive_start))
        .layer(axum::middleware::map_response(
            |mut response: Response| async move {
                response.headers_mut().insert(
                    lucidos_gateway_stamp::workspace_operation::PROTOCOL_HEADER,
                    lucidos_gateway_stamp::workspace_operation::PROTOCOL_VERSION
                        .parse()
                        .expect("literal header value"),
                );
                response
            },
        ))
}

async fn outbound(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<RoutingRequest>,
) -> Result<Response, ApiError> {
    let crate::core::subprocess_origin::SubprocessOrigin::Subprocess {
        source_thread_id: Some(thread),
        ..
    } = crate::core::subprocess_origin::subprocess_origin(&headers)
    else {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Routing requires this engine's thread-bound origin token.",
        ));
    };
    let verdict = if let WorkspaceOperation::Spawn { body } = &mut request.operation {
        validate_spawn_body(body)?;
        let origin =
            actor::build_message_origin(&headers, ActorMode::Agent, None, None, None, None, None)
                .ok_or_else(|| ApiError::internal("The source origin is unavailable."))?;
        let verdict = crate::engine::agent_session::spawn_gate::spawn_verdict(
            &state.pool,
            &state.engine.trigger_configs,
            origin,
        )
        .await;
        if let crate::engine::thread_events::SpawnVerdict::Refused { reason } = &verdict {
            return Err(ApiError::new(StatusCode::FORBIDDEN, reason));
        }
        body.insert("mode".into(), "agent".into());
        verdict.to_header()
    } else {
        String::new()
    };
    let signed =
        super::gateway_stamp::sign_operation(request, thread, verdict).ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "This engine has no gateway signing key, or the operation is invalid.",
            )
        })?;
    let port = crate::core::base_path::gateway_port().ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "This workspace has no registered gateway.",
        )
    })?;
    let client = crate::gateway_auth::client_builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let url = format!(
        "{}/~/api/v1/workspace-operations",
        lucidos_local_token::client::origin(&port)
    );
    let mut response = match client.post(&url).json(&signed).send().await {
        Ok(response) => response,
        Err(error) if error.is_connect() => {
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "This workspace's gateway could not be reached.",
            ))
        }
        Err(_) => return Err(ApiError::new(StatusCode::BAD_GATEWAY,
            "The gateway operation did not finish. Its outcome may be uncertain; it was not retried.")),
    };
    let status = response.status();
    if response
        .headers()
        .get(lucidos_gateway_stamp::workspace_operation::PROTOCOL_HEADER)
        .is_none_or(|version| {
            version != lucidos_gateway_stamp::workspace_operation::PROTOCOL_VERSION
        })
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "Upgrade this workspace's gateway to use routed workspace operations.",
        ));
    }
    let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "The operation reply could not be read; the operation was not retried.",
        )
    })? {
        if bytes.len() + chunk.len() > 32 * 1024 * 1024 {
            return Err(ApiError::new(StatusCode::BAD_GATEWAY,
                "The operation reply exceeds 32 MiB. Query events with pagination for a large thread."));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|_| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "The operation returned invalid JSON; the operation was not retried.",
        )
    })?;
    let mut response = (status, bytes).into_response();
    if let Some(content_type) = content_type {
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    Ok(response)
}

const SPAWN_LOCK_CLASS: i32 = 0x7273_706e;
static SPAWN_ADMISSION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(super) struct SpawnReservation<'a> {
    _transaction: sqlx::Transaction<'a, sqlx::Postgres>,
    _admission: tokio::sync::MutexGuard<'static, ()>,
}

pub(super) async fn has_spawn_record(
    pool: &sqlx::PgPool,
    thread: uuid::Uuid,
) -> Result<bool, ApiError> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM events WHERE event_type = 'ThreadQueued' AND payload->'data'->>'thread_id' = $1)",
    ).bind(thread.to_string()).fetch_one(pool).await.map_err(|e| ApiError::internal(e.to_string()))
}

pub(super) async fn reserve_spawn(
    pool: &sqlx::PgPool,
    thread: uuid::Uuid,
) -> Result<SpawnReservation<'_>, ApiError> {
    let admission = SPAWN_ADMISSION.lock().await;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(SPAWN_LOCK_CLASS)
        .bind(thread.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM thread_summaries WHERE thread_id = $1) OR EXISTS(SELECT 1 FROM events WHERE thread_id = $1) OR EXISTS(SELECT 1 FROM events WHERE event_type = 'ThreadQueued' AND payload->'data'->>'thread_id' = $2)",
    ).bind(thread).bind(thread.to_string()).fetch_one(&mut *tx).await.map_err(|e| ApiError::internal(e.to_string()))?;
    if exists {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "A routed spawn can only start a new thread.",
        ));
    }
    Ok(SpawnReservation {
        _admission: admission,
        _transaction: tx,
    })
}

async fn receive_read(
    state: State<AppState>,
    headers: HeaderMap,
    Json(signed): Json<SignedOperation>,
) -> Result<Response, ApiError> {
    require_route(OperationKind::Read, &signed)?;
    receive(state, headers, signed).await
}

async fn receive_start(
    state: State<AppState>,
    headers: HeaderMap,
    Json(signed): Json<SignedOperation>,
) -> Result<Response, ApiError> {
    require_route(OperationKind::Spawn, &signed)?;
    receive(state, headers, signed).await
}

fn require_route(served: OperationKind, signed: &SignedOperation) -> Result<(), ApiError> {
    let kind = signed.envelope.request.operation.kind();
    if kind == served {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "This engine receives this operation only on /api/v1{}. The sending gateway \
             runs a different Lucidos build: restart Lucidos on that machine, then retry.",
            receive_route(kind)
        )))
    }
}

async fn receive(
    State(state): State<AppState>,
    request_headers: HeaderMap,
    signed: SignedOperation,
) -> Result<Response, ApiError> {
    let remote_start = request_headers
        .get(HEADER_REMOTE_START)
        .and_then(|value| value.to_str().ok());
    let (headers, start) = super::gateway_stamp::verify_operation(&signed, remote_start)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::FORBIDDEN,
                "The operation stamp or its remote start setting does not verify for this \
                 workspace.",
            )
        })?;
    let awaited = awaited_pairing(start);
    let envelope = signed.envelope;
    let spawned_thread = match &envelope.request.operation {
        WorkspaceOperation::Spawn { body } => {
            validate_spawn_body(body)?;
            body.get("thread_id").cloned()
        }
        WorkspaceOperation::Read { .. } => None,
    };
    let route = RoutedOrigin {
        caller_gateway: envelope.caller_gateway.clone(),
        routing_gateway: envelope
            .request
            .via
            .clone()
            .expect("verified routing gateway"),
        remote_start: awaited.clone(),
    };
    let (uri, method, body) = match envelope.request.operation {
        WorkspaceOperation::Read { request } => {
            let path = request.path().map_err(ApiError::bad_request)?;
            let query = request.query_pairs().map_err(ApiError::bad_request)?;
            let mut url = reqwest::Url::parse("http://localhost").expect("literal origin");
            url.set_path(&path);
            url.query_pairs_mut().extend_pairs(query);
            (
                format!(
                    "{}{}",
                    url.path(),
                    url.query().map(|q| format!("?{q}")).unwrap_or_default()
                ),
                "GET",
                Body::empty(),
            )
        }
        WorkspaceOperation::Spawn { mut body } => {
            if let Some(crate::engine::thread_events::SpawnVerdict::Refused { reason }) =
                crate::engine::thread_events::SpawnVerdict::from_header(&envelope.verdict)
            {
                return Err(ApiError::new(StatusCode::FORBIDDEN, reason));
            }
            body.insert("new_thread".into(), true.into());
            body.insert("caller_workspace".into(), envelope.caller_workspace.into());
            body.insert("caller_thread_id".into(), envelope.caller_thread_id.into());
            body.insert("mode".into(), "agent".into());
            (
                "/api/v1/chat/stream".into(),
                "POST",
                Body::from(
                    serde_json::to_vec(&body).map_err(|e| ApiError::internal(e.to_string()))?,
                ),
            )
        }
    };
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    request.headers_mut().extend(headers);
    let app = Router::new()
        .nest(
            "/api/v1",
            Router::new()
                .merge(super::threads::router())
                .merge(super::history::router())
                .merge(super::intelligence_ladder::router())
                .merge(super::chat::router()),
        )
        .with_state(state);
    let response = ROUTED_ORIGIN
        .scope(route, app.oneshot(request))
        .await
        .expect("router service is infallible");
    if let Some(thread) = spawned_thread.filter(|_| response.status().is_success()) {
        let (parts, body) = response.into_parts();
        let bytes = axum::body::to_bytes(body,1024*1024).await
            .map_err(|_|ApiError::internal("The spawn was accepted but its acknowledgment could not be read. Inspect the requested thread before retrying."))?;
        let mut ack:serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_|ApiError::internal("The spawn acknowledgment was invalid. Inspect the requested thread before retrying."))?;
        let fields = ack
            .as_object_mut()
            .ok_or_else(|| ApiError::internal("The spawn acknowledgment was not an object."))?;
        fields.insert("thread_id".into(), thread);
        fields.insert(
            ACK_REMOTE_START.into(),
            match &awaited {
                Some(pairing) => format!(
                    "Waiting for the owner of this workspace's gateway. It asks before \
                     agents on \"{}\" start threads, so the thread's first turn runs after \
                     its owner allows it.",
                    pairing.pairing_label
                ),
                None => "Started.".into(),
            }
            .into(),
        );
        let bytes = serde_json::to_vec(&ack).map_err(|e| ApiError::internal(e.to_string()))?;
        let mut response = Response::from_parts(parts, Body::from(bytes));
        response.headers_mut().remove(header::CONTENT_LENGTH);
        Ok(response)
    } else {
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucidos_gateway_stamp::workspace_operation::{RECEIVE_ROUTE, RECEIVE_START_ROUTE};

    mod guard_tests {
        use super::*;

        #[test]
        fn the_router_serves_each_receive_route_by_its_literal() {
            let scanned: Vec<_> = crate::api::route_scan::scan_api_routes()
                .into_iter()
                .filter(|hit| hit.file == "workspace_operations.rs")
                .map(|hit| hit.path)
                .collect();
            for route in [RECEIVE_ROUTE, RECEIVE_START_ROUTE] {
                assert!(
                    scanned.iter().any(|path| path == route),
                    "{route}: {scanned:?}"
                );
            }
        }
    }

    #[test]
    fn each_operation_kind_is_received_only_on_its_own_route() {
        use lucidos_gateway_stamp::workspace_operation::{
            OperationEnvelope, ReadKind, ReadRequest,
        };
        use lucidos_gateway_stamp::{Claim, StampKey};
        let key = StampKey::read_handover("ab".repeat(32).as_bytes()).unwrap();
        let signed = |operation| {
            let envelope = OperationEnvelope {
                request: RoutingRequest {
                    via: Some("routing-gateway".into()),
                    to: Some("stamp-test-workspace".into()),
                    operation,
                },
                caller_gateway: "source-gateway".into(),
                caller_workspace: "source".into(),
                caller_thread_id: uuid::Uuid::new_v4().to_string(),
                verdict: String::new(),
                nonce: uuid::Uuid::new_v4().to_string(),
            };
            let digest = envelope.digest().unwrap();
            let stamp = key.sign_now(Claim::WorkspaceOperation(envelope.claim(&digest)));
            SignedOperation { envelope, stamp }
        };
        let read = signed(WorkspaceOperation::Read {
            request: ReadRequest {
                kind: ReadKind::ThreadMessages,
                thread_id: Some(uuid::Uuid::new_v4().to_string()),
                query: Default::default(),
            },
        });
        let spawn = signed(WorkspaceOperation::Spawn {
            body: serde_json::json!({"message": "Start a task"})
                .as_object()
                .unwrap()
                .clone(),
        });
        assert!(require_route(OperationKind::Read, &read).is_ok());
        assert!(require_route(OperationKind::Spawn, &spawn).is_ok());
        for (served, signed, route) in [
            (OperationKind::Read, &spawn, RECEIVE_START_ROUTE),
            (OperationKind::Spawn, &read, RECEIVE_ROUTE),
        ] {
            let refused = require_route(served, signed).unwrap_err().into_response();
            assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
            let body = futures::executor::block_on(axum::body::to_bytes(refused.into_body(), 1024))
                .unwrap();
            assert!(
                String::from_utf8_lossy(&body).contains(route),
                "the refusal names the right route"
            );
        }
    }

    #[tokio::test]
    async fn the_real_source_handler_refuses_unbound_tokens_and_denied_spawn_roots() {
        use crate::core::subprocess_origin::{
            init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
        };
        use crate::engine::thread_events::MessageOrigin;
        use crate::test_support::{
            boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db,
        };
        if !in_engine_child("api::workspace_operations::tests::the_real_source_handler_refuses_unbound_tokens_and_denied_spawn_roots").await { return; }
        let (pool, db) = setup_test_db().await;
        let (engine, workspace) = boot_mock_engine(&db).await;
        let state = crate::api::permission_requests::tests::permission_test_state(
            engine.clone(),
            workspace.path().into(),
        )
        .await;
        init_agent_origin_secret("source-operation-test-secret".into());
        let operation = || RoutingRequest {
            via: Some("routing-gateway".into()),
            to: Some("target@cloud".into()),
            operation: WorkspaceOperation::Spawn {
                body:
                    serde_json::json!({"message":"Start a task","thread_id":uuid::Uuid::new_v4()})
                        .as_object()
                        .unwrap()
                        .clone(),
            },
        };
        for token in [
            None,
            Some("forged".into()),
            mint_agent_origin_token(None, 0, None),
        ] {
            let mut headers = HeaderMap::new();
            if let Some(token) = token {
                headers.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
            }
            let response = outbound(State(state.clone()), headers, Json(operation()))
                .await
                .unwrap_err()
                .into_response();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        for origin in [
            MessageOrigin::System,
            MessageOrigin::Webhook {
                webhook_id: "fixture-hook".into(),
                name: "Fixture hook".into(),
            },
            MessageOrigin::EventHook {
                slug: "fixture-gate".into(),
            },
        ] {
            let thread = uuid::Uuid::new_v4();
            crate::api::permission_requests::tests::open_turn(&engine.event_bus, thread, origin)
                .await;
            let before: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM events WHERE event_type='MessageReceived'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            let mut headers = HeaderMap::new();
            headers.insert(
                HEADER_AGENT_ORIGIN_TOKEN,
                mint_agent_origin_token(Some(thread), 0, None)
                    .unwrap()
                    .parse()
                    .unwrap(),
            );
            let response = outbound(State(state.clone()), headers, Json(operation()))
                .await
                .unwrap_err()
                .into_response();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "source refusal must precede signing or a network request"
            );
            let after: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM events WHERE event_type='MessageReceived'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                before, after,
                "a refused source spawn emits no target or source submission"
            );
        }
        drop(state);
        drop(engine);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn routed_spawns_reserve_the_new_thread_while_queued() {
        use crate::engine::thread_events::SpawnVerdict;
        use crate::test_support::{
            boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db,
        };
        use lucidos_gateway_stamp::workspace_operation::OperationEnvelope;
        use lucidos_gateway_stamp::{Claim, StampKey};
        use uuid::Uuid;

        if !in_engine_child(
            "api::workspace_operations::tests::routed_spawns_reserve_the_new_thread_while_queued",
        )
        .await
        {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let (engine, workspace) = boot_mock_engine(&db).await;
        let state = crate::api::permission_requests::tests::permission_test_state(
            engine.clone(),
            workspace.path().into(),
        )
        .await;
        let mut policy = engine.thread_queue.policy().await;
        policy.max_concurrent_total = 0;
        engine.thread_queue.set_policy(policy, None).await.unwrap();
        super::super::gateway_stamp::test_support::stamp_for("fixture-device");
        let key = StampKey::read_handover("ab".repeat(32).as_bytes()).unwrap();
        let allowed = |signed: &SignedOperation| {
            let mut headers = HeaderMap::new();
            headers.insert(
                HEADER_REMOTE_START,
                RemoteStart::allowed()
                    .header_value(&key, "stamp-test-workspace", &signed.envelope.nonce)
                    .parse()
                    .unwrap(),
            );
            headers
        };
        let thread = Uuid::new_v4();
        let caller = Uuid::new_v4();
        let signed_spawn = || {
            let envelope = OperationEnvelope {
                request: RoutingRequest {
                    via: Some("routing-gateway".into()),
                    to: Some("stamp-test-workspace".into()),
                    operation: WorkspaceOperation::Spawn {
                        body: serde_json::json!({
                            "message": "Start a queued task",
                            "thread_id": thread,
                            "mode": "agent",
                        })
                        .as_object()
                        .unwrap()
                        .clone(),
                    },
                },
                caller_gateway: "source-gateway".into(),
                caller_workspace: "source".into(),
                caller_thread_id: caller.to_string(),
                verdict: SpawnVerdict::Allowed.to_header(),
                nonce: Uuid::new_v4().to_string(),
            };
            let digest = envelope.digest().unwrap();
            let stamp = key.sign_now(Claim::WorkspaceOperation(envelope.claim(&digest)));
            SignedOperation { envelope, stamp }
        };
        let responses = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            futures::future::join_all((0..64).map(|_| {
                let signed = signed_spawn();
                receive_start(State(state.clone()), allowed(&signed), Json(signed))
            })),
        )
        .await
        .expect("duplicate spawns must leave pool capacity for submission");
        let mut statuses: Vec<_> = responses
            .into_iter()
            .map(|response| response.into_response().status())
            .collect();
        statuses.sort();
        assert_eq!(statuses[0], StatusCode::OK);
        assert!(statuses[1..]
            .iter()
            .all(|status| *status == StatusCode::FORBIDDEN));
        let signed = signed_spawn();
        let repeated = receive_start(State(state.clone()), allowed(&signed), Json(signed))
            .await
            .into_response();
        assert_eq!(repeated.status(), StatusCode::FORBIDDEN);
        let queued: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE event_type = 'ThreadQueued' \
             AND payload->'data'->>'thread_id' = $1",
        )
        .bind(thread.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(queued, 1, "a queued spawn reserves its new thread identity");
        let messages: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'MessageReceived'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(messages, 0, "blocked capacity must leave the spawn queued");
        drop(state);
        drop(engine);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_spawn_posted_to_the_read_route_is_refused_without_spending_its_nonce() {
        use crate::engine::thread_events::SpawnVerdict;
        use crate::test_support::{
            boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db,
        };
        use lucidos_gateway_stamp::workspace_operation::OperationEnvelope;
        use lucidos_gateway_stamp::{Claim, StampKey};
        use uuid::Uuid;

        if !in_engine_child(
            "api::workspace_operations::tests::a_spawn_posted_to_the_read_route_is_refused_without_spending_its_nonce",
        )
        .await
        {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let (engine, workspace) = boot_mock_engine(&db).await;
        let state = crate::api::permission_requests::tests::permission_test_state(
            engine.clone(),
            workspace.path().into(),
        )
        .await;
        let mut policy = engine.thread_queue.policy().await;
        policy.max_concurrent_total = 0;
        engine.thread_queue.set_policy(policy, None).await.unwrap();
        super::super::gateway_stamp::test_support::stamp_for("fixture-device");
        let key = StampKey::read_handover("ab".repeat(32).as_bytes()).unwrap();
        let envelope = OperationEnvelope {
            request: RoutingRequest {
                via: Some("routing-gateway".into()),
                to: Some("stamp-test-workspace".into()),
                operation: WorkspaceOperation::Spawn {
                    body: serde_json::json!({
                        "message": "Start a task",
                        "thread_id": Uuid::new_v4(),
                        "mode": "agent",
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                },
            },
            caller_gateway: "source-gateway".into(),
            caller_workspace: "source".into(),
            caller_thread_id: Uuid::new_v4().to_string(),
            verdict: SpawnVerdict::Allowed.to_header(),
            nonce: Uuid::new_v4().to_string(),
        };
        let digest = envelope.digest().unwrap();
        let stamp = key.sign_now(Claim::WorkspaceOperation(envelope.claim(&digest)));
        let nonce = envelope.nonce.clone();
        let signed = SignedOperation { envelope, stamp };
        let allowed = || {
            let mut headers = HeaderMap::new();
            headers.insert(
                HEADER_REMOTE_START,
                RemoteStart::allowed()
                    .header_value(&key, "stamp-test-workspace", &nonce)
                    .parse()
                    .unwrap(),
            );
            headers
        };
        let wrong = receive_read(State(state.clone()), allowed(), Json(signed.clone()))
            .await
            .unwrap_err()
            .into_response();
        assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);
        let right = receive_start(State(state.clone()), allowed(), Json(signed))
            .await
            .into_response();
        assert_eq!(
            right.status(),
            StatusCode::OK,
            "the misrouted post must not have spent the nonce"
        );
        drop(state);
        drop(engine);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[test]
    fn routed_spawns_cannot_carry_device_identity_or_existing_thread_controls() {
        for field in [
            "device_id",
            "input_chain",
            "input_seq",
            "parent_thread_id",
            "wake_mode",
            "view_state",
        ] {
            let mut body = serde_json::json!({"message":"Start a task", "thread_id":"00000000-0000-4000-8000-000000000001"})
                .as_object()
                .unwrap()
                .clone();
            body.insert(field.into(), "claimed".into());
            assert!(validate_spawn_body(&body).is_err(), "{field}");
        }
        let body = serde_json::json!({"message":"Start a task", "thread_id":"00000000-0000-4000-8000-000000000001", "mode":"agent", "coding_agent":"codex", "use_coding_agent":true});
        assert!(validate_spawn_body(body.as_object().unwrap()).is_ok());
        for thread in [serde_json::Value::Null, "invalid".into()] {
            let mut invalid = body.as_object().unwrap().clone();
            invalid.insert("thread_id".into(), thread);
            assert!(validate_spawn_body(&invalid).is_err());
        }
    }

    #[tokio::test]
    async fn routed_provenance_is_local_to_the_verified_request() {
        assert!(routed_origin().is_none());
        let route = RoutedOrigin {
            caller_gateway: "source-gateway".into(),
            routing_gateway: "routing-gateway".into(),
            remote_start: Some(Box::new(RemoteStartAsk {
                pairing_id: "pairing-1".into(),
                pairing_label: "Laptop".into(),
            })),
        };
        ROUTED_ORIGIN
            .scope(route.clone(), async {
                assert_eq!(routed_origin(), Some(route.clone()));
                let headers =
                    super::super::gateway_stamp::test_support::stamped_workspace_call_headers(
                        "source",
                        None,
                        &crate::engine::thread_events::SpawnVerdict::Allowed,
                    );
                let origin = actor::build_message_origin(
                    &headers,
                    ActorMode::Agent,
                    None,
                    None,
                    None,
                    None,
                    Some(super::super::actor::CallerOrigin {
                        workspace: "source".into(),
                        thread_id: None,
                        event_id: None,
                        mode: ActorMode::Agent,
                    }),
                );
                assert!(
                    matches!(origin, Some(crate::engine::thread_events::MessageOrigin::Workspace {
                caller_gateway: Some(source), routing_gateway: Some(router), remote_start, ..
            }) if source == "source-gateway" && router == "routing-gateway"
                && remote_start == route.remote_start)
                );
            })
            .await;
        assert!(routed_origin().is_none());
    }
}
