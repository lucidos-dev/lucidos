//! Authenticated diagnostic ingress and agent-only bounded inbox queries.
use super::{ApiError, AppState};
use axum::{
    extract::{Path, RawQuery, Request, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use lucidos_diagnostic_log::bundle::MAX_COMPRESSED_BYTES;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/diagnostic-bundles", post(receive))
        .route("/diagnostic-bundles/:id/query", get(query))
}

async fn receive(
    State(state): State<AppState>,
    req: Request,
) -> Result<Json<serde_json::Value>, ApiError> {
    static RECEIVES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    ingest(&state.engine.event_bus, req, &RECEIVES)
        .await
        .map(|id| Json(serde_json::json!({"bundle_id": id})))
}

async fn ingest(
    bus: &crate::engine::event_bus::EventBus,
    req: Request,
    slots: &'static tokio::sync::Semaphore,
) -> Result<Uuid, ApiError> {
    let delivery = super::gateway_stamp::diagnostic_delivery(req.headers()).ok_or_else(|| {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "An authenticated diagnostic delivery is required",
        )
    })?;
    let permit = slots
        .try_acquire()
        .map_err(|_| ApiError::new(StatusCode::TOO_MANY_REQUESTS, "Diagnostic receiver busy"))?;
    // Enforce while streaming, including chunked bodies with no Content-Length.
    let bytes = axum::body::to_bytes(req.into_body(), MAX_COMPRESSED_BYTES)
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Diagnostic body unavailable or too large",
            )
        })?;
    let (permit, incoming) = tokio::task::spawn_blocking(move || {
        let incoming = if format!("{:x}", Sha256::digest(&bytes)) == delivery.body_digest() {
            crate::diagnostic_bundles::Incoming::new(
                delivery.origin(),
                delivery.delivery_key(),
                &bytes,
            )
        } else {
            Err("Diagnostic body digest mismatch".into())
        };
        // Cancellation leaves the slot with this task until decoding finishes.
        (permit, incoming)
    })
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Diagnostic receiver unavailable",
        )
    })?;
    let _permit = permit;
    // Decoder errors may quote private input. Never return or log them.
    let incoming = incoming.map_err(|_| ApiError::bad_request("Invalid diagnostic bundle"))?;
    bus.receive_diagnostic_bundle(&incoming)
        .await
        .map_err(|error| {
            let status = if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists)
            {
                StatusCode::CONFLICT
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };
            ApiError::new(status, "Diagnostic receipt could not be stored")
        })
}

async fn query(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Result<Json<crate::diagnostic_log::query::QueryResult>, ApiError> {
    query_inbox(&state.pool, id, raw.as_deref(), &headers)
        .await
        .map(Json)
}

async fn query_inbox(
    pool: &sqlx::PgPool,
    id: Uuid,
    raw: Option<&str>,
    headers: &HeaderMap,
) -> Result<crate::diagnostic_log::query::QueryResult, ApiError> {
    if !super::history::diagnostic_log_reader_allowed(headers) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "The diagnostic inbox answers only this workspace's agents",
        ));
    }
    let filter = crate::diagnostic_log::query::parse_inbox(raw, chrono::Utc::now())
        .map_err(|_| ApiError::bad_request("Invalid diagnostic query"))?;
    crate::diagnostic_bundles::query_received(pool, id, &filter)
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Diagnostic inbox unavailable or busy",
            )
        })?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "Diagnostic bundle not found or expired",
            )
        })
}

#[cfg(test)]
#[path = "diagnostic_bundles_tests.rs"]
mod tests;
