//! Capability parity manifest — the single source of truth for which agent
//! surfaces (LLM tools, the `lucidos` CLI, the JS SDK) expose each capability.
//!
//! See `docs/adr/0018-capability-parity-manifest.md`. The problem this solves:
//! the two agent-facing surfaces (LLM tools + CLI) silently drifted behind
//! UI/SDK/HTTP — e.g. notifications had `list`/`mark_read`/`mark_all_read` in the
//! UI/SDK/HTTP but no LLM tool and no CLI command, so the agent fell back to
//! reverse-engineering `curl` against the gateway.
//!
//! ## How it enforces parity
//!
//! Each [`Domain`] declares its operations once, plus which surfaces it targets
//! (`llm` / `cli` / `sdk`; `ui` / `http` are the substrate, not generated). From
//! this one declaration:
//!
//! - the grouped LLM `ToolDefinition` is built in-crate ([`build_llm_tool`]),
//!   so the tool schema can't drift from the manifest (guarded by a test);
//! - the CLI command module is rendered at build time by
//!   `crates/lucidos-cli/build.rs`, and the SDK capability table by
//!   `scripts/codegen.sh` into `packages/lucidos-sdk/src/generated/`. Both come
//!   from `lucidos_capabilities::codegen`, so neither can fall behind.
//!
//! The data itself lives in the `lucidos-capabilities` leaf crate, re-exported
//! here, so the CLI can build from it without the engine. Adding an operation
//! there forces the generated surfaces to follow. It also forces a handler: a
//! unit test checks the handler's recognised-action set against the manifest
//! (see `engine/tools/notifications.rs`).
//!
//! ## Deliberate non-domain: the `event_wait` family
//!
//! `await_event`, `list_event_waits` and `cancel_event_wait`
//! (`engine::event_wait`, ADR 0047) are standalone LLM tools and are
//! deliberately NOT a domain here, so their absence is a decision rather than
//! the drift this manifest exists to catch.
//!
//! **They already have CLI parity**, hand-wired as `lucidos await-event` and
//! `lucidos event-waits list` / `cancel`. What they cannot have is *generated*
//! parity, and the reason is structural: all three are scoped to the CALLING
//! THREAD and take no thread argument at all, which is what stops one thread
//! reading or ending another's subscriptions. The generators build an HTTP
//! request out of declared `Arg`s, so a `:thread_id` path segment would have to
//! be one, and then it would be a flag a caller could point anywhere. The CLI
//! reads `$LUCIDOS_THREAD_ID` by hand instead
//! (`crates/lucidos-cli/src/{await_event,event_waits}.rs`), which the manifest
//! has no way to express.
//!
//! `request_read` (ADR 0409) is absent for the same reason. It acts on the
//! calling thread and takes no argument, so its CLI verb, `lucidos
//! request-read`, is hand-wired over `$LUCIDOS_THREAD_ID` too. So is `lucidos
//! threads child-reports` (ADR 0566): only a coding agent takes its own
//! thread's reports, so no LLM tool or SDK facade has one to take.
//!
//! No SDK either: an app iframe is not a thread and holds no subscriptions of
//! its own. The capability an app or a script actually wants there is the
//! `triggers` domain, which IS in the manifest: a standing rule that reacts to
//! an event with no thread to resume.
//!
//! This note used to argue that parity was unreachable because `await_event`
//! ended the turn and a CLI invocation had no turn to park. ADR 0049 retired
//! that shape (a subscription holds nothing, and its delivery is an ordinary
//! new turn), which is exactly what made the CLI verbs possible.
//!
//! **`lucidos build-slot` is absent for a different reason**, and it is also a
//! decision rather than drift. A *build slot* is HOST state, not workspace
//! state: the pool is a directory of file locks under `$HOME`, shared by every
//! workspace on the machine (ADR 0070). The verb takes no domain argument and
//! reaches no engine. It must work with no engine running at all, which is the
//! case this manifest cannot express. Every domain here materialises as an
//! HTTP call to one workspace, so generating this one would build exactly the
//! dependency the design removes. No LLM tool and no SDK facade either: an app
//! iframe runs no builds.

pub use lucidos_capabilities::*;

use crate::llm::provider::ToolDefinition;
use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// LLM tool schema — built in-crate from the manifest so it can't drift.
// ---------------------------------------------------------------------------

/// Parse an operation's raw `llm_schema` (a JSON object of properties) into a
/// map. Panics on malformed JSON — it's static manifest data covered by
/// [`tests::every_llm_domain_builds`].
fn parse_llm_schema(op: &Operation) -> Map<String, Value> {
    let raw = op
        .llm_schema
        .expect("parse_llm_schema called without llm_schema");
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(m)) => m,
        Ok(_) => panic!("llm_schema for action '{}' is not a JSON object", op.action),
        Err(e) => panic!(
            "llm_schema for action '{}' is invalid JSON: {}",
            op.action, e
        ),
    }
}

