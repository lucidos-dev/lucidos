//! Every mutating route is accounted for, checked against the router source.
//!
//! This is the structural half of ADR 0169's gate, and it replaces evidence the
//! e2e suites used to give. A run that produced no 401 only said the routes
//! exercised that day were fine. It said nothing about the next route, which is
//! the one that breaks: the gate is a LAYER, so a new endpoint inherits it
//! silently, and a caller that cannot present a credential finds out in
//! production.
//!
//! So the inventory below is a ratchet. A new mutating route fails this test
//! until its author picks a side. [`is_exempt`] carries the reason when the
//! caller cannot identify itself.

use super::*;
use std::collections::BTreeSet;

/// Mutating routes whose callers CAN identify themselves, so the layer refuses
/// an unidentified one.
///
/// The default. A route only leaves this list for [`is_exempt`], and only with
/// a reason recorded there.
const GATED_ROUTES: &[&str] = &[
    "/agent-allowed-commands",
    "/app",
    "/app-capture",
    "/app/:app_id/source",
    "/backup",
    "/backup/key",
    "/backup/key/reveal-token",
    "/backup/retention",
    "/backup/schedule",
    "/cc-allowed-tools",
    "/changes/:id/apply",
    "/changes/:id/discard",
    "/changes/:id/revert",
    "/changes/apply-all",
    "/changes/apply-all/cancel",
    "/changes/discard-all",
    "/chat/cancel",
    "/chat/queued-message/remove",
    "/claude-code/apply-now",
    "/claude-code/control",
    "/claude-code/discard",
    "/claude-code/interrupt",
    "/claude-code/stop",
    "/command-checkpoint/undo",
    "/command-permission/consent",
    "/credential-base-urls",
    "/credential-reveal-token",
    "/credentials",
    "/data/*path",
    "/data/edit",
    "/data/upload",
    "/device-presence",
    "/devices/:device_id",
    "/devices/:device_id/name",
    "/devices/:device_id/push",
    "/disk-usage/worktrees/:thread_id/cleanup",
    "/email/send",
    "/engine/rebuild",
    "/env-vars",
    "/events/emit",
    "/frontend-preview/start",
    "/frontend-preview/stop",
    "/handshake-scripts/approve",
    "/internal/approve-plan",
    "/internal/ask-user-question",
    "/internal/coding-agent-diff-refresh",
    "/internal/mark-hardened",
    "/internal/mark-planned",
    "/internal/permission-prompt",
    "/internal/restart-intent",
    "/internal/seed-change-for-test",
    "/mcp-allowed-tools",
    "/mcp-permission/consent",
    "/mcp/auto-approve",
    "/mcp/consent",
    "/mcp/servers/:id",
    "/mcp/servers/:id/disabled-tools",
    "/mcp/servers/:id/start",
    "/mcp/servers/:id/stop",
    "/memory/rebuild",
    "/models",
    "/network-config",
    "/notifications",
    "/notifications/read-all",
    "/oauth/accounts",
    "/oauth/complete",
    "/oauth/reauthorize",
    "/pinned-apps",
    "/plugins/catalog/rescan",
    "/plugins/install-request",
    "/plugins/install/:install_id/cancel",
    "/plugins/install/:install_id/confirm",
    "/plugins/marketplaces",
    "/plugins/marketplaces/:id",
    "/plugins/propose-upstream",
    "/plugins/uninstall-request",
    "/plugins/uninstall/:uninstall_id/cancel",
    "/plugins/uninstall/:uninstall_id/confirm",
    "/plugins/upload-archive",
    "/preferences",
    "/proxy-modules/reload",
    "/push/subscribe",
    "/push/unsubscribe",
    "/release-notices/resolve",
    "/repositories",
    "/repositories/:id",
    "/restart",
    "/standing-applies",
    "/standing-applies/:thread_id",
    "/thread-queue/drop",
    "/thread-queue/policy",
    "/thread-queue/run-now",
    "/threads",
    "/threads/:id",
    "/threads/:id/blobs",
    "/threads/:id/compose",
    "/threads/:thread_id/answer-question",
    "/threads/:thread_id/background-tasks",
    "/threads/:thread_id/background-tasks/:task_id/stop",
    "/threads/:thread_id/continue",
    "/threads/:thread_id/event-waits",
    "/threads/:thread_id/event-waits/:wait_id/cancel",
    "/threads/:thread_id/event-waits/cancel",
    "/threads/archive",
    // Gated twice over. This layer refuses a caller with no credential at all.
    // The handler then insists the one it did present is a registered device
    // (ADR 0192).
    "/threads/delete",
    "/threads/rename",
    "/threads/save",
    "/threads/suggest-title",
    "/threads/unsave",
    "/trigger-groups",
    "/trigger-groups/reorder",
    "/triggers",
    "/triggers/run",
    "/ui/navigate",
    "/webhooks",
    "/webhooks/:id",
];

/// Methods that write. Mirrors [`method_mutates`], which takes a `Method`.
const MUTATING: &[&str] = &["post", "put", "delete", "patch"];

/// Every mutating route the `api` modules register, read off the source.
///
/// The scan is shared with *route reach*, which asks a different question of
/// the same files (`api::route_scan`). This half keeps the writes.
///
/// `any(...)` counts: it answers every method, mutating ones included.
fn registered_mutating_routes() -> BTreeSet<String> {
    crate::api::route_scan::scan_api_routes()
        .into_iter()
        .filter(|hit| {
            hit.methods.contains("ANY")
                || MUTATING
                    .iter()
                    .any(|m| hit.methods.contains(&m.to_uppercase()))
        })
        .filter(|hit| !hit.path.is_empty())
        .map(|hit| hit.path)
        .collect()
}

