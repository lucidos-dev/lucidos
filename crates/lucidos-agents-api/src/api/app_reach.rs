use axum::extract::{MatchedPath, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::error::ApiError;
use crate::app_reach_policy::app_may_call;
pub(crate) use crate::app_reach_policy::APP_ID_HEADER;

/// INVARIANT: Refuse a stamped request to a route apps may not reach.
///
/// The bridge already refused it, which is the boundary. This is the second
/// answer, for a host path that forgets to ask and for a future app host that
/// is not a browser frame. An unstamped request is untouched: the shell and the
/// CLI carry no stamp, and neither does a standalone app tab.
///
/// It reads the MATCHED route, so it must sit inside the nest where routing
/// happened, exactly as [`super::mutating_gate`] does.
pub(crate) async fn enforce_app_reach(request: Request, next: Next) -> Response {
    let Some(app_id) = request
        .headers()
        .get(APP_ID_HEADER)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
    else {
        return next.run(request).await;
    };
    let Some(route) = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
    else {
        return next.run(request).await;
    };
    let mounted = route
        .strip_prefix(crate::net_config::API_V1_PREFIX)
        .unwrap_or(&route)
        .to_string();
    if !app_may_call(&mounted, request.method().as_str()) {
        crate::log_legacy!(
            "[API] Refusing {} {} for app '{}': not app-reachable (ADR 0231)",
            request.method(),
            mounted,
            app_id
        );
        return ApiError::new(
            StatusCode::FORBIDDEN,
            format!(
                "An app may not call {} {}. See system-knowhow/js-sdk/lucidos-apiurl.md, lucidos.request.",
                request.method(),
                mounted
            ),
        )
        .into_response();
    }
    if let Some(key) = human_only_preference(&mounted, request.method(), request.uri().query()) {
        crate::log_legacy!(
            "[API] Refusing PUT /preferences '{}' for app '{}': a human-only key",
            key,
            app_id
        );
        return ApiError::new(
            StatusCode::FORBIDDEN,
            format!(
                "An app may not write the preference '{key}'. The user changes it in Settings."
            ),
        )
        .into_response();
    }
    next.run(request).await
}

fn human_only_preference(
    mounted: &str,
    method: &axum::http::Method,
    query: Option<&str>,
) -> Option<String> {
    if mounted != "/preferences" || method != axum::http::Method::PUT {
        return None;
    }
    query?
        .split('&')
        .filter_map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (form_decode(name) == "key").then(|| form_decode(value))
        })
        .find(|key| {
            crate::core::prefs::internal_hint(key).is_some()
                || APP_REFUSED_PREFERENCES.contains(&key.as_str())
        })
}

const APP_REFUSED_PREFERENCES: &[&str] = &[
    crate::core::prefs::CODING_AGENT_CLAUDE_PATH.key(),
    crate::core::prefs::CODING_AGENT_CODEX_PATH.key(),
    crate::core::prefs::CODING_AGENT_CLAUDE_PERMISSION_MODE.key(),
    crate::core::prefs::OPENCODE_FREE_ENABLED.key(),
];

/// INVARIANT: Decode one `application/x-www-form-urlencoded` component, as axum's `Query`
/// does. An undecodable one stays raw, and `Query` refuses that request anyway.
fn form_decode(component: &str) -> String {
    let spaced = component.replace('+', " ");
    urlencoding::decode(&spaced)
        .map(|decoded| decoded.into_owned())
        .unwrap_or(spaced)
}

#[cfg(test)]
#[path = "app_reach_tests.rs"]
mod tests;
