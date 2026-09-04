//! Refuse a mutating request that said nothing about who is making it.
//!
//! Full design: `docs/plans/2026-08-30-a-thread-acts-in-its-own-subtree.md`
//! § Phase 2b. The rule is ADR 0169: a caller presenting no credential is
//! refused, never stamped as the user.
//!
//! # Why a layer and not seventy-seven handler edits
//!
//! The same reason [`crate::api::target_workspace`] is a layer. A write nobody
//! can be attributed to is a hazard on every mutating endpoint, and a
//! per-handler check is one the next endpoint can forget. Four routes drifted
//! apart exactly that way before ADR 0083.
//!
//! The measured shape settled it. 77 handlers resolve an actor and refuse
//! nobody, across 30 distinct return types, 33 of them infallible. A refusal
//! rendered in each would be 33 signature changes before the first gate.
//!
//! Each exempt route is one a legitimate caller CANNOT identify itself on,
//! never one that merely forgot to. [`is_exempt`] carries the list and the
//! reason per entry.

use axum::extract::{MatchedPath, Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::AppState;

/// Does this method write?
///
/// GET and HEAD are reads, and OPTIONS is the CORS preflight, which carries no
/// credential by design. Everything else changes something and owes a name.
fn method_mutates(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// A [`MatchedPath`] as the module routers spell it, with the mount stripped.
///
/// `MatchedPath` reports the FULL path a nest resolved to, so a route the
/// `settings` router registers as `/devices/register` arrives here as
/// `/api/v1/devices/register`. Every entry below is written the way its own
/// module writes it, which is the spelling a reader greps for.
///
/// A path that does not carry the mount is passed through untouched, which is
/// already the module's own spelling, so the list answers it correctly. The
/// mount is stripped exactly once: a route that merely repeats it deeper down
/// is a different route and stays gated.
fn mounted_route(matched: &str) -> &str {
    matched
        .strip_prefix(super::API_V1_PREFIX)
        .unwrap_or(matched)
}

/// Routes that must answer a caller holding none of the four credentials.
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
    // Identity in the body.
    "/chat/stream",
    "/threads/:thread_id/follow-up",
    // The device bootstrap.
    "/devices/register",
    "/devices/hand-over",
    // Third-party ingress.
    "/webhooks/:id/deliver",
    "/proxy/:name",
    "/proxy/:name/",
    "/proxy/:name/*path",
    // Worker callers, which cannot reach a device id.
    "/internal/client-log",
    "/internal/client-logs",
    "/presence-pong",
    "/notification/read",
];

/// Is `route` on [`EXEMPT_ROUTES`]?
fn is_exempt(route: &str) -> bool {
    EXEMPT_ROUTES.contains(&route)
}

/// Refuse a mutating request carrying no identity, before it reaches a handler.
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
/// `actor::user_actor_resolved` and, past here, always gets a `Some`.
pub(crate) async fn enforce_caller_identified(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if !method_mutates(request.method()) {
        return next.run(request).await;
    }
    let Some(route) = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
    else {
        return next.run(request).await;
    };
    if is_exempt(mounted_route(&route)) {
        return next.run(request).await;
    }
    if let Err(refusal) =
        super::actor::require_user_actor(request.headers(), &state.pool, None).await
    {
        crate::log!(
            "[API] Refusing an unidentified {} {}: no credential to record who is acting",
            request.method(),
            route
        );
        return refusal.into_response();
    }
    next.run(request).await
}

#[cfg(test)]
#[path = "mutating_gate_tests.rs"]
mod tests;
