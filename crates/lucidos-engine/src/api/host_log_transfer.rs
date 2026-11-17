//! Owner-approved host-owned transfer. This module never reads raw host logs.
use super::{
    error::ApiError,
    permission_requests::{self, Spent},
    AppState,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    user: String,
    host: String,
    directory: String,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reason {
    reason: String,
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/engine/host-log-transfer/prepare", post(prepare))
        .route("/engine/host-log-transfer/:id/execute", post(execute))
        .route("/engine/host-log-transfer/:id/status", post(status))
}

fn caller(headers: &HeaderMap) -> Result<(uuid::Uuid, Value), ApiError> {
    let thread = permission_requests::calling_agent_thread(headers)?;
    let workspace = super::base_path::workspace_id()
        .ok_or_else(|| ApiError::internal("Workspace identity unavailable"))?;
    Ok((
        thread,
        json!({"thread_id":thread.to_string(), "workspace_id":workspace}),
    ))
}

async fn hop(path: &str, body: Value) -> Result<Value, ApiError> {
    let port = super::base_path::gateway_port()
        .ok_or_else(|| ApiError::bad_request("Host-log transfer requires a gateway"))?;
    hop_with(&port, path, body).await
}

async fn hop_with(port: &str, path: &str, body: Value) -> Result<Value, ApiError> {
    // Never retry an uncertain act or expose arbitrary gateway/transport errors.
    let text = tokio::time::timeout(
        std::time::Duration::from_secs(225),
        super::gateway_control::post_control(port, &format!("/broker/host-logs/{path}"), &body),
    )
    .await
    .map_err(|_| {
        ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "Host-log outcome uncertain; query status, do not retry",
        )
    })?
    .map_err(safe_refusal)?;
    serde_json::from_str(&text).map_err(|_| ApiError::internal("Host-log metadata unavailable"))
}
fn safe_refusal(refusal: super::gateway_control::ControlRefusal) -> ApiError {
    let fallback = "Host-log operation unavailable or refused; check captures, bounds and durable status. Do not retry an uncertain transfer.";
    let allowed = [
        "No retained file-backed host captures are available; journal-only and foreground output are unsupported",
        "Host-log snapshot exceeds the 128 MiB limit; nothing transferred",
        "Unrecognized host-log rotation; no files selected",
        "Host-log operation storage is full; wait for a pending operation to expire after 15 minutes",
        "Invalid SSH recipient or relative directory",
    ];
    let message = match &refusal {
        super::gateway_control::ControlRefusal::Refused { message, .. } => allowed
            .iter()
            .copied()
            .find(|safe| message.as_str() == *safe)
            .unwrap_or(fallback),
        _ => fallback,
    };
    ApiError::new(axum::http::StatusCode::CONFLICT, message)
}
fn checked_id(id: &str) -> Result<(), ApiError> {
    if !id.starts_with("transfer-")
        || id.len() > 80
        || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return Err(ApiError::bad_request("Invalid host-log operation"));
    }
    Ok(())
}
async fn prepare(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Prepare>,
) -> Result<Json<Value>, ApiError> {
    let (thread, caller) = caller(&headers)?;
    // Validate the reason before allocating bounded gateway storage.
    permission_requests::host_log_ask(&"0".repeat(64), "", &body.reason)?;
    let operation = hop("prepare", json!({"caller":caller, "recipient":{"user":body.user, "host":body.host, "directory":body.directory}})).await?;
    let ask = ask(&operation, &body.reason)?;
    let request = permission_requests::prepare(&state.engine.event_bus, thread, ask).await?;
    Ok(Json(
        json!({"operation":operation, "request_id":request, "next":"Ask your question tool with request_id as the only question. Only the owner can answer. Then execute this operation once."}),
    ))
}
fn ask(operation: &Value, reason: &str) -> Result<permission_requests::Ask, ApiError> {
    let digest = operation["approval"]
        .as_str()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| ApiError::internal("Invalid host-log consent manifest"))?;
    // JSON escapes control characters in installation/file metadata. Card text
    // is host-authored; no raw content and no agent-provided permission wording.
    permission_requests::host_log_ask(
        digest,
        &serde_json::to_string_pretty(operation)
            .map_err(|_| ApiError::internal("Invalid host-log manifest"))?,
        reason,
    )
}
async fn execute(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Reason>,
) -> Result<Json<Value>, ApiError> {
    let (thread, caller) = caller(&headers)?;
    let port = super::base_path::gateway_port()
        .ok_or_else(|| ApiError::bad_request("Host-log transfer requires a gateway"))?;
    execute_with(
        &state.engine.event_bus,
        thread,
        caller,
        &id,
        &body.reason,
        &port,
    )
    .await
}
async fn execute_with(
    bus: &crate::engine::event_bus::EventBus,
    thread: uuid::Uuid,
    caller: Value,
    id: &str,
    reason: &str,
    port: &str,
) -> Result<Json<Value>, ApiError> {
    checked_id(id)?;
    let operation = hop_with(port, &format!("{id}/status"), json!({"caller":caller})).await?;
    if operation["state"] != "prepared" {
        return Ok(Json(operation));
    }
    // The gateway refuses an expired operation, so spending the grant first
    // would waste the owner's approval.
    unexpired(&operation)?;
    match permission_requests::spend_or_prepare(bus, thread, ask(&operation, reason)?).await? {
        Spent::Requested(request) => Ok(Json(json!({"request_id":request, "operation":operation}))),
        Spent::Allowed(_) => Ok(Json(
            hop_with(
                port,
                &format!("{id}/transfer"),
                json!({"caller":caller,"approval":operation["approval"]}),
            )
            .await?,
        )),
        Spent::DeviceSessionScope => Err(ApiError::internal("Unexpected permission scope")),
    }
}
fn unexpired(operation: &Value) -> Result<(), ApiError> {
    let expires_at = operation["expires_at"]
        .as_u64()
        .ok_or_else(|| ApiError::internal("Invalid host-log operation expiry"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if now > expires_at {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "Host-log operation expired; prepare a new one and ask the owner again",
        ));
    }
    Ok(())
}
async fn status(headers: HeaderMap, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    let (_, caller) = caller(&headers)?;
    checked_id(&id)?;
    Ok(Json(
        hop(&format!("{id}/status"), json!({"caller":caller})).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operation_path_and_consent_manifest_are_closed_inputs() {
        for bad in [
            "../transfer-a",
            "transfer-a/status",
            "transfer-a?x",
            "transfer-a\n",
        ] {
            assert!(checked_id(bad).is_err());
        }
        assert!(checked_id("transfer-Ab1234").is_ok());
        assert!(ask(&json!({"approval":"fake"}), "diagnose crash").is_err());
        let a = ask(
            &json!({"approval":"a".repeat(64), "recipient":{"host":"logs.example.org"}}),
            "diagnose crash",
        )
        .unwrap();
        assert_eq!(
            a.action,
            crate::engine::permission::Action::TransferHostLogs
        );
        assert_eq!(a.breadth, "a".repeat(64));
        assert!(a.question.contains("secrets"));
        assert!(a.question.contains("logs.example.org"));
    }

    #[test]
    fn an_expired_or_undated_operation_never_reaches_the_grant() {
        assert!(unexpired(&json!({"expires_at": u64::MAX})).is_ok());
        assert!(unexpired(&json!({"expires_at": 0})).is_err());
        assert!(unexpired(&json!({})).is_err());
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::api::permission_requests::tests::{
        agent_thread, ask_prepared, grant, owner_device, tap,
    };
    use crate::engine::{event_bus::EventBus, permission::menu::ALLOW_ONCE_LABEL};
    use crate::test_support::{setup_test_db, teardown_test_db};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn broker_does_not_reach_transfer_without_exact_owner_consent_even_after_reopen() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = agent_thread(&bus).await;
        let other = agent_thread(&bus).await;
        let operation = json!({"id":"transfer-fixture", "approval":"a".repeat(64), "state":"prepared", "expires_at":u64::MAX, "recipient":{"host":"logs.example.org"}});
        let calls = Arc::new(AtomicUsize::new(0));
        let recorded = calls.clone();
        let answer = operation.clone();
        let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
            let calls = recorded.clone();
            let op = answer.clone();
            async move {
                if uri.path().ends_with("/transfer") {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"state":"verified"}))
                } else {
                    Json(op)
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port().to_string();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let caller = json!({"workspace_id":"demo","thread_id":thread});
        let first = execute_with(
            &bus,
            thread,
            caller.clone(),
            "transfer-fixture",
            "diagnose crash",
            &port,
        )
        .await
        .unwrap();
        assert!(first.0["request_id"].is_string());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let card = ask_prepared(&bus, thread, first.0["request_id"].as_str().unwrap()).await;
        let declined = card.pick(crate::engine::permission::menu::DONT_ALLOW_LABEL);
        tap(&bus, thread, &card, declined, Some(owner_device())).await;
        let denied = execute_with(
            &bus,
            thread,
            caller.clone(),
            "transfer-fixture",
            "diagnose crash",
            &port,
        )
        .await
        .unwrap();
        assert!(denied.0["request_id"].is_string());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        grant(
            &bus,
            thread,
            ask(&operation, "diagnose crash").unwrap(),
            ALLOW_ONCE_LABEL,
        )
        .await;
        let (reopened, _rx) = EventBus::new(pool.clone());
        let foreign = execute_with(
            &reopened,
            other,
            caller.clone(),
            "transfer-fixture",
            "diagnose crash",
            &port,
        )
        .await
        .unwrap();
        assert!(foreign.0["request_id"].is_string());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let done = execute_with(
            &reopened,
            thread,
            caller.clone(),
            "transfer-fixture",
            "diagnose crash",
            &port,
        )
        .await
        .unwrap();
        assert_eq!(done.0["state"], "verified");
        let replay = execute_with(
            &reopened,
            thread,
            caller,
            "transfer-fixture",
            "diagnose crash",
            &port,
        )
        .await
        .unwrap();
        assert!(replay.0["request_id"].is_string());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
        pool.close().await;
        teardown_test_db(&db).await;
    }
}
