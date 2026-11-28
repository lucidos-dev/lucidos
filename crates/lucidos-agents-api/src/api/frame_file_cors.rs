use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

fn is_granted(headers: &HeaderMap) -> bool {
    match headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok()) {
        Some("font") => true,
        Some("script") => crate::core::base_path::forwarded_prefix(headers) != "/",
        _ => false,
    }
}

pub(super) async fn grant_font_and_script_loads(request: Request, next: Next) -> Response {
    let granted = is_granted(request.headers());
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.append(header::VARY, HeaderValue::from_static("sec-fetch-dest"));
    if granted {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt as _;

    #[derive(Clone, Copy, Debug)]
    enum Via {
        Gateway,
        Direct,
    }

    async fn answer(dest: Option<&str>, via: Via) -> Response {
        let app = axum::Router::new()
            .route(
                "/app/habit-tracker/main.js",
                axum::routing::get(|| async { "ok" }),
            )
            .layer(axum::middleware::from_fn(grant_font_and_script_loads));
        let mut request = axum::http::Request::builder()
            .uri("/app/habit-tracker/main.js")
            .header(header::ORIGIN, "null");
        if let Some(dest) = dest {
            request = request.header("sec-fetch-dest", dest);
        }
        if let Via::Gateway = via {
            request = request.header("x-forwarded-prefix", "/dev/");
        }
        app.oneshot(request.body(axum::body::Body::empty()).unwrap())
            .await
            .expect("the router answers")
    }

    fn allow_origin(response: &Response) -> Option<&HeaderValue> {
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
    }

    fn assert_granted(response: &Response, what: &str) {
        assert_eq!(
            allow_origin(response),
            Some(&HeaderValue::from_static("*")),
            "{what}"
        );
        assert_eq!(
            response.headers().get(header::X_CONTENT_TYPE_OPTIONS),
            Some(&HeaderValue::from_static("nosniff")),
            "a grant must not let a text file run as a script: {what}"
        );
    }

    fn varies_on_destination(response: &Response) -> bool {
        response.headers().get_all(header::VARY).iter().any(|v| {
            v.to_str()
                .is_ok_and(|v| v.eq_ignore_ascii_case("sec-fetch-dest"))
        })
    }

    #[tokio::test]
    async fn a_font_loads_across_origins_however_it_arrived() {
        for via in [Via::Gateway, Via::Direct] {
            assert_granted(&answer(Some("font"), via).await, &format!("{via:?}"));
        }
    }

    #[tokio::test]
    async fn a_module_script_loads_across_origins_behind_the_gateway() {
        assert_granted(&answer(Some("script"), Via::Gateway).await, "script");
    }

    #[tokio::test]
    async fn a_script_hitting_the_engine_directly_is_not_granted() {
        assert_eq!(
            allow_origin(&answer(Some("script"), Via::Direct).await),
            None
        );
    }

    #[tokio::test]
    async fn nothing_that_hands_the_page_bytes_is_granted() {
        for via in [Via::Gateway, Via::Direct] {
            for dest in [
                None,
                Some("empty"),
                Some("style"),
                Some("image"),
                Some("json"),
                Some("iframe"),
            ] {
                let response = answer(dest, via).await;
                assert_eq!(allow_origin(&response), None, "{dest:?} {via:?}");
            }
        }
    }

    #[tokio::test]
    async fn every_answer_varies_on_the_destination() {
        for dest in [None, Some("empty"), Some("font"), Some("script")] {
            let response = answer(dest, Via::Direct).await;
            assert!(varies_on_destination(&response), "{dest:?}");
        }
    }

    #[tokio::test]
    async fn the_destination_is_matched_exactly() {
        assert_eq!(allow_origin(&answer(Some("Font"), Via::Direct).await), None);
        assert_eq!(
            allow_origin(&answer(Some("font, empty"), Via::Direct).await),
            None
        );
    }
}