/// The property name→schema pairs an operation contributes to the grouped LLM
/// tool: the raw `llm_schema` verbatim when supplied, else scalar properties
/// derived from `args`.
fn op_llm_properties(op: &Operation) -> Vec<(String, Value)> {
    if op.llm_schema.is_some() {
        return parse_llm_schema(op).into_iter().collect();
    }
    op.args
        .iter()
        .map(|arg| {
            let mut prop = Map::new();
            prop.insert("type".to_string(), Value::String(arg.ty.json_type().into()));
            if !arg.enum_values.is_empty() {
                prop.insert("enum".to_string(), serde_json::json!(arg.enum_values));
            }
            prop.insert(
                "description".to_string(),
                Value::String(arg.description.into()),
            );
            (arg.name.to_string(), Value::Object(prop))
        })
        .collect()
}

/// Required-argument names for an operation's "(requires: …)" hint, derived from
/// the required `args`. Skipped for ops with a raw `llm_schema`: their LLM shape
/// can diverge from `args` (different names, e.g. `trigger_id` vs the HTTP query
/// `id`), so the per-property descriptions in the schema carry requiredness
/// instead of a possibly-wrong flat hint.
fn required_arg_names(op: &Operation) -> Vec<String> {
    if op.llm_schema.is_some() {
        return Vec::new();
    }
    op.args
        .iter()
        .filter(|a| a.required)
        .map(|a| a.name.to_string())
        .collect()
}

/// Build the grouped `ToolDefinition` for a domain from its manifest entry.
/// Shape mirrors the existing grouped tools (`manage_models` / `manage_
/// repositories`): an `action` enum plus the union of all operation args, with
/// only `action` strictly required (per-action requirements are described in the
/// text, since one params object spans every action).
pub fn build_llm_tool(domain: &Domain) -> ToolDefinition {
    let llm_ops: Vec<&Operation> = domain
        .operations
        .iter()
        .filter(|o| o.on_llm(domain))
        .collect();

    let mut description = String::from(domain.tool_summary);
    description.push_str("\n\nActions:");
    for op in &llm_ops {
        // Colon rather than an em dash: this string is LLM-facing prose the
        // engine emits on every turn, so `.claude/rules/em-dashes.md`
        // applies to it exactly as to a source line.
        description.push_str(&format!("\n• {}: {}", op.action, op.summary));
        // Required-args hint: from the raw llm_schema when the op supplies one
        // (its shape may differ from the HTTP args), else from `args`.
        let required = required_arg_names(op);
        if !required.is_empty() {
            description.push_str(&format!(" (requires: {})", required.join(", ")));
        }
    }

    let mut properties = Map::new();
    properties.insert(
        "action".to_string(),
        serde_json::json!({
            "type": "string",
            "enum": domain.actions(),
            "description": "Which operation to perform.",
        }),
    );
    // Union the per-operation properties. An operation whose LLM shape diverges
    // from its HTTP `args` supplies a raw `llm_schema` (used verbatim); the rest
    // derive scalar properties from `args`. A name shared across operations must
    // have a consistent shape (asserted by a manifest test), so first-wins.
    for op in &llm_ops {
        for (name, schema) in op_llm_properties(op) {
            properties.entry(name).or_insert(schema);
        }
    }

    ToolDefinition {
        name: domain.tool_name.to_string(),
        description,
        parameters: serde_json::json!({
            "type": "object",
            "properties": Value::Object(properties),
            "required": ["action"],
        }),
    }
}

/// The grouped LLM tools of a workspace with no capability configured: the
/// ungated domains, plus Classic's `memory`, since a turn runs Classic until
/// its Tree is ready.
pub fn llm_tools() -> Vec<ToolDefinition> {
    llm_tools_for(&crate::llm::ToolCapabilities::default())
}

/// The grouped LLM tools one workspace is offered: every domain whose gate
/// `caps` opens, in manifest order. A shut gate removes its tool and moves
/// nothing else. So the memory module swaps `memory` for `recall` in one
/// slot, and the rest of the cached array stays as it was.
pub fn llm_tools_for(caps: &crate::llm::ToolCapabilities) -> Vec<ToolDefinition> {
    domains()
        .iter()
        .filter(|d| d.llm && crate::llm::tools::gate_is_open(d.llm_gate, caps))
        .map(build_llm_tool)
        .collect()
}

#[cfg(test)]
mod tests {

