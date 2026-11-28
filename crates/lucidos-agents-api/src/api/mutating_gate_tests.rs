use super::*;
use std::collections::BTreeSet;

const GATED_ROUTES: &[&str] = &[
    "/workspace-operations",
    "/models/spend-cap-holds/:provider/clear",
    "/agent-allowed-commands",
    "/agent-denied-commands",
    "/app",
    "/app-capture",
    "/app/:app_id/source",
    "/app/opened",
    "/snapshots",
    "/snapshots/:handle",
    "/snapshots/:handle/check",
    "/snapshots/:handle/restore",
    "/snapshots/restore-caller",
    "/backup",
    "/backup/key/reveal-token",
    "/backup/recipient",
    "/backup/recipient/confirm",
    "/backup/recipient/setup",
    "/backup/recipient/verify",
    "/backup/retention",
    "/backup/schedule",
    "/browser/forget-login",
    "/cc-allowed-tools",
    "/changes/:id/apply",
    "/changes/:id/discard",
    "/changes/:id/set-aside",
    "/changes/:id/bring-back",
    "/changes/:id/revert",
    "/changes/:id/queue",
    "/changes/apply-all",
    "/changes/apply-all/cancel",
    "/changes/discard-all",
    "/changes/lane/cancel",
    "/chat/cancel",
    "/chat/queued-message/remove",
    "/chat/settings",
    "/claude-code/apply-now",
    "/claude-code/control",
    "/claude-code/discard",
    "/claude-code/interrupt",
    "/claude-code/stop",
    "/command-checkpoint/undo",
    "/command-permission/consent",
    "/config-changes/:event_id/undo",
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
    "/terminal-suggestions",
    "/disk-usage/cleanup",
    "/disk-usage/worktrees/:thread_id/cleanup",
    "/email/send",
    "/engine/host-log-transfer/prepare",
    "/engine/gateway-control",
    "/engine/host-log-transfer/:id/execute",
    "/engine/host-log-transfer/:id/status",
    "/engine/rebuild",
    "/env-vars",
    "/events/emit",
    "/follow-main/nudge",
    "/form-requests/:request_id/cancel",
    "/frontend-preview/start",
    "/frontend-preview/stop",
    "/handshake-scripts/approve",
    "/hooks/:slug",
    "/hooks/:slug/arm",
    "/hooks/:slug/disarm",
    "/hooks/pending/:message/cancel",
    "/intelligence-ladder/copy-from-chat",
    "/intelligence-ladder/default",
    "/intelligence-ladder/remove-model",
    "/intelligence-ladder/rungs",
    "/intelligence-ladder/sort",
    "/internal/approve-plan",
    "/internal/ask-user-question",
    "/internal/coding-agent-diff-refresh",
    "/internal/command-deny-check",
    "/internal/mark-hardened",
    "/internal/mark-planned",
    "/internal/permission-prompt",
    "/internal/request-app-capture-for-test",
    "/internal/restart-intent",
    "/internal/seed-change-for-test",
    "/internal/start-plan",
    "/themes/resolve",
    "/mcp-allowed-tools",
    "/main-check/close",
    "/main-check/fix-now",
    "/mcp-permission/consent",
    "/mcp/auto-approve",
    "/mcp/consent",
    "/mcp/servers/:id",
    "/mcp/servers/:id/disabled-tools",
    "/mcp/servers/:id/start",
    "/mcp/servers/:id/stop",
    "/memory/correct-by-id",
    "/memory/rebuild",
    "/memory/tree-backfill/cap",
    "/models",
    "/notification/native-receipt",
    "/notifications",
    "/notifications/read-all",
    "/oauth/accounts",
    "/oauth/complete",
    "/oauth/reauthorize",
    "/onepassword",
    "/onepassword/test",
    "/users/changed",
    "/operators/changed",
    "/pairing-requests/changed",
    "/permission-requests",
    "/permissions/:permission_id",
    "/permissions/batches",
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
    "/private-paths",
    "/providers/chatgpt-plan/sign-in",
    "/providers/chatgpt-plan/sign-out",
    "/providers/claude-gateway/sign-in",
    "/providers/claude-gateway/sign-out",
    "/proxy-modules/reload",
    "/push/subscribe",
    "/push/unsubscribe",
    "/release-notices/resolve",
    "/repositories",
    "/repositories/:id",
    "/restart",
    "/side-questions",
    "/side-questions/dismiss",
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
    "/threads/:thread_id/agent-pause",
    "/threads/:thread_id/archive",
    "/threads/:thread_id/background-tasks",
    "/threads/:thread_id/background-tasks/:task_id/stop",
    "/threads/:thread_id/hold",
    "/threads/:thread_id/terminal",
    "/threads/:thread_id/terminal/:terminal_id",
    "/threads/:thread_id/terminal/:terminal_id/ticket",
    "/threads/:thread_id/terminal/:terminal_id/type",
    "/threads/:thread_id/automation-pause", // INVARIANT: retired alias of /hold.
    "/threads/:thread_id/cache-keep-alive",
    "/threads/:thread_id/context-ceiling",
    "/threads/:thread_id/child-reports/take",
    "/threads/:thread_id/confinement-tests",
    "/threads/:thread_id/continue",
    "/threads/:thread_id/detach",
    "/threads/:thread_id/users/release",
    "/threads/:thread_id/users/take",
    "/threads/:thread_id/users/unwatch",
    "/threads/:thread_id/users/watch",
    "/threads/:thread_id/successor",
    "/threads/:thread_id/successor/withdraw",
    "/threads/:thread_id/successor-offer/decline",
    "/threads/:thread_id/successor-offer/continue",
    "/threads/:thread_id/handoff-checkpoints",
    "/threads/:thread_id/event-waits",
    "/threads/:thread_id/event-waits/:wait_id/cancel",
    "/threads/:thread_id/event-waits/cancel",
    "/threads/:thread_id/expected-wake",
    "/threads/:thread_id/harden",
    "/threads/:thread_id/mcp-servers/:id/disable",
    "/threads/:thread_id/mcp-servers/:id/enable",
    "/threads/:thread_id/read-request",
    "/threads/:thread_id/read-request/seen",
    "/threads/:thread_id/submit-draft",
    "/threads/:thread_id/wake-decision",
    "/threads/archive",
    "/threads/archive-all",
    "/threads/delete",
    "/threads/drafts",
    "/threads/rename",
    "/threads/save",
    "/threads/suggest-title",
    "/threads/unarchive",
    "/threads/unsave",
    "/threads/move-to-folder",
    "/trigger-groups",
    "/trigger-groups/reorder",
    "/triggers",
    "/triggers/run",
    "/ui/navigate",
    "/voice/ticket",
    "/webhooks",
    "/webhooks/:id",
    "/widgets/arrange",
    "/widgets/dashboard-add",
    "/widgets/dashboard-remove",
    "/widgets/fit",
    "/widgets/fit-session",
    "/widgets/fit-report",
    "/widgets/make-reusable",
    "/widgets/size",
    "/widgets/stop-reusing",
    "/workspaces",
    "/workspaces/trash/:id/restore",
    "/workspaces/:id/stop",
    "/workspaces/:id/start",
    "/workspaces/:id/autostart",
    "/workspaces/:id/rename",
    "/workspaces/:id/delete",
    "/workspaces/:id/device-session",
    "/workspaces/:id/restart",
];

