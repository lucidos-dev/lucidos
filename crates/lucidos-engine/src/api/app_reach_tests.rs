//! The table answers for every route, and keeps the decisions ADR 0231 made.

use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

fn api_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/api")
}

/// A route-registering function that is NOT part of the `/api/v1` router.
///
/// Every other one must be a `router()` that `create_router` merges, and the
/// scan below fails on anything it cannot place. That is what stops a new
/// router escaping classification by not looking like the others.
///
/// The third column is prose for a reader: nothing keys on it.
const MOUNTED_ELSEWHERE: &[(&str, &str, &str)] = &[
    (
        "apps.rs",
        "ui_router",
        "nested at /app, outside /api/v1, so the hatch cannot address it",
    ),
    (
        "local_auth.rs",
        "an_open_door_streams_a_body_instead_of_collecting_it",
        "an inline test's own router",
    ),
    (
        "workspace_label.rs",
        "stand_in_gateway",
        "an inline test's stand-in gateway",
    ),
];

/// Routes whose path is built at registration, so no literal exists to put in
/// `ROUTE_REACH`.
///
/// Each row names the file, the function, and the answer. The scan refuses a
/// computed route absent from here, so this is a declaration rather than a way
/// around the question. It lives beside the scan because the scan is its only
/// reader: the running gate matches on the route axum reports, which is the
/// built string.
///
/// The font bytes are version-stamped into their own filename, which is what
/// lets them be cached as immutable. Pinning that literal would make a font
/// upgrade edit a table for no gain.
const COMPUTED_ROUTE_REACH: &[(&str, &str, Reach)] = &[("sdk_fonts.rs", "router", Asset)];

/// Every route the `/api/v1` router serves, with the methods each answers.
///
/// Built from the shared scan, filtered to the functions this mount owns. A
/// registering function that is neither merged here nor declared above is a
/// panic. Skipping one silently is how a route would reach an app unasked.
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
    // The check can say no. Without this, an empty or broken scan would pass
    // every loop below while proving nothing.
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
fn an_unclassified_route_is_denied() {
    assert_eq!(entry_for("/not-a-route"), None);
    assert!(!app_may_call("/not-a-route", "GET"));
    // A real route the table denies answers the same way to a caller.
    assert!(!app_may_call("/credential-value", "GET"));
    // And a method the row does not list is denied on an allowed route.
    assert!(app_may_call("/models", "GET"));
    assert!(!app_may_call("/models", "DELETE"));
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

/// The omissions ADR 0231 names as the safety property.
///
/// Each was a decision the SDK made by leaving something out, and a generic
/// hatch re-opens all of them. A later widening argues with this test.
#[test]
fn the_deliberate_omissions_stay_denied() {
    let denied: &[(&str, &str)] = &[
        // The user sends their own prompt, always.
        ("/chat/stream", "POST"),
        ("/threads", "POST"),
        ("/threads/:thread_id/continue", "POST"),
        ("/threads/:thread_id/follow-up", "POST"),
        ("/threads/:id/compose", "PUT"),
        // Never answer as the user.
        ("/threads/:thread_id/answer-question", "POST"),
        ("/internal/ask-user-question", "POST"),
        ("/internal/approve-plan", "POST"),
        ("/internal/permission-prompt", "POST"),
        ("/command-permission/consent", "POST"),
        ("/mcp/consent", "POST"),
        ("/mcp/auto-approve", "PUT"),
        ("/command-checkpoint/undo", "POST"),
        // The credential never enters the iframe.
        ("/credential-value", "GET"),
        ("/credential-reveal-token", "POST"),
        ("/credential-base-urls", "PUT"),
        ("/credentials", "GET"),
        ("/backup/key", "GET"),
        ("/email/send", "POST"),
        ("/email-account", "GET"),
        ("/oauth/accounts", "GET"),
        ("/oauth/complete", "POST"),
        ("/oauth/reauthorize", "POST"),
        // A thread's contents, and the workspace's memory.
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
        // The device id the isolation keeps from the frame.
        ("/devices", "GET"),
        ("/devices/register", "POST"),
        ("/devices/hand-over", "POST"),
        ("/device-presence", "POST"),
        ("/push/subscribe", "POST"),
        // Changing the platform.
        ("/changes/:id/apply", "POST"),
        ("/changes/apply-all", "POST"),
        ("/claude-code/control", "POST"),
        ("/engine/rebuild", "POST"),
        ("/restart", "POST"),
        ("/plugins/install-request", "POST"),
        ("/plugins/uninstall-request", "POST"),
        ("/thread-queue/policy", "PUT"),
        ("/proxy-modules/reload", "POST"),
        ("/handshake-scripts/approve", "POST"),
        // Outside the workspace.
        ("/browse-directories", "GET"),
        ("/repositories", "GET"),
        ("/repositories/:id/file", "GET"),
        ("/workspaces", "GET"),
        ("/webhooks", "GET"),
        // A sibling app's source, and a capture the host did not ask for.
        ("/app/:app_id/source", "GET"),
        ("/app-capture", "POST"),
    ];
    for (path, method) in denied {
        assert!(
            !app_may_call(path, method),
            "{method} {path} is app-reachable, and ADR 0231 says it must not be"
        );
    }
}

/// An app reads the env vars and never writes them.
///
/// `build_script_env_vars` puts every user env var into `run_bash` and
/// `run_python`. `RESERVED_EXACT` guards the names the engine owns, and an
/// interpreter's loader hooks are not among them. So a writable `/env-vars`
/// was app authority converted into host code execution. `PYTHONPATH` names a
/// directory the app wrote through `/data/*path`, and a `sitecustomize.py`
/// there runs on the next Python tool call.
///
/// That is the chain ADR 0144 closed for `scripts/`, through a door ADR 0231
/// never argued for: it names `/env-vars` as a READ, in its context and in its
/// plan, and the write verbs carried no reasoning.
///
/// The read stays. It is the gap ADR 0231 exists to close, and it hands an app
/// no name the engine spawns anything with.
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

/// Everything a shipped SDK namespace calls stays reachable.
///
/// The bridge reads the generated copy of this table, so a row changed here
/// would break `lucidos.data.read` in every app with nothing else failing.
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

// ---------------------------------------------------------------------------
// The generated copy the bridge reads
// ---------------------------------------------------------------------------

fn generated_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .join("packages/lucidos-sdk/src/generated/app-reach.ts")
}