    /// The arrange schema spells the size and dashboard enums out by hand,
    /// since a `const` string cannot read them. This keeps it on the Rust lists.
    #[test]
    fn the_arrange_schema_offers_every_size_and_dashboard() {
        let schema: serde_json::Value = serde_json::from_str(DASHBOARD_ARRANGE_LLM_SCHEMA).unwrap();
        let names = |v: &serde_json::Value| -> Vec<String> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect()
        };
        let dashboards: Vec<String> = crate::engine::thread_events::Dashboard::ALL
            .iter()
            .map(|d| d.as_str().to_string())
            .collect();
        let sizes: Vec<String> = crate::core::apps::TileSize::ALL
            .iter()
            .map(|s| s.as_str().to_string())
            .collect();
        assert_eq!(names(&schema["dashboard"]["enum"]), dashboards);
        assert_eq!(
            names(&schema["tiles"]["items"]["properties"]["size"]["enum"]),
            sizes
        );
        assert_eq!(
            DASHBOARD_QUERY_ARG.enum_values,
            dashboards
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .as_slice()
        );
    }
    use super::*;
    use std::collections::HashMap;

    /// The schema enum is what the model picks values from, and the parser
    /// validates against `ThreadStatus::ALL`. If they drift, the model is
    /// offered a value the engine then refuses, or a real status becomes
    /// unaskable. Both schemas are checked because the two are separate
    /// literals.
    #[test]
    fn threads_status_enum_matches_the_thread_status_enum() {
        let values = crate::engine::thread_lifecycle::ThreadStatus::ALL
            .iter()
            .map(|s| format!("\"{}\"", s.as_str()))
            .collect::<Vec<_>>()
            .join(",");
        let expected = format!("\"enum\":[{values}]");
        for (action, schema) in [
            ("list", THREADS_LIST_LLM_SCHEMA),
            ("count", THREADS_COUNT_LLM_SCHEMA),
        ] {
            assert!(
                schema.contains(&expected),
                "the threads '{action}' status property must offer exactly \
                 ThreadStatus::ALL, expected to find {expected}"
            );
        }
    }

    #[test]
    fn threads_change_state_enum_matches_the_change_state_kind() {
        let values = crate::engine::thread_lifecycle::ChangeStateKind::ALL
            .map(|k| format!("\"{}\"", k.as_str()))
            .join(",");
        let expected = format!("\"change_state\": {{\"type\":\"string\",\"enum\":[{values}]");
        assert!(
            THREADS_LIST_LLM_SCHEMA.contains(&expected),
            "the threads 'list' change_state property must offer exactly \
             ChangeStateKind::ALL, expected to find {expected}"
        );
    }

    /// A caller reading only the tool schema has to be able to tell that
    /// `active` is a union, and which half answers "is the workspace busy?".
    /// Getting that wrong is what hid four pending changes for three hours on
    /// 2026-08-07.
    #[test]
    fn threads_active_descriptions_state_the_union_and_point_at_running() {
        for (action, schema) in [
            ("list", THREADS_LIST_LLM_SCHEMA),
            ("count", THREADS_COUNT_LLM_SCHEMA),
        ] {
            let parsed: serde_json::Value =
                serde_json::from_str(schema).expect("schema is valid JSON");
            let active = parsed["active"]["description"]
                .as_str()
                .expect("active is documented");
            assert!(
                active.contains("UNION") && active.contains("waiting_for_user_answer"),
                "threads '{action}' must say what active actually groups: {active}"
            );
            assert!(
                active.contains("status ['running']"),
                "threads '{action}' must name the filter a busy check wants: {active}"
            );
        }
    }

    #[test]
    fn notifications_domain_is_declared() {
        let d = domains()
            .iter()
            .find(|d| d.name == "notifications")
            .expect("notifications domain present");
        assert_eq!(d.actions(), vec!["list", "mark_read", "mark_all_read"]);
        assert!(d.llm && d.cli && d.sdk);
    }

    #[test]
    fn aliases_resolve_to_their_domain() {
        let d = domain_for_tool("read_notifications").expect("alias resolves");
        assert_eq!(d.name, "notifications");
        // The canonical tool name resolves too.
        assert_eq!(
            domain_for_tool("notifications").unwrap().name,
            "notifications"
        );
        // An unknown name does not.
        assert!(domain_for_tool("nope").is_none());
    }

    #[test]
    fn built_tool_exposes_action_enum_and_all_args() {
        let d = domain_for_tool("notifications").unwrap();
        let tool = build_llm_tool(d);
        assert_eq!(tool.name, "notifications");
        let props = &tool.parameters["properties"];
        assert_eq!(props["action"]["enum"], serde_json::json!(d.actions()));
        // Union of args across operations is present.
        for name in ["filter", "limit", "id"] {
            assert!(
                props.get(name).is_some(),
                "expected arg '{name}' in built tool schema"
            );
        }
        assert_eq!(tool.parameters["required"], serde_json::json!(["action"]));
    }

    /// A shared arg name must have one consistent shape across operations, or the
    /// first-wins union in `build_llm_tool` would silently hide a divergence.
    #[test]
    fn shared_arg_names_have_consistent_shape() {
        for d in domains() {
            let mut seen: HashMap<&str, (ArgType, &[&str])> = HashMap::new();
            for op in d.operations {
                for a in op.args {
                    if let Some((ty, ev)) = seen.get(a.name) {
                        assert_eq!(
                            *ty, a.ty,
                            "arg '{}' type differs across ops in '{}'",
                            a.name, d.name
                        );
                        assert_eq!(
                            *ev, a.enum_values,
                            "arg '{}' enum differs across ops in '{}'",
                            a.name, d.name
                        );
                    } else {
                        seen.insert(a.name, (a.ty, a.enum_values));
                    }
                }
            }
        }
    }

    /// `build_llm_tool` unions per-operation properties first-wins; a property
    /// defined by two operations with DIFFERENT structure (type / enum / oneOf /
    /// anyOf) would be silently hidden. Descriptions may legitimately differ
    /// (prose), so compare structure only. Guards the LLM-side union the same way
    /// `shared_arg_names_have_consistent_shape` guards the CLI/SDK `args`.
    #[test]
    fn llm_union_properties_have_consistent_structure() {
        fn structural(v: &Value) -> Value {
            match v {
                Value::Object(m) => Value::Object(
                    m.iter()
                        .filter(|(k, _)| k.as_str() != "description")
                        .map(|(k, val)| (k.clone(), val.clone()))
                        .collect(),
                ),
                other => other.clone(),
            }
        }
        for d in domains().iter().filter(|d| d.llm) {
            let mut seen: HashMap<String, Value> = HashMap::new();
            for op in d.operations.iter().filter(|o| o.on_llm(d)) {
                for (name, schema) in op_llm_properties(op) {
                    let s = structural(&schema);
                    if let Some(prev) = seen.get(&name) {
                        assert_eq!(
                            *prev, s,
                            "LLM property '{}' has conflicting structure across operations in \
                             domain '{}' — build_llm_tool's first-wins union would hide one",
                            name, d.name
                        );
                    } else {
                        seen.insert(name, s);
                    }
                }
            }
        }
    }

    /// Every LLM domain's grouped tool must build — exercises `build_llm_tool`
    /// for all of them so a malformed `llm_schema` (the raw JSON contributed by
    /// diverging ops) fails the suite rather than panicking at runtime.
    #[test]
    fn every_llm_domain_builds() {
        for d in domains().iter().filter(|d| d.llm) {
            let tool = build_llm_tool(d);
            assert_eq!(tool.name, d.tool_name);
            let actions = tool.parameters["properties"]["action"]["enum"]
                .as_array()
                .expect("action enum is an array");
            assert!(
                !actions.is_empty(),
                "domain '{}' has an LLM tool but no LLM-exposed actions",
                d.name
            );
            assert!(tool.parameters["properties"].is_object());
        }
    }

    /// No agent surface reaches webhooks, in any shape.
    ///
    /// The side-effect-grant precedent: an agent must not widen its own
    /// authority, and a webhook is a publicly reachable door emitting a pinned
    /// event. The manifest is the single source for every generated surface, so
    /// the flags here are what actually enforce it.
    #[test]
    fn no_agent_surface_can_create_a_webhook() {
        let webhooks = domains().iter().find(|d| d.name == "webhooks").unwrap();
        assert!(!webhooks.llm, "webhooks must not be an LLM tool");
        assert!(!webhooks.sdk, "an app must not manage webhooks");
        assert!(webhooks.cli, "the user's own CLI is the point");
        assert!(
            webhooks.actions().is_empty(),
            "an llm: false domain must contribute no actions to any tool schema"
        );
        assert!(
            webhooks.llm_aliases.is_empty(),
            "an alias would resolve a flat tool name back to this domain"
        );
        for op in webhooks.operations {
            assert!(!op.on_llm(webhooks), "'{}' reached the LLM", op.action);
            assert!(!op.on_sdk(webhooks), "'{}' reached the SDK", op.action);
            assert!(
                op.llm_alias.is_none(),
                "'{}' carries a flat LLM tool name",
                op.action
            );
        }
        let cli_ops: Vec<&str> = webhooks
            .operations
            .iter()
            .filter(|o| o.on_cli(webhooks))
            .map(|o| o.cli_name)
            .collect();
        assert_eq!(cli_ops, vec!["list", "create", "update", "delete"]);
    }

    /// Each operation's `llm_alias` (when present) must resolve back to its
    /// domain via `domain_for_tool`, and map to the operation's action via
    /// `legacy_tool_for_action` — the round-trip the grouped handlers rely on.
    #[test]
    fn llm_aliases_round_trip() {
        for d in domains() {
            for op in d.operations.iter().filter(|o| o.on_llm(d)) {
                if let Some(alias) = op.llm_alias {
                    let resolved = domain_for_tool(alias)
                        .unwrap_or_else(|| panic!("alias '{}' resolves to a domain", alias));
                    assert_eq!(
                        resolved.name, d.name,
                        "alias '{}' resolved to wrong domain",
                        alias
                    );
                    assert_eq!(
                        d.legacy_tool_for_action(op.action),
                        Some(alias),
                        "action '{}' in '{}' does not map back to its alias",
                        op.action,
                        d.name
                    );
                }
            }
        }
    }

    /// Any operation generated onto the CLI or SDK must have a real HTTP route
    /// (the codegen builds a request from `method` + `path`). An LLM-only op
    /// (e.g. trigger pause/resume) is exempt — it never reaches the generators.
    #[test]
    fn generated_ops_have_http_routes() {
        for d in domains() {
            for op in d.operations {
                if op.on_cli(d) || op.on_sdk(d) {
                    assert!(
                        op.path.starts_with('/'),
                        "op '{}.{}' is generated but has no valid path",
                        d.name,
                        op.action
                    );
                }
            }
        }
    }

    #[test]
    fn phase3_domains_declared() {
        let prefs = domains().iter().find(|d| d.name == "preferences").unwrap();
        assert_eq!(prefs.actions(), vec!["get", "set"]);
        assert!(prefs.llm && prefs.cli && prefs.sdk);

        let triggers = domains().iter().find(|d| d.name == "triggers").unwrap();
        assert_eq!(
            triggers.actions(),
            vec!["create", "list", "update", "delete", "pause", "resume", "run"]
        );
        // pause/resume are LLM-only (no dedicated HTTP route).
        let pause = triggers
            .operations
            .iter()
            .find(|o| o.action == "pause")
            .unwrap();
        assert!(pause.on_llm(triggers) && !pause.on_cli(triggers) && !pause.on_sdk(triggers));
        // `run` DOES have its own route (POST /triggers/run), so unlike
        // pause/resume it is on every surface. A regression that drops it from
        // the CLI or SDK re-splits the parity the manifest exists to hold.
        let run = triggers
            .operations
            .iter()
            .find(|o| o.action == "run")
            .unwrap();
        assert!(run.on_llm(triggers) && run.on_cli(triggers) && run.on_sdk(triggers));
        assert_eq!(run.path, "/triggers/run");
        assert!(run.mutating);

        let groups = domains()
            .iter()
            .find(|d| d.name == "trigger_groups")
            .unwrap();
        assert_eq!(
            groups.actions(),
            vec!["list", "create", "rename", "reorder", "delete"]
        );
        assert!(groups.llm && groups.cli && !groups.sdk);

        // apps is CLI+SDK only (LLM keeps standalone create_app/list_apps).
        let apps = domains().iter().find(|d| d.name == "apps").unwrap();
        assert!(!apps.llm && apps.cli && apps.sdk);
        assert!(apps.actions().is_empty(), "apps exposes no LLM actions");
        // list/get are on the SDK; update/delete are CLI-only.
        let get = apps.operations.iter().find(|o| o.action == "get").unwrap();
        assert!(get.on_cli(apps) && get.on_sdk(apps));
        let delete = apps
            .operations
            .iter()
            .find(|o| o.action == "delete")
            .unwrap();
        assert!(delete.on_cli(apps) && !delete.on_sdk(apps));
    }

    #[test]
    fn phase5a_domains_declared() {
        // mcp: grouped LLM tool plus a CLI. No SDK, since no app manages
        // servers.
        let mcp = domains().iter().find(|d| d.name == "mcp").unwrap();
        assert_eq!(
            mcp.actions(),
            vec!["setup", "list", "enable", "disable", "start", "stop", "remove"]
        );
        assert!(mcp.llm && mcp.cli && !mcp.sdk);
        assert_eq!(domain_for_tool("setup_mcp_server").unwrap().name, "mcp");
        assert_eq!(
            mcp.legacy_tool_for_action("remove"),
            Some("remove_mcp_server")
        );

        // The routes each verb declares are the routes `api/mcp.rs` serves. A
        // wrong path here is not cosmetic: the CLI generator builds its request
        // out of it, so a stale one ships a command that 404s.
        let op = |action: &str| {
            mcp.operations
                .iter()
                .find(|o| o.action == action)
                .unwrap_or_else(|| panic!("mcp has no {action}"))
        };
        assert_eq!(op("list").path, "/mcp/servers");
        assert_eq!(op("start").path, "/mcp/servers/:id/start");
        assert_eq!(op("stop").path, "/mcp/servers/:id/stop");
        assert_eq!(op("remove").path, "/mcp/servers/:id");
        assert_eq!(op("remove").method, Method::Delete);
        assert_eq!(
            op("enable").path,
            "/threads/:thread_id/mcp-servers/:id/enable"
        );
        assert_eq!(
            op("disable").path,
            "/threads/:thread_id/mcp-servers/:id/disable"
        );

        // Every path segment the generator substitutes needs an arg to
        // substitute from, or the emitted `format!` loses the id.
        for action in ["enable", "disable", "start", "stop", "remove"] {
            let o = op(action);
            assert!(
                o.args
                    .iter()
                    .any(|a| a.name == "id" && a.loc == ArgIn::Path),
                "mcp {action} declares no :id path arg"
            );
            assert!(o.on_cli(mcp), "mcp {action} should generate a CLI command");
        }

        for action in ["enable", "disable"] {
            assert!(
                op(action)
                    .args
                    .iter()
                    .any(|a| a.name == "thread_id" && a.loc == ArgIn::Path),
                "mcp {action} declares no :thread_id path arg"
            );
        }

        // setup is the one verb off the CLI: no HTTP route, and its shape is an
        // llm_schema rather than args, so a generated command would post
        // nothing.
        assert!(!op("setup").on_cli(mcp));

        // plugins — grouped LLM tool only (confirm-panel handshake; no CLI/SDK).
        let plugins = domains().iter().find(|d| d.name == "plugins").unwrap();
        assert_eq!(
            plugins.actions(),
            vec![
                "install",
                "register_marketplace",
                "check_updates",
                "update",
                "uninstall"
            ]
        );
        assert!(plugins.llm && !plugins.cli && !plugins.sdk);
        assert_eq!(domain_for_tool("install_plugin").unwrap().name, "plugins");
        assert_eq!(
            plugins.legacy_tool_for_action("register_marketplace"),
            Some("register_plugin_marketplace")
        );
    }

    #[test]
    fn phase5b_domains_declared() {
        // events — grouped LLM tool only (rich hand-written CLI stays).
        let events = domains().iter().find(|d| d.name == "events").unwrap();
        assert_eq!(
            events.actions(),
            vec!["emit", "query", "count", "event_types"]
        );
        assert!(events.llm && !events.cli && !events.sdk);
        assert_eq!(domain_for_tool("emit_event").unwrap().name, "events");
        assert_eq!(events.legacy_tool_for_action("count"), Some("count_events"));
        assert_eq!(
            events.legacy_tool_for_action("event_types"),
            Some("list_event_types")
        );

        // changes — grouped LLM tool only (hand-written CLI stays).
        let changes = domains().iter().find(|d| d.name == "changes").unwrap();
        assert_eq!(
            changes.actions(),
            vec![
                "list",
                "apply",
                "apply_when_settled",
                "apply_as_they_settle",
                "cancel_standing_apply",
                "set_aside",
                "bring_back"
            ]
        );
        assert!(changes.llm && !changes.cli && !changes.sdk);
        assert_eq!(domain_for_tool("apply_change").unwrap().name, "changes");
    }

    #[test]
    fn phase5c_thread_queue_declared() {
        let tq = domains().iter().find(|d| d.name == "thread_queue").unwrap();
        assert!(tq.llm && tq.cli && !tq.sdk);
        // LLM exposes list + update_policy (run-now/drop are CLI-only).
        assert_eq!(tq.actions(), vec!["list", "update_policy"]);
        // CLI exposes list + run-now + drop (update_policy is LLM-only).
        let cli_ops: Vec<&str> = tq
            .operations
            .iter()
            .filter(|o| o.on_cli(tq))
            .map(|o| o.cli_name)
            .collect();
        assert_eq!(cli_ops, vec!["list", "run-now", "drop"]);
        assert_eq!(
            domain_for_tool("list_thread_queue").unwrap().name,
            "thread_queue"
        );
        assert_eq!(
            tq.legacy_tool_for_action("update_policy"),
            Some("update_thread_queue_policy")
        );
    }

    #[test]
    fn phase5d_memory_declared() {
        let mem = domains().iter().find(|d| d.name == "memory").unwrap();
        assert!(mem.llm && mem.cli && !mem.sdk);
        // The LLM gets correction plus the two reads that help it ANSWER: an
        // on-demand `search` for when the pre-turn injection missed, and
        // `source` to walk one memory back to its conversation. `stats` and
        // `entries` stay off it deliberately, being operator reads with no
        // bearing on a user's question and exactly the browsing the tool
        // summaries warn against.
        assert_eq!(
            mem.actions(),
            vec!["correct", "correct_by_id", "source", "search"]
        );
        // The CLI keeps every read, including the two the LLM now shares.
        let cli_ops: Vec<&str> = mem
            .operations
            .iter()
            .filter(|o| o.on_cli(mem))
            .map(|o| o.cli_name)
            .collect();
        assert_eq!(cli_ops, vec!["stats", "entries", "source", "search"]);
        assert_eq!(domain_for_tool("correct_memory").unwrap().name, "memory");
        assert_eq!(
            mem.legacy_tool_for_action("correct_by_id"),
            Some("correct_memory_by_id")
        );
    }

    #[test]
    fn phase5e_threads_declared() {
        let threads = domains().iter().find(|d| d.name == "threads").unwrap();
        // `search` answers "we talked about this", which `list` structurally
        // cannot: it filters by status and channel and never by topic.
        // `detach_child` stops waiting for a child (ADR 0278). `archive` closes
        // the caller or one of its children (ADR 0310). `drafts` and
        // `held_messages` say what a thread holds that nothing has sent.
        // `cache_keep_alive` is the per-thread keep-alive switch (ADR 0554).
        // `triage` and `apply_triage` are thread triage (ADR 0349).
        assert_eq!(
            threads.actions(),
            vec![
                "list",
                "count",
                "drafts",
                "held_messages",
                "search",
                "detach_child",
                "archive",
                "cache_keep_alive",
                "triage",
                "apply_triage"
            ]
        );
        assert!(threads.llm && !threads.cli && !threads.sdk);
        assert_eq!(domain_for_tool("list_threads").unwrap().name, "threads");
        assert_eq!(
            threads.legacy_tool_for_action("count"),
            Some("count_threads")
        );
        // run_thread / run_coding_agent stay standalone — NOT folded here.
        assert!(domain_for_tool("run_thread").is_none());
        assert!(domain_for_tool("run_coding_agent").is_none());
    }

    /// What a thread holds unsent is READ-ONLY to every agent surface: the
    /// two actions that read it are GETs, flagged non-mutating.
    #[test]
    fn the_unsent_reads_are_read_only() {
        let threads = domains().iter().find(|d| d.name == "threads").unwrap();
        for action in ["drafts", "held_messages"] {
            let op = threads
                .operations
                .iter()
                .find(|o| o.action == action)
                .unwrap_or_else(|| panic!("the threads domain lost '{action}'"));
            assert!(!op.mutating, "'{action}' must not be flagged mutating");
            assert!(matches!(op.method, Method::Get), "'{action}' must be a GET");
        }
        assert_eq!(
            threads.legacy_tool_for_action("drafts"),
            Some("list_drafts")
        );
    }

    /// The `threads` domain is READ-ONLY, and deleting is the reason to say so
    /// out loud (ADR 0192). The assertion above pins the exact action list, so
    /// it already fails if a `delete` lands; this one fails with the reason.
    ///
    /// Deleting a thread removes its rows, its sub-threads and what the
    /// workspace learned from them, with no undo. It is offered to the owner in
    /// the UI and nowhere else. So adding it to any generated surface is the
    /// thing that must not happen quietly.
    #[test]
    fn the_threads_domain_exposes_no_delete() {
        let threads = domains().iter().find(|d| d.name == "threads").unwrap();
        for action in threads.actions() {
            assert!(
                !action.contains("delete") && !action.contains("remove"),
                "the threads domain grew a destructive action '{action}'. Deleting \
                 a thread is the owner's button in the UI and is reachable from no \
                 generated surface (ADR 0192)."
            );
        }
        for name in ["delete_thread", "delete_threads", "remove_thread"] {
            assert!(
                domain_for_tool(name).is_none(),
                "'{name}' resolves to a domain, so some surface offers it"
            );
        }
    }

    #[test]
    fn phase5f_env_vars_declared() {
        let ev = domains().iter().find(|d| d.name == "env_vars").unwrap();
        // Full LLM/CLI parity (no SDK). The retired set_environment_variable tool
        // stays wired as a back-compat alias to the `set` action.
        assert!(ev.llm && ev.cli && !ev.sdk);
        assert_eq!(ev.actions(), vec!["list", "set", "delete"]);
        let cli_ops: Vec<&str> = ev
            .operations
            .iter()
            .filter(|o| o.on_cli(ev))
            .map(|o| o.cli_name)
            .collect();
        assert_eq!(cli_ops, vec!["list", "set", "delete"]);
        // Back-compat alias resolves to the env_vars domain / `set` action.
        assert_eq!(
            domain_for_tool("set_environment_variable").unwrap().name,
            "env_vars"
        );
        assert_eq!(
            ev.legacy_tool_for_action("set"),
            Some("set_environment_variable")
        );
        // list/delete are brand-new ops with no retired predecessor.
        assert_eq!(ev.legacy_tool_for_action("list"), None);
        assert_eq!(ev.legacy_tool_for_action("delete"), None);
    }

    #[test]
    fn phase5g_models_and_repositories_migrated() {
        // models — LLM tool name stays `manage_models`; the built schema must
        // reproduce the old hand-written tool (actions + properties), and the CLI
        // gets list/add/update/delete.
        let models = domains().iter().find(|d| d.name == "models").unwrap();
        assert_eq!(models.tool_name, "manage_models");
        assert!(models.llm && models.cli && !models.sdk);
        assert_eq!(
            models.actions(),
            vec!["list", "add", "enable", "disable", "update", "remove"]
        );
        let cli_ops: Vec<&str> = models
            .operations
            .iter()
            .filter(|o| o.on_cli(models))
            .map(|o| o.cli_name)
            .collect();
        assert_eq!(cli_ops, vec!["list", "add", "update", "delete"]);
        // The migrated LLM name resolves; the built schema has the same property
        // set the old get_manage_models_tool exposed.
        let tool = build_llm_tool(models);
        assert_eq!(tool.name, "manage_models");
        let props = &tool.parameters["properties"];
        for p in [
            "action",
            "id",
            "label",
            "provider",
            "sort_order",
            "context_window",
            "routes",
            "preferred_provider",
            "vision",
        ] {
            assert!(
                props.get(p).is_some(),
                "manage_models missing property `{p}`"
            );
        }
        assert_eq!(
            props["provider"]["enum"],
            serde_json::json!(MODEL_PROVIDER_ENUM)
        );
        assert_eq!(props["routes"]["type"], "array");
        assert_eq!(
            props["routes"]["items"]["required"],
            serde_json::json!(["provider"])
        );
        assert_eq!(domain_for_tool("manage_models").unwrap().name, "models");

        // repositories: all three verbs on the LLM tool, `list` alone on the
        // CLI. A write verb appearing here means the read-only decision was
        // reversed by accident. See the block comment above `REPO_NAME_ARG`.
        let repos = domains().iter().find(|d| d.name == "repositories").unwrap();
        assert_eq!(repos.tool_name, "manage_repositories");
        assert!(repos.llm && repos.cli && !repos.sdk);
        assert_eq!(repos.actions(), vec!["add", "list", "remove"]);
        let repo_cli_ops: Vec<&str> = repos
            .operations
            .iter()
            .filter(|o| o.on_cli(repos))
            .map(|o| o.cli_name)
            .collect();
        assert_eq!(repo_cli_ops, vec!["list"]);
        let repo_tool = build_llm_tool(repos);
        let repo_props = &repo_tool.parameters["properties"];
        for p in ["action", "name", "path", "description"] {
            assert!(
                repo_props.get(p).is_some(),
                "manage_repositories missing property `{p}`"
            );
        }
        assert_eq!(
            domain_for_tool("manage_repositories").unwrap().name,
            "repositories"
        );
    }

    /// The CLI and SDK read `args`, so the route editing surface lives there:
    /// `routes` on add and update, `preferred_provider` on update.
    #[test]
    fn model_route_args_reach_the_cli() {
        let models = domains().iter().find(|d| d.name == "models").unwrap();
        let args_of = |action: &str| -> Vec<&str> {
            let op = models.operations.iter().find(|o| o.action == action);
            op.unwrap().args.iter().map(|a| a.name).collect()
        };
        assert!(args_of("add").contains(&"routes"));
        assert!(args_of("update").contains(&"routes"));
        assert!(args_of("update").contains(&"preferred_provider"));
        // `routes` alone can add a model, so `provider` must not be required.
        let add = models.operations.iter().find(|o| o.action == "add");
        let provider = add.unwrap().args.iter().find(|a| a.name == "provider");
        assert!(!provider.unwrap().required);
    }

    /// The trigger provider pin reaches every surface: CLI and SDK through
    /// `args`, the LLM tool through the create schema.
    #[test]
    fn trigger_provider_pin_reaches_every_surface() {
        let triggers = domains().iter().find(|d| d.name == "triggers").unwrap();
        for action in ["create", "update"] {
            let op = triggers.operations.iter().find(|o| o.action == action);
            let arg = op.unwrap().args.iter().find(|a| a.name == "provider");
            assert_eq!(
                arg.expect("provider arg").enum_values,
                MODEL_PROVIDER_ENUM,
                "{action}"
            );
        }
        let tool = build_llm_tool(triggers);
        let provider = &tool.parameters["properties"]["provider"];
        assert_eq!(
            provider["anyOf"][1]["enum"],
            serde_json::json!(MODEL_PROVIDER_ENUM)
        );
    }

    /// One provider list: the manifest's enum is `ProviderKind::ALL`, and every
    /// raw schema that spells the list out spells exactly it.
    #[test]
    fn provider_enums_match_provider_kind() {
        let names: Vec<&str> = crate::llm::ProviderKind::ALL
            .iter()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(MODEL_PROVIDER_ENUM, names.as_slice());
        let spelled = serde_json::to_string(MODEL_PROVIDER_ENUM).unwrap();
        for schema in [TRIGGER_CREATE_LLM_SCHEMA, MODELS_ADD_LLM_SCHEMA] {
            assert!(schema.contains("\"vertex\""), "the schema names providers");
            for (i, _) in schema.match_indices("\"vertex\"") {
                assert!(
                    schema[..i].ends_with('[') && schema[i..].starts_with(&spelled[1..]),
                    "a provider enum drifted from ProviderKind::ALL"
                );
            }
        }
    }
}
