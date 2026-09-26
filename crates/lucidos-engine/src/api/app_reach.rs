//! Whether an app frame may reach a route, declared route by route.
//!
//! An app frame runs at an opaque origin (ADR 0227), so the host bridge is its
//! only way to the engine. `lucidos.request` carries anything the SDK does not
//! name, and that hatch needs an answer per route (ADR 0231).
//!
//! **Absence is denial.** A route missing from [`ROUTE_REACH`] is refused, and
//! the completeness test below fails the build. So the question is answered
//! when the route is written, rather than when a workspace finds out.

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
use Reach::{Agent, App, Asset, Host};

/// Who may reach a route. See the table in this module's doc.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reach {
    App,
    Asset,
    Host,
    Agent,
}

/// Every route the `/api/v1` router serves, with its answer and, for an `App`
/// route, the methods an app frame may use.
///
/// Paths are spelled exactly as the module routers write them, which is also
/// what `MatchedPath` reports once the mount is stripped.
pub const ROUTE_REACH: &[(&str, Reach, &[&str])] = &[
    ("/_test/push-log", Host, &[]),
    ("/agent-allowed-commands", Host, &[]),
    ("/app", App, &["GET"]),
    ("/app-capture", Host, &[]),
    ("/app-frame-capability", Host, &[]),
    ("/app/:app_id/source", Host, &[]),
    ("/apps", App, &["GET"]),
    ("/backup", Host, &[]),
    ("/backup/key", Host, &[]),
    ("/backup/key/exists", Host, &[]),
    ("/backup/key/reveal-token", Host, &[]),
    ("/backup/last-successful", Host, &[]),
    ("/backup/list", Host, &[]),
    ("/backup/providers", Host, &[]),
    ("/backup/retention", Host, &[]),
    ("/backup/schedule", Host, &[]),
    ("/backup/status", Host, &[]),
    ("/blobs/:hash", Host, &[]),
    ("/blobs/:hash/preview", Host, &[]),
    ("/browse-directories", Host, &[]),
    ("/cc-allowed-tools", Host, &[]),
    ("/changes", Host, &[]),
    ("/changes/:id", Host, &[]),
    ("/changes/:id/apply", Host, &[]),
    ("/changes/:id/diff", Host, &[]),
    ("/changes/:id/discard", Host, &[]),
    ("/changes/:id/file", Host, &[]),
    ("/changes/:id/revert", Host, &[]),
    ("/changes/applied", Host, &[]),
    ("/changes/apply-all", Host, &[]),
    ("/changes/apply-all/cancel", Host, &[]),
    ("/changes/discard-all", Host, &[]),
    ("/changes/for-repo/:repo_id", Host, &[]),
    ("/chat/cancel", Host, &[]),
    ("/chat/queued-message/remove", Host, &[]),
    ("/chat/stream", Host, &[]),
    ("/claude-code/apply-now", Host, &[]),
    ("/claude-code/commands", Host, &[]),
    ("/claude-code/control", Host, &[]),
    ("/claude-code/discard", Host, &[]),
    ("/claude-code/interrupt", Host, &[]),
    ("/claude-code/stop", Host, &[]),
    ("/coding-agents/binaries", Host, &[]),
    ("/command-checkpoint/diff", Host, &[]),
    ("/command-checkpoint/undo", Host, &[]),
    ("/command-permission/consent", Host, &[]),
    ("/commits", Host, &[]),
    ("/commits/before", Host, &[]),
    ("/credential-base-urls", Host, &[]),
    ("/credential-reveal-token", Host, &[]),
    ("/credential-value", Host, &[]),
    ("/credentials", Host, &[]),
    ("/data", App, &["GET"]),
    ("/data/*path", App, &["GET", "PUT", "DELETE"]),
    ("/data/edit", App, &["POST"]),
    ("/data/upload", App, &["POST"]),
    ("/device-presence", Host, &[]),
    ("/devices", Host, &[]),
    ("/devices/:device_id", Host, &[]),
    ("/devices/:device_id/name", Host, &[]),
    ("/devices/:device_id/push", Host, &[]),
    ("/devices/hand-over", Host, &[]),
    ("/devices/register", Host, &[]),
    ("/disk-usage/summary", Host, &[]),
    ("/disk-usage/worktrees", Host, &[]),
    ("/disk-usage/worktrees/:thread_id/cleanup", Host, &[]),
    ("/email-account", Host, &[]),
    ("/email/send", Host, &[]),
    ("/engine/changelog", Host, &[]),
    ("/engine/rebuild", Host, &[]),
    ("/engine/version-status", Host, &[]),
    // Read only. Every `run_bash` and `run_python` the agent runs gets the
    // user env vars (`build_script_env_vars`). The reserved list guards the
    // names the engine owns, not the ones an interpreter executes. So a
    // writable `/env-vars` turns app authority into host code execution:
    // `PYTHONPATH` beside a file the app wrote through `/data/*path` runs on
    // the next Python tool call, with `CRED_*` and `OAUTH_*` in scope. The
    // read is what ADR 0231 opened, and it stays.
    ("/env-vars", App, &["GET"]),
    ("/events", Host, &[]),
    ("/events/:event_id/context", Host, &[]),
    ("/events/:event_id/location", Host, &[]),
    ("/events/:event_id/tool-args", Host, &[]),
    ("/events/:event_id/tool-result", Host, &[]),
    ("/events/count", App, &["GET"]),
    ("/events/emit", App, &["POST"]),
    ("/events/query", App, &["GET"]),
    ("/events/types", App, &["GET"]),
    ("/fonts/fira-code.css", Asset, &[]),
    ("/form-requests/:request_id/cancel", Host, &[]),
    ("/form-requests/pending", Host, &[]),
    ("/frontend-preview", Host, &[]),
    ("/frontend-preview/start", Host, &[]),
    ("/frontend-preview/stop", Host, &[]),
    ("/handshake-scripts", Host, &[]),
    ("/handshake-scripts/approve", Host, &[]),
    ("/health", App, &["GET"]),
    ("/history", Host, &[]),
    ("/internal/approve-plan", Agent, &[]),
    ("/internal/ask-user-question", Agent, &[]),
    ("/internal/client-log", Agent, &[]),
    ("/internal/client-logs", Agent, &[]),
    ("/internal/coding-agent-diff-refresh", Agent, &[]),
    ("/internal/hardened-state", Agent, &[]),
    ("/internal/mark-hardened", Agent, &[]),
    ("/internal/mark-planned", Agent, &[]),
    ("/internal/permission-prompt", Agent, &[]),
    ("/internal/planned-state", Agent, &[]),
    ("/internal/restart-intent", Agent, &[]),
    ("/internal/seed-change-for-test", Agent, &[]),
    ("/knowhow", App, &["GET"]),
    ("/knowhow/read", App, &["GET"]),
    ("/mcp-allowed-tools", Host, &[]),
    ("/mcp-permission/consent", Host, &[]),
    ("/mcp/auto-approve", Host, &[]),
    ("/mcp/consent", Host, &[]),
    ("/mcp/servers", Host, &[]),
    ("/mcp/servers/:id", Host, &[]),
    ("/mcp/servers/:id/disabled-tools", Host, &[]),
    ("/mcp/servers/:id/start", Host, &[]),
    ("/mcp/servers/:id/stop", Host, &[]),
    ("/memory/embedding-model-status", Host, &[]),
    ("/memory/entries", Host, &[]),
    ("/memory/rebuild", Host, &[]),
    ("/memory/search", Host, &[]),
    ("/memory/source", Host, &[]),
    ("/memory/stats", Host, &[]),
    ("/messages", Host, &[]),
    ("/models", App, &["GET"]),
    ("/network-config", Host, &[]),
    ("/notification", App, &["GET"]),
    ("/notification/read", App, &["POST"]),
    ("/notifications", App, &["GET", "POST"]),
    ("/notifications/before", App, &["GET"]),
    ("/notifications/read-all", App, &["POST"]),
    ("/oauth/:provider/access-token", App, &["GET"]),
    ("/oauth/accounts", Host, &[]),
    ("/oauth/complete", Host, &[]),
    ("/oauth/known-providers", Host, &[]),
    ("/oauth/reauthorize", Host, &[]),
    ("/pinned-apps", Host, &[]),
    ("/plugins/catalog", Host, &[]),
    ("/plugins/catalog/rescan", Host, &[]),
    ("/plugins/install-request", Host, &[]),
    ("/plugins/install/:install_id/cancel", Host, &[]),
    ("/plugins/install/:install_id/confirm", Host, &[]),
    ("/plugins/installed", Host, &[]),
    ("/plugins/marketplaces", Host, &[]),
    ("/plugins/marketplaces/:id", Host, &[]),
    ("/plugins/propose-upstream", Host, &[]),
    ("/plugins/uninstall-request", Host, &[]),
    ("/plugins/uninstall/:uninstall_id/cancel", Host, &[]),
    ("/plugins/uninstall/:uninstall_id/confirm", Host, &[]),
    ("/plugins/upload-archive", Host, &[]),
    ("/preferences", App, &["GET", "PUT"]),
    ("/presence-pong", Host, &[]),
    ("/proxy-modules/reload", Host, &[]),
    (
        "/proxy/:name",
        App,
        &["GET", "POST", "PUT", "DELETE", "PATCH"],
    ),
    (
        "/proxy/:name/",
        App,
        &["GET", "POST", "PUT", "DELETE", "PATCH"],
    ),
    (
        "/proxy/:name/*path",
        App,
        &["GET", "POST", "PUT", "DELETE", "PATCH"],
    ),
    ("/push/subscribe", Host, &[]),
    ("/push/unsubscribe", Host, &[]),
    ("/push/vapid-key", Host, &[]),
    ("/release-notices", Host, &[]),
    ("/release-notices/resolve", Host, &[]),
    ("/repositories", Host, &[]),
    ("/repositories/:id", Host, &[]),
    ("/repositories/:id/diff", Host, &[]),
    ("/repositories/:id/file", Host, &[]),
    ("/repositories/:id/files", Host, &[]),
    ("/response-styles", Host, &[]),
    ("/restart", Host, &[]),
    ("/sdk-iframe-audio.js", Asset, &[]),
    ("/sdk-iframe.css", Asset, &[]),
    ("/sdk-prefs.js", Asset, &[]),
    ("/sdk.js", Asset, &[]),
    ("/search", Host, &[]),
    ("/session/messages", Host, &[]),
    ("/sse-worker.js", Asset, &[]),
    ("/standing-applies", Host, &[]),
    ("/standing-applies/:thread_id", Host, &[]),
    ("/static/html2canvas.min.js", Asset, &[]),
    ("/tailnet-status", Host, &[]),
    ("/thread-queue", Host, &[]),
    ("/thread-queue/drop", Host, &[]),
    ("/thread-queue/policy", Host, &[]),
    ("/thread-queue/run-now", Host, &[]),
    ("/threads", Host, &[]),
    ("/threads/:id", Host, &[]),
    ("/threads/:id/blobs", Host, &[]),
    ("/threads/:id/compose", Host, &[]),
    ("/threads/:thread_id/answer-question", Host, &[]),
    ("/threads/:thread_id/background-tasks", Agent, &[]),
    ("/threads/:thread_id/background-tasks/:task_id", Agent, &[]),
    (
        "/threads/:thread_id/background-tasks/:task_id/stop",
        Agent,
        &[],
    ),
    ("/threads/:thread_id/cc-diff", Host, &[]),
    ("/threads/:thread_id/continue", Host, &[]),
    ("/threads/:thread_id/detach", Host, &[]),
    ("/threads/:thread_id/event-waits", Agent, &[]),
    (
        "/threads/:thread_id/event-waits/:wait_id/cancel",
        Agent,
        &[],
    ),
    ("/threads/:thread_id/event-waits/cancel", Agent, &[]),
    ("/threads/:thread_id/events", Host, &[]),
    ("/threads/:thread_id/follow-up", Host, &[]),
    ("/threads/:thread_id/images", Host, &[]),
    ("/threads/:thread_id/images/:index", Host, &[]),
    ("/threads/:thread_id/messages", Host, &[]),
    ("/threads/archive", Host, &[]),
    ("/threads/archived-count", Host, &[]),
    ("/threads/count", App, &["GET"]),
    ("/threads/delete", Host, &[]),
    ("/threads/delete-preflight", Host, &[]),
    ("/threads/filter-facets", Host, &[]),
    ("/threads/list", App, &["GET"]),
    ("/threads/older", Host, &[]),
    ("/threads/rename", Host, &[]),
    ("/threads/save", Host, &[]),
    ("/threads/search", Host, &[]),
    ("/threads/suggest-title", Host, &[]),
    ("/threads/unsave", Host, &[]),
    ("/trigger-groups", App, &["GET"]),
    ("/trigger-groups/reorder", Host, &[]),
    ("/triggers", App, &["GET", "POST", "PUT", "DELETE"]),
    ("/triggers/historical", App, &["GET"]),
    ("/triggers/run", App, &["POST"]),
    ("/ui/navigate", App, &["POST"]),
    ("/voice", Host, &[]),
    ("/webhooks", Host, &[]),
    ("/webhooks/:id", Host, &[]),
    ("/webhooks/:id/deliver", Host, &[]),
    ("/webhooks/ingress", Host, &[]),
    ("/webhooks/refusals", Host, &[]),
    ("/workspace-label", Host, &[]),
    ("/workspaces", Host, &[]),
    ("/ws-echo", Host, &[]),
];