/// The `App` rows, and only those.
///
/// Emitting the denials as well would ship a map of the engine to every app,
/// and the client needs none of it: absence already means refused.
fn generate_ts() -> String {
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED. Do not edit by hand.\n");
    out.push_str(
        "// Regenerate: cargo test -p lucidos-engine --lib generate_app_reach_file -- --ignored\n",
    );
    out.push_str("//\n");
    out.push_str("// Source of truth: ROUTE_REACH in crates/lucidos-engine/src/api/app_reach.rs\n");
    out.push_str("// (ADR 0231). Only app-reachable routes appear. A route absent from this\n");
    out.push_str("// table is refused, which is what makes the list safe to be short.\n\n");
    out.push_str("export interface AppReachableRoute {\n");
    out.push_str("  /** Route pattern, with `:param` and `*wildcard` segments. */\n");
    out.push_str("  path: string;\n");
    out.push_str("  methods: string[];\n");
    out.push_str("}\n\n");
    out.push_str("export const APP_REACHABLE_ROUTES: AppReachableRoute[] = [\n");
    for (path, reach, methods) in ROUTE_REACH {
        if *reach != App {
            continue;
        }
        let list = methods
            .iter()
            .map(|m| format!("'{m}'"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("  {{ path: '{path}', methods: [{list}] }},\n"));
    }
    out.push_str("];\n");
    out
}

#[test]
fn generated_app_reach_is_up_to_date() {
    let path = generated_path();
    let generated = generate_ts();
    match std::fs::read_to_string(&path) {
        Ok(existing) => assert_eq!(
            existing,
            generated,
            "Generated {} is stale. Run: cargo test -p lucidos-engine --lib \
             generate_app_reach_file -- --ignored",
            path.display()
        ),
        Err(_) => panic!(
            "Generated file missing at {}. Run: cargo test -p lucidos-engine --lib \
             generate_app_reach_file -- --ignored",
            path.display()
        ),
    }
}

#[test]
#[ignore]
fn generate_app_reach_file() {
    let path = generated_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, generate_ts()).unwrap();
    crate::log!("[Codegen] wrote {}", path.display());
}

// ---------------------------------------------------------------------------
// The layer, driven over a real router
// ---------------------------------------------------------------------------

/// A router shaped like the engine's: the same mount, the same layer.
///
/// The layer reads `MatchedPath`, which exists only once axum has routed. A
/// direct call would prove nothing about what runs in production.
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
    axum::Router::new().nest(super::super::API_V1_PREFIX, inner)
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
    // The shell, the CLI and a standalone app tab carry no stamp. The gate has
    // nothing to say about any of them.
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

/// `/preferences` is an `App` route, yet a key the agent may not write is
/// refused to an app too. Otherwise an app turns the command guard off, raises
/// the tool-call cap, or opens the engine bind that `/network-config` keeps
/// host-only.
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

/// The coding-agent binary paths choose what a session spawns, and the
/// permission mode chooses what it may do unasked. Say an app sets the Codex
/// path to `/bin/sh` and writes `apps/<id>/app-server`. The next Codex thread
/// on that app then runs the app's script as the user. The agent may still set
/// these keys, so they are refused to an app alone.
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

/// `local_base_url` chooses where local-model chat goes. An app setting it to
/// its own host would read every prompt. Settings still writes it.
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

/// `vapid_keys` is the Web Push signing keypair. An app replacing it with
/// garbage makes every later push fail to sign, on every device.
#[tokio::test]
async fn a_stamped_write_of_engine_bookkeeping_is_refused() {
    use axum::http::StatusCode;
    for query in [
        "key=vapid_keys",
        "key=backfill_repo_names_from_changes_done",
        "key=theme&key=vapid_keys",
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

/// The bridge stamps the decoded app id, and `fetch` sends a Latin-1 character
/// as one raw byte. A stamp that is not ASCII is still a stamp.
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

#[tokio::test]
async fn an_unmatched_route_answers_404_rather_than_a_refusal() {
    // No `MatchedPath`, so the request never routed. A refusal here would
    // point the caller at the wrong problem.
    assert_eq!(
        status_of("GET", "/api/v1/no-such-route", Some("habit-tracker")).await,
        axum::http::StatusCode::NOT_FOUND
    );
}

/// The three routes with live breakage, which ADR 0231 opens.
///
/// All three broke on a READ, which is what the ADR names: an app rendering
/// "Unavailable in the shell" for `/env-vars`, a raw model id for `/models`,
/// and a notification it could not raise. The write half of `/env-vars` is
/// refused, for the reason beside its row.
#[test]
fn the_routes_that_motivated_the_hatch_are_open() {
    assert!(app_may_call("/env-vars", "GET"));
    assert!(app_may_call("/models", "GET"));
    assert!(app_may_call("/notifications", "POST"));
}