/// The scanner has to actually find routes, or every assertion below passes
/// vacuously and the ratchet protects nothing.
#[test]
fn the_route_scanner_reads_the_real_router() {
    let routes = registered_mutating_routes();
    assert!(
        routes.len() > 100,
        "the scanner found only {} mutating routes, so it is not reading the router",
        routes.len()
    );
    for known in ["/changes/:id/apply", "/chat/stream", "/proxy/:name/*path"] {
        assert!(routes.contains(known), "the scanner missed {known}");
    }
}

/// Every mutating route is on exactly one side, and adding one is a decision.
///
/// This is what the e2e legs used to prove by not 401ing, and it holds for a
/// route no test exercises. A new endpoint fails here until its author says
/// whether its caller can identify itself.
#[test]
fn every_mutating_route_is_gated_or_deliberately_exempt() {
    let registered = registered_mutating_routes();
    let gated: BTreeSet<String> = GATED_ROUTES.iter().map(|s| s.to_string()).collect();

    let unaccounted: Vec<&String> = registered
        .iter()
        .filter(|r| !gated.contains(*r) && !is_exempt(r))
        .collect();
    assert!(
        unaccounted.is_empty(),
        "new mutating route(s) nobody has classified: {unaccounted:?}\n\
         Add each to GATED_ROUTES if its caller can present a credential, or to \
         `is_exempt` with the reason it cannot."
    );

    let vanished: Vec<&String> = gated.difference(&registered).collect();
    assert!(
        vanished.is_empty(),
        "GATED_ROUTES names route(s) the router no longer registers: {vanished:?}"
    );
}

/// An exemption that names nothing is one nobody can see is dead.
///
/// It fails LOUDLY on a rename, which is the case that matters. The route
/// carries on under its new name, silently gated, while the exemption sits
/// there still looking correct and its caller starts getting 401s.
#[test]
fn every_exemption_names_a_route_that_exists() {
    let registered = registered_mutating_routes();
    for route in EXEMPT_ROUTES {
        assert!(
            registered.contains(*route),
            "`is_exempt` names {route}, which no api module registers as mutating"
        );
    }
}

/// The two sides never overlap: a route is gated or exempt, never listed twice.
#[test]
fn no_route_is_both_gated_and_exempt() {
    for route in GATED_ROUTES {
        assert!(
            !is_exempt(route),
            "{route} is in GATED_ROUTES and also exempt"
        );
    }
}

#[test]
fn reads_and_the_preflight_pass_and_everything_else_writes() {
    for read in [Method::GET, Method::HEAD, Method::OPTIONS] {
        assert!(!method_mutates(&read), "{read} should read");
    }
    for write in [
        Method::POST,
        Method::PUT,
        Method::DELETE,
        Method::PATCH,
        Method::TRACE,
    ] {
        assert!(method_mutates(&write), "{write} should write");
    }
}

/// The whole exemption list, spelled the way axum actually hands it over.
///
/// This is the regression test for the shape that broke 215 e2e tests:
/// `MatchedPath` reports the FULL nested path, so an `is_exempt` written
/// against the module's own `/devices/register` matched nothing and the
/// device bootstrap 401'd itself. Drive the whole check, mount and all.
#[test]
fn exactly_the_four_classes_are_exempt_as_axum_reports_them() {
    let exempt = [
        "/chat/stream",
        "/threads/:thread_id/follow-up",
        "/devices/register",
        "/devices/hand-over",
        "/webhooks/:id/deliver",
        "/proxy/:name",
        "/proxy/:name/",
        "/proxy/:name/*path",
        "/internal/client-log",
        "/internal/client-logs",
        "/presence-pong",
        "/notification/read",
    ];
    for route in exempt {
        let matched = format!("{}{route}", crate::api::API_V1_PREFIX);
        assert!(
            is_exempt(mounted_route(&matched)),
            "{matched} should be exempt"
        );
    }
}

/// The mount comes off exactly once, so a route that merely repeats it is
/// a different route. Drives `mounted_route`, which is the half the
/// exemption test above shares and the gated test below does not.
#[test]
fn the_mount_is_stripped_once_and_only_from_the_front() {
    let doubled = format!("{p}{p}/devices/register", p = crate::api::API_V1_PREFIX);
    assert!(
        !is_exempt(mounted_route(&doubled)),
        "{doubled} is not the bootstrap route"
    );
    // A path with no mount at all is already the module's own spelling.
    assert!(is_exempt(mounted_route("/devices/register")));
}

/// The near-misses, which are the ones a prefix match would let through.
#[test]
fn a_route_that_merely_looks_exempt_is_gated() {
    for route in [
        // Lucidos verbs living under an exempt prefix.
        "/proxy-modules/reload",
        "/webhooks/:id",
        "/webhooks",
        "/devices/:device_id",
        "/devices/:device_id/name",
        "/chat/cancel",
        // Sibling of the exempt `/notification/read`, and the page's own
        // call, so it stays gated.
        "/notifications",
        "/notifications/read-all",
        "/threads/:thread_id/continue",
        // The build slot's own route, which is why the engine mints a token.
        "/events/emit",
        // Telemetry-shaped, but it emits DeviceVisible and its caller is
        // the page. It was taught the header rather than exempted.
        "/device-presence",
        // A sample of the ordinary sweep.
        "/changes/:id/apply",
        "/triggers",
        "/preferences",
    ] {
        assert!(!is_exempt(route), "{route} should be gated");
    }
}
