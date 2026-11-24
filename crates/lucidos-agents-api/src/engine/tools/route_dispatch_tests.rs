use axum::http::{HeaderMap, StatusCode};
use serde_json::{json, Value};

use super::*;
use crate::capability_manifest::{
    Arg, ArgIn, ArgType, Domain, Gate, LlmExecution, Method, Operation,
};
use crate::core::subprocess_origin::{
    init_agent_origin_secret, subprocess_origin, SubprocessOrigin,
};

const fn arg(name: &'static str, ty: ArgType, loc: ArgIn) -> Arg {
    Arg {
        name,
        ty,
        enum_values: &[],
        required: false,
        loc,
        description: "",
    }
}

const fn op(
    action: &'static str,
    method: Method,
    path: &'static str,
    args: &'static [Arg],
) -> Operation {
    Operation {
        action,
        summary: "",
        method,
        path,
        args,
        cli_name: action,
        sdk_name: action,
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    }
}

const OPS: &[Operation] = &[
    op(
        "list",
        Method::Get,
        "/things",
        &[
            arg("limit", ArgType::Int, ArgIn::Query),
            arg("filter", ArgType::Str, ArgIn::Query),
        ],
    ),
    op(
        "update",
        Method::Put,
        "/things/:id",
        &[
            arg("id", ArgType::Str, ArgIn::Path),
            arg("label", ArgType::Str, ArgIn::Body),
            arg("window", ArgType::Int, ArgIn::Body),
            arg("routes", ArgType::Json, ArgIn::Body),
        ],
    ),
    op(
        "pin",
        Method::Put,
        "/things",
        &[
            arg("id", ArgType::Str, ArgIn::Query),
            arg("pinned", ArgType::Bool, ArgIn::FixedBody("true")),
        ],
    ),
    Operation {
        llm: Some(false),
        ..op("purge", Method::Delete, "/things", &[])
    },
];

const DOMAIN: Domain = Domain {
    name: "things",
    tool_name: "things",
    tool_summary: "",
    llm: true,
    cli: true,
    sdk: false,
    operations: OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::Route,
};

#[test]
fn query_args_ride_the_query_and_absent_ones_stay_off() {
    let request = route_request(
        &DOMAIN,
        &json!({ "action": "list", "limit": 5, "filter": "a b&c" }),
    )
    .unwrap();
    assert_eq!(
        request,
        RouteRequest {
            method: Method::Get,
            uri: "/things?limit=5&filter=a%20b%26c".into(),
            body: None,
        }
    );
    let bare = route_request(&DOMAIN, &json!({ "action": "list", "filter": null })).unwrap();
    assert_eq!(bare.uri, "/things");
}

/// A path arg fills its segment. A body arg rides as given, an explicit null
/// included, and a JSON arg sent as a string is parsed, as the CLI parses it.
#[test]
fn path_and_body_args_ride_as_the_cli_sends_them() {
    let args = json!({
        "action": "update",
        "id": "a/b",
        "label": "Fast",
        "window": null,
        "routes": "[{\"provider\":\"vertex\"}]",
        "stray": true,
    });
    let request = route_request(&DOMAIN, &args).unwrap();
    assert_eq!(request.method, Method::Put);
    assert_eq!(request.uri, "/things/a%2Fb");
    assert_eq!(
        request.body,
        Some(json!({ "label": "Fast", "window": null, "routes": [{ "provider": "vertex" }] }))
    );

    let no_body = route_request(&DOMAIN, &json!({ "action": "update", "id": "x" })).unwrap();
    assert_eq!(
        no_body.body,
        Some(json!({})),
        "a PUT always sends an object"
    );
}

/// A fixed value rides every call, and the caller cannot override it.
#[test]
fn a_fixed_body_value_rides_whatever_the_caller_sends() {
    let request = route_request(
        &DOMAIN,
        &json!({ "action": "pin", "id": "x", "pinned": false }),
    )
    .unwrap();
    assert_eq!(request.uri, "/things?id=x");
    assert_eq!(request.body, Some(json!({ "pinned": true })));
}

#[test]
fn a_bad_action_or_arg_is_refused_before_any_request() {
    let err = route_request(&DOMAIN, &json!({})).unwrap_err();
    assert!(
        err.contains("action is required (one of: list, update, pin)"),
        "{err}"
    );
    let err = route_request(&DOMAIN, &json!({ "action": "purge" })).unwrap_err();
    assert!(
        err.contains("unknown action 'purge'"),
        "an LLM-off op is unknown: {err}"
    );
    let err = route_request(&DOMAIN, &json!({ "action": "update" })).unwrap_err();
    assert!(err.contains("id is required"), "{err}");
    let err = route_request(
        &DOMAIN,
        &json!({ "action": "update", "id": "x", "routes": "[" }),
    )
    .unwrap_err();
    assert!(err.contains("routes must be valid JSON"), "{err}");
    let err = route_request(&DOMAIN, &json!({ "action": "list", "limit": [1] })).unwrap_err();
    assert!(err.contains("limit takes one value"), "{err}");
}

/// The tool presents exactly the origin token a `lucidos` command from the
/// same thread's bash tool would.
#[test]
fn the_tool_carries_the_clis_origin_token() {
    init_agent_origin_secret("route-dispatch-test-secret".into());
    let thread = uuid::Uuid::new_v4();
    let cli_token = subprocess_origin_env_vars(Some(thread), 0, None)
        .into_iter()
        .find(|(name, _)| *name == ENV_AGENT_ORIGIN_TOKEN)
        .map(|(_, token)| token)
        .unwrap();
    let headers = origin_headers(thread);
    assert_eq!(headers[HEADER_AGENT_ORIGIN_TOKEN], cli_token.as_str());
    assert_eq!(headers.len(), 1, "no other credential: {headers:?}");
}

