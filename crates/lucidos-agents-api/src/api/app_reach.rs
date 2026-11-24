//! Whether an app frame may reach a route, declared route by route.
//!
//! An app frame runs at an opaque origin (ADR 0227), so the host bridge is its
//! only way to the engine. `lucidos.request` carries anything the SDK does not
//! name, and that hatch needs an answer per route (ADR 0231).
//!
//! **Absence is denial.** A route missing from
//! [`ROUTE_REACH`](crate::app_reach_policy::ROUTE_REACH) is refused, and the
//! completeness test below fails the build. So the question is answered when
//! the route is written, rather than when a workspace finds out.

//! | Reach | Means | Example |
//! |---|---|---|
//! | `App` | an app frame may call the listed methods | `/data/*path` |
//! | `Asset` | the app document loads it as a tag, never through the bridge | `/sdk.js` |
//! | `Host` | the Lucidos shell only | `/credential-value` |
//! | `Agent` | a coding-agent subprocess only | the `/internal/` tree |
//!
//! Three of the four refuse the hatch. They are separate words because they
//! refuse for different reasons, and whoever classifies a new route should pick
//! one on purpose. An `Asset` is reachable by the frame, as a subresource on
//! its own document, so calling it host-only would misinform the next reader.

//! # Where this is enforced
//!
//! The host bridge is the chokepoint, and it reads the generated copy of the
//! `App` rows. The engine applies the same table to any request carrying the
//! app stamp, which is defence in depth rather than the boundary.
//!
//! A standalone app tab is a same-origin top-level document, so no
//! classification binds it. ADR 0231 states that limit rather than implying a
//! wall.

use axum::extract::{MatchedPath, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::error::ApiError;
use crate::app_reach_policy::app_may_call;
pub(crate) use crate::app_reach_policy::APP_ID_HEADER;

/// Refuse a stamped request to a route apps may not reach.
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
    // No matched path is the router's fallback, which is a 404. Answering with
    // a refusal would point the caller at the wrong problem.
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

/// Preferences the agent may write but an app may not.
///
/// The coding-agent keys choose what a session spawns and what it may do
/// unasked. An app writes files under its own folder, which is the app
/// thread's working directory. So a path pointed at `/bin/sh` runs the app's
/// own script as the user when the next session starts.
///
/// The OpenCode Free switch sends chat anonymously to a third-party relay whose
/// free models may train on it. ADR 0104 makes that the human's informed
/// opt-in, so no app may turn it on unasked.
const APP_REFUSED_PREFERENCES: &[&str] = &[
    crate::core::prefs::CODING_AGENT_CLAUDE_PATH.key(),
    crate::core::prefs::CODING_AGENT_CODEX_PATH.key(),
    crate::core::prefs::CODING_AGENT_CLAUDE_PERMISSION_MODE.key(),
    crate::core::prefs::OPENCODE_FREE_ENABLED.key(),
];

/// Decode one `application/x-www-form-urlencoded` component, as axum's `Query`
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
