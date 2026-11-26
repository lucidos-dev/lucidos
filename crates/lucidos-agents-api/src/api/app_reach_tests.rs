use super::*;
use crate::app_reach_policy::{entry_for, Reach, Reach::App, ROUTE_REACH};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const MOUNTED_ELSEWHERE: &[(&str, &str, &str)] = &[
    (
        "internal_faults.rs",
        "internal_faults_middleware_records_only_our_response_and_preserves_bodies",
        "an inline test's own router",
    ),
    (
        "restore_generation.rs",
        "durable_boundary_fences_before_handler_without_waiting_for_hello",
        "an inline test's own router",
    ),
    (
        "apps.rs",
        "ui_router",
        "nested at /app, outside /api/v1, so the hatch cannot address it",
    ),
    (
        "frame_file_cors.rs",
        "answer",
        "an inline test's own router",
    ),
    ("fonts.rs", "answer", "an inline test's own router"),
    (
        "snapshots_restore.rs",
        "delivery_is_refused_unless_the_post_left_unanswered",
        "an inline test's own router",
    ),
    (
        "local_auth.rs",
        "an_open_door_streams_a_body_instead_of_collecting_it",
        "an inline test's own router",
    ),
    (
        "settings.rs",
        "stand_in_app",
        "an inline test's stand-in gateway",
    ),
];

const COMPUTED_ROUTE_REACH: &[(&str, &str, Reach)] = &[
    ("internal.rs", "router", Reach::Agent),
    ("workspace_calls.rs", "router", Reach::Agent),
];

#[test]
fn an_unclassified_route_is_denied() {
    assert_eq!(entry_for("/not-a-route"), None);
    assert!(!app_may_call("/not-a-route", "GET"));
    assert!(!app_may_call("/credential-value", "GET"));
    assert!(app_may_call("/models", "GET"));
    assert!(!app_may_call("/models", "DELETE"));
}

#[test]
fn host_log_transfer_routes_are_agent_only() {
    for route in [
        "/engine/host-log-transfer/prepare",
        "/engine/gateway-control",
        "/engine/host-log-transfer/:id/execute",
        "/engine/host-log-transfer/:id/status",
    ] {
        assert_eq!(entry_for(route), Some((Reach::Agent, &[][..])));
        assert!(!app_may_call(route, "POST"));
    }
}

#[test]
fn a_denied_route_lists_no_methods() {
    for (path, reach, methods) in ROUTE_REACH {
        if *reach != App {
            assert!(
                methods.is_empty(),
                "{path} is not app-reachable, so it must list no methods"
            );
        } else {
            assert!(
                !methods.is_empty(),
                "{path} is app-reachable with no method"
            );
        }
    }
}

#[test]
fn the_deliberate_omissions_stay_denied() {
    let denied: &[(&str, &str)] = &[
        ("/chat/stream", "POST"),
        ("/threads", "POST"),
        ("/threads/:thread_id/continue", "POST"),
        ("/threads/:thread_id/follow-up", "POST"),
        ("/threads/:id/compose", "PUT"),
        ("/threads/:thread_id/answer-question", "POST"),
        ("/internal/ask-user-question", "POST"),
        ("/internal/approve-plan", "POST"),
        ("/internal/permission-prompt", "POST"),
        ("/command-permission/consent", "POST"),
        ("/mcp/consent", "POST"),
        ("/mcp/auto-approve", "PUT"),
        ("/command-checkpoint/undo", "POST"),
        ("/credential-value", "GET"),
        ("/credential-reveal-token", "POST"),
        ("/voice/ticket", "POST"),
        ("/credential-base-urls", "PUT"),
        ("/credentials", "GET"),
        ("/private-paths", "GET"),
        ("/private-paths", "PUT"),
        ("/backup/key", "GET"),
        ("/email/send", "POST"),
        ("/email-account", "GET"),
        ("/oauth/accounts", "GET"),
        ("/oauth/complete", "POST"),
        ("/oauth/reauthorize", "POST"),
        ("/messages", "GET"),
        ("/history", "GET"),
        ("/threads/:thread_id/messages", "GET"),
        ("/threads/:thread_id/events", "GET"),
        ("/session/messages", "GET"),
        ("/search", "GET"),
        ("/memory/entries", "GET"),
        ("/memory/search", "GET"),
        ("/blobs/:hash", "GET"),
        ("/events/:event_id/tool-args", "GET"),
        ("/events/:event_id/tool-result", "GET"),
        ("/devices", "GET"),
        ("/devices/register", "POST"),
        ("/devices/hand-over", "POST"),
        ("/devices/caller", "GET"),
        ("/device-presence", "POST"),
        ("/push/subscribe", "POST"),
        ("/changes/:id/apply", "POST"),
        ("/changes/apply-all", "POST"),
        ("/claude-code/control", "POST"),
        ("/engine/rebuild", "POST"),
        ("/engine/build-log", "GET"),
        ("/restart", "POST"),
        ("/plugins/install-request", "POST"),
        ("/plugins/uninstall-request", "POST"),
        ("/thread-queue/policy", "PUT"),
        ("/proxy-modules/reload", "POST"),
        ("/handshake-scripts/approve", "POST"),
        ("/browse-directories", "GET"),
        ("/repositories", "GET"),
        ("/repositories/:id/file", "GET"),
        ("/workspaces", "GET"),
        ("/webhooks", "GET"),
        ("/app/:app_id/source", "GET"),
        ("/app-capture", "POST"),
        ("/widgets/fit-report", "POST"),
    ];
    for (path, method) in denied {
        assert!(
            !app_may_call(path, method),
            "{method} {path} is app-reachable, and ADR 0231 says it must not be"
        );
    }
}