const MUTATING: &[&str] = &["post", "put", "delete", "patch"];

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
        let matched = format!("{}{route}", crate::net_config::API_V1_PREFIX);
        assert!(
            is_exempt(mounted_route(&matched)),
            "{matched} should be exempt"
        );
    }
}

#[test]
fn the_mount_is_stripped_once_and_only_from_the_front() {
    let doubled = format!(
        "{p}{p}/devices/register",
        p = crate::net_config::API_V1_PREFIX
    );
    assert!(
        !is_exempt(mounted_route(&doubled)),
        "{doubled} is not the bootstrap route"
    );
    assert!(is_exempt(mounted_route("/devices/register")));
}

#[test]
fn a_route_that_merely_looks_exempt_is_gated() {
    for route in [
        "/proxy-modules/reload",
        "/webhooks/:id",
        "/webhooks",
        "/devices/:device_id",
        "/devices/:device_id/name",
        "/chat/cancel",
        "/notification/native-receipt",
        "/notifications",
        "/notifications/read-all",
        "/threads/:thread_id/continue",
        "/events/emit",
        "/device-presence",
        "/changes/:id/apply",
        "/triggers",
        "/preferences",
    ] {
        assert!(!is_exempt(route), "{route} should be gated");
    }
}

const READ_ONLY_SOCKET_ROUTES: &[&str] = &["/ws-echo"];

