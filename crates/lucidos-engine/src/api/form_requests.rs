//! HTTP surface for *form requests*: the open list the client offers from, and
//! the Cancel a credential or email form sends. See `engine::form_requests`.

use super::*;
use crate::engine::form_requests;
use crate::engine::thread_events::FormRequestOutcome;

/// The kinds this route cancels. A plugin request cancels through its own
/// route, which also drops the staged files. An authorization page closes when
/// its OAuth flow ends.
const CANCELABLE_HERE: &[&str] = &["CredentialRequested", "EmailConfirmRequested"];

/// GET /api/v1/form-requests/pending: every open form request, oldest first.
///
/// The client calls this on every stream open, so a request whose frame it
/// missed still reaches it. Stagings past their TTL are closed as expired
/// here. A plugin request with no staging is left out, so it is never offered
/// with a Confirm that would 404.
async fn list_pending(
    State(state): State<AppState>,
) -> Result<Json<Vec<form_requests::PendingFormRequest>>, ApiError> {
    let engine = &state.engine;
    let expired = crate::engine::tools::plugins::sweep_expired_stagings(
        &engine.pending_installs,
        &engine.pending_uninstalls,
    );
    for request_id in expired.iter().filter_map(|id| id.parse().ok()) {
        form_requests::resolve_or_log(
            &state.pool,
            &engine.event_bus,
            request_id,
            FormRequestOutcome::Expired,
            None,
        )
        .await;
    }
    let is_staged = |kind: &str, id: uuid::Uuid| {
        let key = id.to_string();
        if kind == "PluginInstallRequested" {
            engine
                .pending_installs
                .lock()
                .expect("pending_installs mutex poisoned")
                .contains_key(&key)
        } else {
            engine
                .pending_uninstalls
                .lock()
                .expect("pending_uninstalls mutex poisoned")
                .contains_key(&key)
        }
    };
    let pending = form_requests::pending(&state.pool)
        .await
        .map_err(ApiError::db)?
        .into_iter()
        .filter(|request| form_requests::is_answerable(request, is_staged))
        .collect();
    Ok(Json(pending))
}

#[derive(Serialize)]
struct CancelResponse {
    /// False when another device or path answered it first. Not an error: the
    /// request is closed either way, which is what the caller wanted.
    resolved: bool,
}

/// POST /api/v1/form-requests/{request_id}/cancel: the form's Cancel.
async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request_id): Path<uuid::Uuid>,
) -> Result<Json<CancelResponse>, ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    let kind = form_requests::event_type_of(&state.pool, request_id)
        .await
        .map_err(ApiError::db)?
        .ok_or_else(|| ApiError::not_found(format!("No form request {request_id}")))?;
    if !CANCELABLE_HERE.contains(&kind.as_str()) {
        return Err(ApiError::bad_request(format!(
            "Form request {request_id} is a {kind}, which this route does not cancel. \
             A plugin request cancels through its /api/v1/plugins route."
        )));
    }
    let resolved = form_requests::resolve(
        &state.pool,
        &state.engine.event_bus,
        request_id,
        FormRequestOutcome::Canceled,
        Some(actor),
    )
    .await
    .map_err(|e| ApiError::internal(format!("Could not cancel form request {request_id}: {e}")))?;
    Ok(Json(CancelResponse { resolved }))
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/form-requests/pending", get(list_pending))
        .route("/form-requests/:request_id/cancel", post(cancel))
}