#[test]
fn an_app_cannot_write_an_env_var_the_agent_will_run_under() {
    assert!(app_may_call("/env-vars", "GET"));
    for method in ["POST", "PUT", "DELETE"] {
        assert!(
            !app_may_call("/env-vars", method),
            "{method} /env-vars is app-reachable. A user env var reaches every \
             run_bash and run_python, so an app writing one gets host code \
             execution through a loader hook such as PYTHONPATH."
        );
    }
}

#[test]
fn the_shipped_sdk_surface_stays_reachable() {
    let allowed: &[(&str, &str)] = &[
        ("/data", "GET"),
        ("/data/*path", "GET"),
        ("/data/*path", "PUT"),
        ("/data/*path", "DELETE"),
        ("/data/edit", "POST"),
        ("/data/upload", "POST"),
        ("/events/query", "GET"),
        ("/events/emit", "POST"),
        ("/triggers", "GET"),
        ("/triggers", "POST"),
        ("/triggers", "PUT"),
        ("/triggers", "DELETE"),
        ("/triggers/run", "POST"),
        ("/preferences", "GET"),
        ("/preferences", "PUT"),
        ("/notifications", "GET"),
        ("/notification/read", "POST"),
        ("/notifications/read-all", "POST"),
        ("/apps", "GET"),
        ("/app", "GET"),
        ("/threads/list", "GET"),
        ("/threads/count", "GET"),
        ("/oauth/:provider/access-token", "GET"),
        ("/proxy/:name/*path", "POST"),
        ("/ui/navigate", "POST"),
    ];
    for (path, method) in allowed {
        assert!(
            app_may_call(path, method),
            "{method} {path} is called by a shipped SDK namespace and the table refuses it"
        );
    }
}

fn gated_router() -> axum::Router {
    let inner = axum::Router::new()
        .route("/env-vars", axum::routing::get(|| async { "ok" }))
        .route(
            "/models",
            axum::routing::get(|| async { "ok" }).delete(|| async { "ok" }),
        )
        .route("/credentials", axum::routing::get(|| async { "ok" }))
        .route(
            "/threads/:thread_id/messages",
            axum::routing::get(|| async { "ok" }),
        )
        .route("/preferences", axum::routing::put(|| async { "ok" }))
        .layer(axum::middleware::from_fn(super::enforce_app_reach));
    axum::Router::new().nest(crate::net_config::API_V1_PREFIX, inner)
}

async fn status_of(method: &str, path: &str, app: Option<&str>) -> axum::http::StatusCode {
    use tower::ServiceExt as _;
    let mut builder = axum::http::Request::builder().method(method).uri(path);
    if let Some(id) = app {
        builder = builder.header(APP_ID_HEADER, id);
    }
    gated_router()
        .oneshot(builder.body(axum::body::Body::empty()).unwrap())
        .await
        .expect("the router answers")
        .status()
}

#[tokio::test]
async fn a_stamped_call_to_a_denied_route_is_refused() {
    assert_eq!(
        status_of("GET", "/api/v1/credentials", Some("habit-tracker")).await,
        axum::http::StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_of("GET", "/api/v1/threads/abc/messages", Some("habit-tracker")).await,
        axum::http::StatusCode::FORBIDDEN,
        "the mount is stripped before the table is asked, and a param matches"
    );
}

