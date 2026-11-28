use axum::extract::{MatchedPath, Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::AppState;

pub(super) fn method_mutates(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

pub(super) fn mounted_route(matched: &str) -> &str {
    matched
        .strip_prefix(crate::net_config::API_V1_PREFIX)
        .unwrap_or(matched)
}

/// INVARIANT: Routes that must answer a caller holding none of the four credentials.
///
/// Matched on the ROUTE, never the request URI, so `/webhooks/:id/deliver` is
/// one entry rather than a prefix comparison a real path could slip past. Four
/// classes, each stating the credential it cannot present:
///
/// - **Identity arrives in the body.** `caller_workspace` names another
///   workspace, and no header carries it. Both routes gate on it themselves.
/// - **The device bootstrap.** A device is not in `devices` until it registers,
///   and `actor::require_user_actor` suppresses an id that names no row.
/// - **Third-party ingress.** A sender that is not Lucidos holds no Lucidos
///   credential. Webhook delivery authenticates by HMAC over the body, and the
///   proxy forwards elsewhere and emits no actor-bearing event.
/// - **A worker caller.** Two conditions together: no device id is reachable
///   where it runs, AND the route already works with no actor. A service worker
///   and the SSE worker have no `localStorage`. `sw.js` is the only caller of
///   `/notification/read`, and gating it stuck the unread badge on a push tap.
///
/// `POST /device-presence` looks like that last class and is NOT here. Its
/// caller is the page, so it was taught the header instead.
pub(super) const EXEMPT_ROUTES: &[&str] = &[
    "/chat/stream",
    "/threads/:thread_id/follow-up",
    lucidos_gateway_stamp::workspace_operation::RECEIVE_ROUTE,
    lucidos_gateway_stamp::workspace_operation::RECEIVE_START_ROUTE,
    "/devices/register",
    "/devices/hand-over",
    "/diagnostic-bundles",
    "/webhooks/:id/deliver",
    "/proxy/:name",
    "/proxy/:name/",
    "/proxy/:name/*path",
    "/internal/client-log",
    "/internal/client-logs",
    "/presence-pong",
    "/notification/read",
];

fn is_exempt(route: &str) -> bool {
    EXEMPT_ROUTES.contains(&route)
}

pub(super) const MUTATING_SOCKET_ROUTES: &[&str] =
    &["/voice", "/threads/:thread_id/terminal/:terminal_id/socket"];

pub(super) fn request_mutates(method: &Method, route: &str) -> bool {
    method_mutates(method) || MUTATING_SOCKET_ROUTES.contains(&route)
}

async fn refuse_unidentified(
    pool: &sqlx::PgPool,
    tickets: &super::socket_ticket::SocketTickets,
    method: &Method,
    matched: Option<&str>,
    headers: &axum::http::HeaderMap,
    uri: &axum::http::Uri,
) -> Result<(), super::error::ApiError> {
    let Some(matched) = matched else {
        return Ok(());
    };
    let route = mounted_route(matched);
    if !request_mutates(method, route) || is_exempt(route) {
        return Ok(());
    }
    let generation = if MUTATING_SOCKET_ROUTES.contains(&route) {
        crate::core::snapshot::boundary::read(pool)
            .await
            .map_err(super::error::ApiError::db)?
            .map(|boundary| boundary.restore_id)
    } else {
        None
    };
    let socket_claim = (MUTATING_SOCKET_ROUTES.contains(&route)
        && !headers.contains_key(crate::core::subprocess_origin::HEADER_DEVICE_ID))
    .then(|| super::socket_ticket::ticket_in(uri))
    .flatten()
    .and_then(|ticket| tickets.peek_device(&ticket, generation));
    if let Err(refusal) =
        super::actor::require_user_actor(headers, pool, socket_claim.as_deref()).await
    {
        crate::log_legacy!(
            "[API] Refusing an unidentified {} {}: no credential to record who is acting",
            method,
            matched
        );
        return Err(refusal);
    }
    Ok(())
}

/// INVARIANT: Refuse a mutating request carrying no identity, before it reaches a handler.
///
/// Layered inside [`crate::api::target_workspace::enforce_target_workspace`], so
/// a mis-aimed write still gets the 409 naming the right engine rather than a
/// 401 about a credential it did present.
///
/// A request with no [`MatchedPath`] matched the router's fallback. That is a
/// 404, and answering it with a credential complaint would tell a caller to fix
/// the wrong thing.
///
/// The layer does not hand the handler its actor. Each keeps calling
/// `actor::user_actor` and, past here, always gets a `Some`.
pub(crate) async fn enforce_caller_identified(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let matched = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string());
    if let Err(refusal) = refuse_unidentified(
        &state.pool,
        &state.socket_tickets,
        request.method(),
        matched.as_deref(),
        request.headers(),
        request.uri(),
    )
    .await
    {
        return refusal.into_response();
    }
    next.run(request).await
}

#[cfg(test)]
#[path = "mutating_gate_tests.rs"]
mod tests;
