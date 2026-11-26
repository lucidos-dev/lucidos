use super::*;
use crate::engine::form_requests;
use crate::engine::thread_events::FormRequestOutcome;

/// INVARIANT: The kinds this route cancels. A plugin request cancels through its own
/// route, which also drops the staged files. An authorization page closes when
/// its OAuth flow ends.
const CANCELABLE_HERE: &[&str] = &["CredentialRequested", "EmailConfirmRequested"];

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
    resolved: bool,
}

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

#[derive(Deserialize)]
struct ResolvedQuery {
    request_ids: String,
}

/// INVARIANT: Resolve only identities with event evidence, never absence from the pending list.
async fn list_resolved(
    State(state): State<AppState>,
    Query(query): Query<ResolvedQuery>,
) -> Result<Json<Vec<uuid::Uuid>>, ApiError> {
    let ids: Vec<uuid::Uuid> = query
        .request_ids
        .split(',')
        .take(101)
        .map(|id| {
            id.parse()
                .map_err(|_| ApiError::bad_request("Invalid form request id"))
        })
        .collect::<Result<_, _>>()?;
    if ids.len() > 100 {
        return Err(ApiError::bad_request(
            "At most 100 form request ids are allowed",
        ));
    }
    form_requests::resolved_ids(&state.pool, &ids)
        .await
        .map(Json)
        .map_err(ApiError::db)
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/form-requests/pending", get(list_pending))
        .route("/form-requests/resolved", get(list_resolved))
        .route("/form-requests/:request_id/cancel", post(cancel))
}