#[tokio::test]
async fn an_unstamped_call_is_untouched() {
    assert_eq!(
        status_of("GET", "/api/v1/credentials", None).await,
        axum::http::StatusCode::OK
    );
}

#[tokio::test]
async fn a_stamped_call_is_refused_per_method() {
    assert_eq!(
        status_of("GET", "/api/v1/models", Some("habit-tracker")).await,
        axum::http::StatusCode::OK
    );
    assert_eq!(
        status_of("DELETE", "/api/v1/models", Some("habit-tracker")).await,
        axum::http::StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_of("GET", "/api/v1/env-vars", Some("habit-tracker")).await,
        axum::http::StatusCode::OK
    );
}

#[tokio::test]
async fn a_stamped_write_of_a_human_only_preference_is_refused() {
    use axum::http::StatusCode;
    for query in [
        "key=command_guard",
        "key=network_bind",
        "key=max_tool_calls",
        "key=provider_enabled_vertex",
        "key=command%5Fguard",
        "key=theme&key=command_guard",
    ] {
        assert_eq!(
            status_of(
                "PUT",
                &format!("/api/v1/preferences?{query}"),
                Some("habit-tracker")
            )
            .await,
            StatusCode::FORBIDDEN,
            "an app wrote the human-only preference in `{query}`"
        );
    }
}

#[tokio::test]
async fn a_stamped_write_of_a_coding_agent_spawn_preference_is_refused() {
    use axum::http::StatusCode;
    for query in [
        "key=coding_agent_codex_path",
        "key=coding_agent_claude_path",
        "key=coding_agent_claude_permission_mode",
        "key=coding%5Fagent%5Fcodex%5Fpath",
        "key=theme&key=coding_agent_codex_path",
    ] {
        assert_eq!(
            status_of(
                "PUT",
                &format!("/api/v1/preferences?{query}"),
                Some("habit-tracker")
            )
            .await,
            StatusCode::FORBIDDEN,
            "an app wrote the coding-agent preference in `{query}`"
        );
    }
    assert_eq!(
        status_of(
            "PUT",
            "/api/v1/preferences?key=coding_agent_codex_path",
            None
        )
        .await,
        StatusCode::OK,
        "Settings writes the coding-agent paths through this same route"
    );
}

#[tokio::test]
async fn a_stamped_write_of_the_opencode_free_switch_is_refused() {
    use axum::http::StatusCode;
    for query in [
        "key=opencode_free_enabled",
        "key=opencode%5Ffree%5Fenabled",
        "key=theme&key=opencode_free_enabled",
    ] {
        assert_eq!(
            status_of(
                "PUT",
                &format!("/api/v1/preferences?{query}"),
                Some("habit-tracker")
            )
            .await,
            StatusCode::FORBIDDEN,
            "an app switched on the OpenCode Free relay in `{query}`"
        );
    }
    assert_eq!(
        status_of("PUT", "/api/v1/preferences?key=opencode_free_enabled", None).await,
        StatusCode::OK,
        "Settings writes the OpenCode Free switch through this same route"
    );
}

#[tokio::test]
async fn a_stamped_write_of_the_local_model_host_is_refused() {
    use axum::http::StatusCode;
    assert_eq!(
        status_of(
            "PUT",
            "/api/v1/preferences?key=local_base_url",
            Some("habit-tracker")
        )
        .await,
        StatusCode::FORBIDDEN,
        "an app redirected local-model chat"
    );
    assert_eq!(
        status_of("PUT", "/api/v1/preferences?key=local_base_url", None).await,
        StatusCode::OK,
        "Settings writes the local host through this same route"
    );
}

#[tokio::test]
async fn a_stamped_write_of_engine_bookkeeping_is_refused() {
    use axum::http::StatusCode;
    for query in [
        "key=backup_last_run",
        "key=backfill_repo_names_from_changes_done",
        "key=theme&key=backup_last_run",
    ] {
        assert_eq!(
            status_of(
                "PUT",
                &format!("/api/v1/preferences?{query}"),
                Some("habit-tracker")
            )
            .await,
            StatusCode::FORBIDDEN,
            "an app wrote the engine's own state in `{query}`"
        );
    }
}

