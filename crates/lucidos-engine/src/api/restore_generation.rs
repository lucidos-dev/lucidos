//! Fence browser mutations against the durable restore boundary before dispatch.
//! Identity exemptions are deliberately independent of generation admission.
use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::{error::ApiError, AppState};

pub(super) const GENERATION: &str = "x-lucidos-restore-generation";
const CALLER_KIND: &str = "x-lucidos-caller-kind";

fn browser_shaped(headers: &HeaderMap) -> bool {
    headers.contains_key(super::actor::HEADER_DEVICE_ID)
        || headers.contains_key("origin")
        || headers
            .keys()
            .any(|name| name.as_str().starts_with("sec-fetch-"))
}

fn requires_generation(headers: &HeaderMap, route: &str, local: bool) -> bool {
    // Device bootstrap establishes identity, never dispatches archived intent.
    if matches!(route, "/devices/register" | "/devices/hand-over") {
        return false;
    }
    let kind = headers.get(CALLER_KIND).and_then(|v| v.to_str().ok());
    let browser = browser_shaped(headers) || (local && kind == Some("browser"));
    // A forwarded local token authenticates the gateway hop, not its caller.
    // Only the gateway's authenticated process stamp can exempt that caller.
    let forwarded = headers.contains_key("x-forwarded-prefix");
    if local && !browser && (kind == Some("local-process") || (kind.is_none() && !forwarded)) {
        return false;
    }
    // Third-party ingress retains its own authentication. A browser cannot
    // borrow this exemption by selecting an identity-exempt route.
    if !browser
        && (!forwarded || (local && kind == Some("external")))
        && matches!(
            route,
            "/webhooks/:id/deliver" | "/proxy/:name" | "/proxy/:name/" | "/proxy/:name/*path"
        )
    {
        return false;
    }
    true
}

fn matches_generation(headers: &HeaderMap, current: Option<uuid::Uuid>) -> bool {
    let mut values = headers.get_all(GENERATION).iter();
    let Some(value) = values.next() else {
        // Legacy clients can work before the first restore. Missing provenance
        // is never interpreted as original-generation intent after a restore.
        return current.is_none();
    };
    if values.next().is_some() {
        return false;
    }
    let Ok(value) = value.to_str() else {
        return false;
    };
    match current {
        None => value == "none",
        Some(current) => uuid::Uuid::parse_str(value).is_ok_and(|got| got == current),
    }
}

pub(super) async fn enforce(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    enforce_with_pool(State((state.pool, state.call_tickets)), request, next).await
}

