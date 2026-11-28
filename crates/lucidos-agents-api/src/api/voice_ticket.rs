use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use super::error::ApiError;
use super::secret_reveal::{reveal_request_allowed, RefererRule};
use super::socket_ticket::{SocketTarget, SocketTicketView, SocketTickets};
use super::AppState;
use crate::engine::thread_events::MessageOrigin;

pub(super) const NOT_FROM_THE_SHELL: &str =
    "A call starts from the Lucidos page itself, never from an app \
     page. This request named an app page, or no page at all. If this is Lucidos, the browser \
     is withholding the page address (the Referer header): allow it for this site.";

#[derive(Deserialize)]
struct MintRequest {
    thread_id: Uuid,
}

async fn mint_ticket(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<MintRequest>,
) -> Result<Json<SocketTicketView>, ApiError> {
    let ticket = mint(
        &state.pool,
        &state.engine.event_bus,
        &state.engine.voice_sessions,
        &state.socket_tickets,
        &headers,
        request.thread_id,
    )
    .await?;
    Ok(Json(SocketTicketView::new(ticket)))
}

pub(super) async fn mint(
    pool: &sqlx::PgPool,
    event_bus: &crate::engine::event_bus::EventBus,
    sessions: &crate::voice::registry::LiveVoiceSessions,
    tickets: &SocketTickets,
    headers: &HeaderMap,
    thread_id: Uuid,
) -> Result<String, ApiError> {
    // INVARIANT: The strict rule: a browser-shaped mint must name the page it came from,
    // and that page must not be an app. A `POST` passes the service worker
    // untouched, so the shell's own `Referer` always arrives.
    if !reveal_request_allowed(headers, RefererRule::Required) {
        crate::log_legacy!(
            "[Voice] Refusing a socket ticket for an app document or an unnamed page"
        );
        return Err(ApiError::new(StatusCode::FORBIDDEN, NOT_FROM_THE_SHELL));
    }
    let generation = crate::core::snapshot::boundary::read(pool)
        .await
        .map_err(ApiError::db)?
        .map(|boundary| boundary.restore_id);
    let act = "start a voice call";
    let actor = super::actor::require_owner_device(headers, pool, event_bus, None, act).await?;
    let MessageOrigin::Device { device_id, .. } = &actor else {
        return Err(super::actor::OwnerGateRefusal::NotAPerson.render(act));
    };
    super::voice::refuse_call_on(pool, sessions, thread_id).await?;
    tickets
        .mint(device_id, SocketTarget::Call { thread_id }, generation)
        .ok_or_else(|| ApiError::internal("The socket ticket store is poisoned."))
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/voice/ticket", post(mint_ticket))
}