#[tokio::test]
async fn a_stamp_that_is_not_ascii_is_still_refused() {
    use tower::ServiceExt as _;
    let request = axum::http::Request::builder()
        .method("PUT")
        .uri("/api/v1/preferences?key=command_guard")
        .header(
            APP_ID_HEADER,
            axum::http::HeaderValue::from_bytes(b"caf\xe9").unwrap(),
        )
        .body(axum::body::Body::empty())
        .unwrap();
    let status = gated_router()
        .oneshot(request)
        .await
        .expect("the router answers")
        .status();
    assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_app_still_writes_an_ordinary_preference_and_the_shell_writes_any() {
    use axum::http::StatusCode;
    assert_eq!(
        status_of(
            "PUT",
            "/api/v1/preferences?key=theme",
            Some("habit-tracker")
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        status_of("PUT", "/api/v1/preferences?key=command_guard", None).await,
        StatusCode::OK,
        "Settings writes the human-only keys through this same route"
    );
}

#[test]
fn form_decode_agrees_with_axum_query() {
    for raw in [
        "a+b",
        "a%20b",
        "a%2Bb",
        "%E2%9C%93",
        "command%5Fguard",
        "x%3Dy%26z",
    ] {
        let uri: axum::http::Uri = format!("/p?k={raw}").parse().unwrap();
        let axum::extract::Query(pairs) =
            axum::extract::Query::<Vec<(String, String)>>::try_from_uri(&uri).unwrap();
        assert_eq!(pairs[0].1, form_decode(raw), "{raw}");
    }
}

#[tokio::test]
async fn an_unmatched_route_answers_404_rather_than_a_refusal() {
    assert_eq!(
        status_of("GET", "/api/v1/no-such-route", Some("habit-tracker")).await,
        axum::http::StatusCode::NOT_FOUND
    );
}

#[test]
fn the_routes_that_motivated_the_hatch_are_open() {
    assert!(app_may_call("/env-vars", "GET"));
    assert!(app_may_call("/models", "GET"));
    assert!(app_may_call("/notifications", "POST"));
}

mod guard_tests {
    use super::*;

    fn api_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/api")
    }

    fn scan_routes() -> BTreeMap<String, BTreeSet<String>> {
        let mod_src =
            std::fs::read_to_string(api_dir().join("mod.rs")).expect("api/mod.rs is readable");
        let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for hit in crate::api::route_scan::scan_api_routes() {
            let elsewhere = MOUNTED_ELSEWHERE
                .iter()
                .any(|(file, func, _)| *file == hit.file && *func == hit.func);
            if elsewhere {
                continue;
            }
            let module = hit.file.trim_end_matches(".rs").trim_end_matches("/mod");
            assert!(
                hit.func == "router" && mod_src.contains(&format!(".merge({module}::router())")),
                "{}::{} registers a route and nothing says where it is mounted. Merge it \
             into create_router, or declare it in MOUNTED_ELSEWHERE.",
                hit.file,
                hit.func
            );
            if hit.path.is_empty() {
                let declared = COMPUTED_ROUTE_REACH
                    .iter()
                    .any(|(file, func, _)| *file == hit.file && *func == hit.func);
                assert!(
                    declared,
                    "{} registers a computed route. Add it to COMPUTED_ROUTE_REACH \
                 with its answer.",
                    hit.file
                );
                continue;
            }
            found.entry(hit.path).or_default().extend(hit.methods);
        }
        found
    }

    #[test]
    fn every_api_route_states_whether_an_app_may_reach_it() {
        let scanned = scan_routes();
        assert!(
            scanned.len() > 150 && scanned.contains_key("/data/*path"),
            "the route scan found {} routes, so it is not reading the routers",
            scanned.len()
        );

        let declared: BTreeSet<&str> = ROUTE_REACH.iter().map(|(p, _, _)| *p).collect();
        let unanswered: Vec<&String> = scanned
            .keys()
            .filter(|p| !declared.contains(p.as_str()))
            .collect();
        assert!(
            unanswered.is_empty(),
            "these routes do not say whether an app may reach them. Add a row to \
         ROUTE_REACH in app_reach.rs, default Host unless an app needs it: {unanswered:?}"
        );

        let stale: Vec<&&str> = declared
            .iter()
            .filter(|p| !scanned.contains_key(**p))
            .collect();
        assert!(
            stale.is_empty(),
            "ROUTE_REACH answers for routes that no longer exist: {stale:?}"
        );
    }

    #[test]
    fn an_app_route_lists_only_methods_the_route_serves() {
        let scanned = scan_routes();
        for (path, reach, methods) in ROUTE_REACH {
            if *reach != App {
                continue;
            }
            let served = scanned
                .get(*path)
                .unwrap_or_else(|| panic!("{path} is not a route"));
            if served.contains("ANY") {
                continue;
            }
            for method in *methods {
                assert!(
                    served.contains(*method),
                    "{path} is app-reachable for {method}, which it does not serve"
                );
            }
        }
    }
}