async fn enforce_with_pool(
    State((pool, tickets)): State<(sqlx::PgPool, super::voice_ticket::CallTickets)>,
    request: Request,
    next: Next,
) -> Response {
    let Some(matched) = request.extensions().get::<MatchedPath>() else {
        return next.run(request).await;
    };
    let route = super::mutating_gate::mounted_route(matched.as_str());
    if !super::mutating_gate::request_mutates(request.method(), route)
        || !requires_generation(
            request.headers(),
            route,
            super::local_auth::is_local_process(request.headers()),
        )
    {
        return next.run(request).await;
    }
    let boundary = match crate::core::snapshot::boundary::read(&pool).await {
        Ok(boundary) => boundary,
        Err(error) => return ApiError::db(error).into_response(),
    };
    let generation = boundary.map(|b| b.restore_id);
    // Browsers cannot set headers on WebSocket handshakes. Only a live ticket
    // minted in this generation can supply the missing provenance. An explicit
    // header still has to agree; device identity alone never exempts a socket.
    let ticket_matches = route == "/voice"
        && super::voice_ticket::ticket_in(request.uri())
            .and_then(|ticket| tickets.peek_device(&ticket, generation))
            .is_some();
    let admitted = if route == "/voice" {
        ticket_matches
            && (!request.headers().contains_key(GENERATION)
                || matches_generation(request.headers(), generation))
    } else {
        matches_generation(request.headers(), generation)
    };
    if !admitted {
        return ApiError::new(
            StatusCode::CONFLICT,
            "Workspace restored; review retained intent before submitting again.",
        )
        .with_reason("restore_generation_changed")
        .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn durable_boundary_fences_before_handler_without_waiting_for_hello() {
        use axum::{
            body::Body,
            http::Method,
            routing::{get, post},
            Router,
        };
        use tower::ServiceExt;
        let (pool, db) = crate::test_support::setup_test_db().await;
        let generation = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO snapshot_restore_boundary (singleton, restore_id, snapshot, restored_at, event_sequence) VALUES (TRUE, $1, 'fixture', NOW(), 0)")
            .bind(generation).execute(&pool).await.unwrap();
        let tickets = crate::api::voice_ticket::CallTickets::new();
        let router = Router::new()
            .route("/chat/stream", post(|| async { StatusCode::ACCEPTED }))
            .route("/voice", get(|| async { StatusCode::ACCEPTED }))
            .route("/threads/list", get(|| async { StatusCode::ACCEPTED }))
            .layer(axum::middleware::from_fn_with_state(
                (pool.clone(), tickets.clone()),
                enforce_with_pool,
            ));
        for (method, path, header, expected) in [
            (Method::POST, "/chat/stream", None, StatusCode::CONFLICT),
            (
                Method::POST,
                "/chat/stream",
                Some("none".to_string()),
                StatusCode::CONFLICT,
            ),
            (
                Method::POST,
                "/chat/stream",
                Some(uuid::Uuid::new_v4().to_string()),
                StatusCode::CONFLICT,
            ),
            (
                Method::POST,
                "/chat/stream",
                Some(generation.to_string()),
                StatusCode::ACCEPTED,
            ),
            (Method::GET, "/voice", None, StatusCode::CONFLICT),
            (Method::GET, "/threads/list", None, StatusCode::ACCEPTED),
        ] {
            let mut request = Request::builder().method(method).uri(path);
            if let Some(header) = header {
                request = request.header(GENERATION, header);
            }
            let response = router
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{path}");
        }
        let home = uuid::Uuid::new_v4();
        let old = tickets.mint("fixture-device", home, None).unwrap();
        let current = tickets
            .mint("fixture-device", home, Some(generation))
            .unwrap();
        for (ticket, explicit, expected) in [
            (&old, None, StatusCode::CONFLICT),
            (&current, None, StatusCode::ACCEPTED),
            (&current, Some("none".to_string()), StatusCode::CONFLICT),
            (&current, Some(generation.to_string()), StatusCode::ACCEPTED),
        ] {
            let mut request = Request::builder()
                .uri(format!("/voice?ticket={ticket}"))
                .header(super::super::actor::HEADER_DEVICE_ID, "fixture-device");
            if let Some(value) = explicit {
                request = request.header(GENERATION, value);
            }
            assert_eq!(
                router
                    .clone()
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap()
                    .status(),
                expected
            );
        }
        pool.close().await;
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat/stream")
                    .header(GENERATION, generation.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        crate::test_support::teardown_test_db(&db).await;
    }

    #[test]
    fn restored_workspace_requires_exact_single_generation() {
        let generation = uuid::Uuid::new_v4();
        let mut headers = HeaderMap::new();
        assert!(!matches_generation(&headers, Some(generation)));
        headers.insert(GENERATION, "none".parse().unwrap());
        assert!(matches_generation(&headers, None));
        assert!(!matches_generation(&headers, Some(generation)));
        headers.insert(GENERATION, generation.to_string().parse().unwrap());
        assert!(matches_generation(&headers, Some(generation)));
        assert!(!matches_generation(&headers, Some(uuid::Uuid::new_v4())));
        assert!(!matches_generation(&headers, None));
        headers.append(GENERATION, generation.to_string().parse().unwrap());
        assert!(!matches_generation(&headers, Some(generation)));
    }

    #[test]
    fn gateway_token_does_not_exempt_browser_or_body_identity_routes() {
        let mut headers = HeaderMap::new();
        headers.insert(CALLER_KIND, "browser".parse().unwrap());
        for route in [
            "/chat/stream",
            "/threads/:thread_id/follow-up",
            "/internal/client-log",
            "/notification/read",
            "/voice",
            "/proxy/:name",
        ] {
            assert!(requires_generation(&headers, route, true), "{route}");
        }
        headers.insert(CALLER_KIND, "local-process".parse().unwrap());
        assert!(!requires_generation(&headers, "/chat/stream", true));
        assert!(requires_generation(&headers, "/chat/stream", false));
        headers.remove(CALLER_KIND);
        headers.insert("x-forwarded-prefix", "/workspace/".parse().unwrap());
        assert!(requires_generation(&headers, "/chat/stream", true));
        assert!(requires_generation(&headers, "/proxy/:name", true));
        headers.insert(CALLER_KIND, "external".parse().unwrap());
        assert!(!requires_generation(&headers, "/proxy/:name", true));
        assert!(requires_generation(&headers, "/chat/stream", true));
        headers.insert(
            super::super::actor::HEADER_DEVICE_ID,
            "device".parse().unwrap(),
        );
        assert!(requires_generation(&headers, "/chat/stream", true));
    }
}
