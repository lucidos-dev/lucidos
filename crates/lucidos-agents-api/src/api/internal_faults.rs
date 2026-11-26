use crate::internal_faults::FaultKind;
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};

/// INVARIANT: In-process only: upstream headers cannot forge or remove this marker.
#[derive(Clone, Copy)]
pub(crate) struct ExternalFailure;

pub(crate) fn external(mut response: Response) -> Response {
    response.extensions_mut().insert(ExternalFailure);
    response
}

fn is_own_fault(response: &Response) -> bool {
    response.status() == axum::http::StatusCode::INTERNAL_SERVER_ERROR
        && response.extensions().get::<ExternalFailure>().is_none()
}

pub(super) async fn observe(
    State(inbox): State<lucidos_diagnostic_log::fault::Inbox>,
    request: Request,
    next: Next,
) -> Response {
    let response = next.run(request).await;
    if is_own_fault(&response) {
        // INVARIANT: Durably queue first: a DB error may be what produced this response.
        if let Err(e) = inbox.record(FaultKind::Own5xx) {
            crate::log_legacy!("[InternalFault] could not queue own-5xx: {}", e);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    #[tokio::test]
    async fn internal_faults_middleware_records_only_our_response_and_preserves_bodies() {
        use tower::ServiceExt;
        let root = tempfile::tempdir().unwrap();
        let inbox = lucidos_diagnostic_log::fault::Inbox::new(root.path());
        let app = axum::Router::new()
            .route(
                "/upstream",
                axum::routing::get(|| async {
                    external((StatusCode::INTERNAL_SERVER_ERROR, "upstream").into_response())
                }),
            )
            .route(
                "/ours",
                axum::routing::get(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "ours") }),
            )
            .layer(axum::middleware::from_fn_with_state(inbox.clone(), observe));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/upstream")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(inbox.pending().unwrap().is_empty());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ours")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 100)
                .await
                .unwrap(),
            "ours"
        );
        let pending = inbox.pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, FaultKind::Own5xx);
    }

    #[test]
    fn internal_faults_exclude_provider_network_rate_limit_and_readiness() {
        assert!(is_own_fault(
            &StatusCode::INTERNAL_SERVER_ERROR.into_response()
        ));
        for status in [
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            assert!(!is_own_fault(&external(status.into_response())));
        }
        for status in [
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::BAD_REQUEST,
        ] {
            assert!(!is_own_fault(&status.into_response()));
        }
    }
}