/// The classification for a route pattern, or `None` when the table is silent.
///
/// `None` means denied. It is distinct from `Some(Host)` only for the caller
/// that wants to tell a missing answer from a deliberate one.
pub fn entry_for(route: &str) -> Option<(Reach, &'static [&'static str])> {
    ROUTE_REACH
        .iter()
        .find(|(path, _, _)| *path == route)
        .map(|(_, reach, methods)| (*reach, *methods))
}

/// May an app frame call this method on this route pattern?
///
/// Default deny: an unknown route, a non-`App` route, and a method the row does
/// not list all answer false.
pub fn app_may_call(route: &str, method: &str) -> bool {
    match entry_for(route) {
        Some((App, methods)) => methods.iter().any(|m| m.eq_ignore_ascii_case(method)),
        _ => false,
    }
}

/// The header the host bridge stamps with the calling app's id.
///
/// The host sets it after dropping every `x-lucidos-*` the app supplied, so in
/// the shell an app cannot name another app. In a standalone app tab there is
/// no host and no stamp, and the document is same-origin anyway.
pub const APP_ID_HEADER: &str = "x-lucidos-app-id";

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
    // Lossy, never `to_str`: an app id outside ASCII arrives as raw bytes, and
    // a stamp that failed to decode must not read as no stamp at all.
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
        .strip_prefix(super::API_V1_PREFIX)
        .unwrap_or(&route)
        .to_string();
    if !app_may_call(&mounted, request.method().as_str()) {
        crate::log!(
            "[API] Refusing {} {} for app '{}': not app-reachable (ADR 0231)",
            request.method(),
            mounted,
            app_id
        );
        return ApiError::new(
            StatusCode::FORBIDDEN,
            format!(
                "An app may not call {} {}. See system-knowhow/js-sdk.md, lucidos.request.",
                request.method(),
                mounted
            ),
        )
        .into_response();
    }
    if let Some(key) = human_only_preference(&mounted, request.method(), request.uri().query()) {
        crate::log!(
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

/// The human-only key a `PUT /preferences` names, if any.
///
/// `/preferences` is an `App` route, so an app can keep its own settings. The
/// keys the agent may not write (`preference_catalog::INTERNAL_KEYS`) include
/// the security switches: the command guard, the tool-call cap and the network
/// bind. An app is no more trusted than the agent, so it may not write them
/// either. Every `key` parameter counts, so a repeated one cannot hide a key.
///
/// [`APP_REFUSED_PREFERENCES`] adds keys the agent may write but an app may not.
/// The engine's own bookkeeping (`preference_catalog::SILENT_PREF_KEYS`) is no
/// setting at all. An app overwriting `vapid_keys` stops every push.
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
            crate::core::preference_catalog::internal_hint(key).is_some()
                || crate::core::preference_catalog::is_silent_key(key)
                || APP_REFUSED_PREFERENCES.contains(&key.as_str())
        })
}

/// Preferences that choose what a coding-agent session spawns and what it may
/// do unasked. An app writes files under its own folder, which is the app
/// thread's working directory. So a path pointed at `/bin/sh` runs the app's
/// own script as the user when the next session starts.
///
/// `local_base_url` chooses where local-model chat is sent. An app pointing it
/// at its own host would receive every prompt and the conversation behind it.
const APP_REFUSED_PREFERENCES: &[&str] = &[
    crate::core::PREF_CODING_AGENT_CLAUDE_PATH,
    crate::core::PREF_CODING_AGENT_CODEX_PATH,
    crate::core::PREF_CODING_AGENT_CLAUDE_PERMISSION_MODE,
    crate::core::PREF_LOCAL_BASE_URL,
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