/// A route sees the tool's call as the calling thread's agent, never as the
/// owner and never as an unattested caller.
#[tokio::test]
async fn a_route_sees_an_agent_from_the_calling_thread() {
    use crate::api::actor::{owner_attestation, OwnerAttestation};
    async fn who(headers: HeaderMap, body: String) -> axum::Json<Value> {
        let thread = match subprocess_origin(&headers) {
            SubprocessOrigin::Subprocess {
                source_thread_id, ..
            } => source_thread_id,
            _ => None,
        };
        axum::Json(json!({
            "agent": owner_attestation(&headers) == OwnerAttestation::Agent,
            "thread": thread,
            "body": body,
        }))
    }
    init_agent_origin_secret("route-dispatch-test-secret".into());
    let thread = uuid::Uuid::new_v4();
    let router = axum::Router::new().route("/things/:id", axum::routing::put(who));
    let request = route_request(
        &DOMAIN,
        &json!({ "action": "update", "id": "x", "label": "L" }),
    )
    .unwrap();
    let answer = send(router, &request, origin_headers(thread))
        .await
        .unwrap();
    let answer: Value = serde_json::from_str(&answer).unwrap();
    assert_eq!(answer["agent"], true, "{answer}");
    assert_eq!(answer["thread"], json!(thread), "{answer}");
    assert_eq!(
        answer["body"],
        json!({ "label": "L" }).to_string(),
        "{answer}"
    );
}

#[test]
fn a_prepared_permission_tells_the_agent_how_to_ask() {
    let request = route_request(&DOMAIN, &json!({ "action": "list" })).unwrap();
    let body = json!({
        "error": "This act needs the owner's permission.",
        "reason": "permission_required",
        "request_id": "permission-request:abc",
    });
    let err = tool_result(&request, StatusCode::FORBIDDEN, &body.to_string()).unwrap_err();
    assert!(err.starts_with("Error: Nothing changed"), "{err}");
    assert!(err.contains("ask_user_question"), "{err}");
    assert!(err.contains("`permission-request:abc`"), "{err}");

    let refused = json!({ "error": "No such thing" }).to_string();
    let err = tool_result(&request, StatusCode::NOT_FOUND, &refused).unwrap_err();
    assert_eq!(
        err,
        "Error: GET /api/v1/things returned 404 Not Found: No such thing"
    );

    let refused = json!({ "success": false, "error": "Model 'x' not found" }).to_string();
    let err = tool_result(&request, StatusCode::OK, &refused).unwrap_err();
    assert_eq!(
        err, "Error: GET /api/v1/things returned 200 OK: Model 'x' not found",
        "an ApiResult refusal is an error under a 2xx"
    );

    assert_eq!(
        tool_result(&request, StatusCode::OK, " {\"a\":1}\n"),
        Ok("{\"a\":1}".into())
    );
    assert_eq!(
        tool_result(&request, StatusCode::NO_CONTENT, ""),
        Ok("[ACTION COMPLETED]".into())
    );
}

/// The domains whose grouped tool still runs a hand-written handler. This list
/// only shrinks: move a domain to its route, then delete its name here.
/// Registry row: "Manifest tools that run in-process" in
/// `docs/temporary-measures.md`.
const LEGACY_IN_PROCESS: &[&str] = &[
    "notifications",
    "preferences",
    "triggers",
    "trigger_groups",
    "events",
    "changes",
    "repositories",
    "env_vars",
    "widgets",
    "threads",
    "memory",
    "recall",
    "thread_queue",
    "mcp",
    "plugins",
];

#[test]
fn only_the_listed_legacy_domains_run_in_process() {
    for domain in domains() {
        let listed = LEGACY_IN_PROCESS.contains(&domain.name);
        match domain.llm_execution {
            LlmExecution::LegacyInProcess => assert!(
                listed,
                "{} runs in-process but is not on LEGACY_IN_PROCESS. A new domain runs \
                 the route its CLI calls: set llm_execution to Route and write no handler.",
                domain.name
            ),
            LlmExecution::Route => assert!(
                !listed,
                "{} is route-backed now: delete it from LEGACY_IN_PROCESS.",
                domain.name
            ),
        }
    }
    for name in LEGACY_IN_PROCESS {
        assert!(
            domains().iter().any(|d| d.name == *name),
            "LEGACY_IN_PROCESS names {name}, which is no manifest domain"
        );
    }
}

#[test]
fn a_route_backed_tool_reaches_no_handler() {
    let tool_names = include_str!("../../../../lucidos-core/src/llm/tool_names.rs");
    let route_backed = domains()
        .iter()
        .filter(|d| d.llm && d.llm_execution == LlmExecution::Route);
    for domain in route_backed {
        let tool = domain.tool_name;
        assert!(
            route_backed_domain(tool).is_some(),
            "{tool} must dispatch to its route before any handler arm"
        );
        assert!(
            domain.alias_names().is_empty(),
            "{tool} has aliases, which would bypass its route: {:?}",
            domain.alias_names()
        );
        assert!(
            !tool_names.contains(&format!(": &str = \"{tool}\";")),
            "tool_names.rs names {tool}, a route-backed tool. A handler arm needs that \
             constant, and a route-backed tool has no handler."
        );
    }
}