fn voice_uri(ticket: Option<&str>) -> axum::http::Uri {
    let ticket = ticket.map_or(String::new(), |t| {
        format!(
            "&{}={t}",
            crate::core::subprocess_origin::QUERY_SOCKET_TICKET
        )
    });
    format!(
        "{}/voice?thread_id={}{ticket}",
        crate::net_config::API_V1_PREFIX,
        uuid::Uuid::new_v4()
    )
    .parse()
    .expect("a valid URI")
}

fn matched_voice() -> String {
    format!("{}/voice", crate::net_config::API_V1_PREFIX)
}

#[tokio::test]
async fn an_unidentified_voice_upgrade_is_refused_though_it_is_a_get() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let tickets = crate::api::socket_ticket::SocketTickets::new();
    let nobody = axum::http::HeaderMap::new();

    for ticket in [None, Some("never-minted")] {
        let refusal = refuse_unidentified(
            &pool,
            &tickets,
            &Method::GET,
            Some(&matched_voice()),
            &nobody,
            &voice_uri(ticket),
        )
        .await
        .expect_err("an unidentified voice upgrade must be refused");
        assert_eq!(refusal.status, axum::http::StatusCode::UNAUTHORIZED);
    }

    let list = format!("{}/threads/list", crate::net_config::API_V1_PREFIX);
    refuse_unidentified(
        &pool,
        &tickets,
        &Method::GET,
        Some(&list),
        &nobody,
        &list.parse().expect("a valid URI"),
    )
    .await
    .expect("an ordinary read names nobody and passes");

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_voice_upgrade_is_named_by_its_ticket_but_never_over_the_header() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    crate::test_support::seed_device(&pool, "owners-phone", Some("Mozilla/5.0"), None).await;
    let tickets = crate::api::socket_ticket::SocketTickets::new();
    let thread_id = uuid::Uuid::new_v4();
    let ticket = tickets
        .mint(
            "owners-phone",
            crate::api::socket_ticket::SocketTarget::Call { thread_id },
            None,
        )
        .expect("a ticket");

    refuse_unidentified(
        &pool,
        &tickets,
        &Method::GET,
        Some(&matched_voice()),
        &axum::http::HeaderMap::new(),
        &voice_uri(Some(&ticket)),
    )
    .await
    .expect("a live ticket identifies the caller");
    assert!(
        tickets.peek_device(&ticket, None).is_some(),
        "the gate must not spend the ticket the handler redeems"
    );

    let mut forged = axum::http::HeaderMap::new();
    forged.insert(
        crate::core::subprocess_origin::HEADER_DEVICE_ID,
        "nobody-registered".parse().expect("a valid header value"),
    );
    let refusal = refuse_unidentified(
        &pool,
        &tickets,
        &Method::GET,
        Some(&matched_voice()),
        &forged,
        &voice_uri(Some(&ticket)),
    )
    .await
    .expect_err("the ticket must not stand in for the header's device");
    assert_eq!(refusal.status, axum::http::StatusCode::UNAUTHORIZED);

    crate::test_support::teardown_test_db(&db_name).await;
}

mod guard_tests {
    use super::*;

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

    #[test]
    fn every_socket_route_is_classified() {
        let api_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/api");
        let mut sockets = BTreeSet::new();
        for hit in crate::api::route_scan::scan_api_routes() {
            let src = std::fs::read_to_string(api_dir.join(&hit.file)).expect("source is readable");
            if src.contains("WebSocketUpgrade") {
                sockets.insert(hit.path);
            }
        }
        assert!(
            sockets.contains("/voice"),
            "the scan found no voice socket, so it protects nothing: {sockets:?}"
        );
        for route in &sockets {
            let writes = MUTATING_SOCKET_ROUTES.contains(&route.as_str());
            let reads = READ_ONLY_SOCKET_ROUTES.contains(&route.as_str());
            assert!(
                writes != reads,
                "{route} takes a socket upgrade. List it in MUTATING_SOCKET_ROUTES if the \
             caller writes through it, or in READ_ONLY_SOCKET_ROUTES if it reaches nothing."
            );
        }
    }
}
