pub use lucidos_capabilities::*;

use crate::llm::provider::ToolDefinition;
use serde_json::{Map, Value};

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

fn op_llm_properties(op: &Operation) -> Vec<(String, Value)> {
    if op.llm_schema.is_some() {
        return parse_llm_schema(op).into_iter().collect();
    }
    op.caller_args()
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

/// INVARIANT: Required-argument names for an operation's "(requires: …)" hint, derived from
/// the required `args`. Skipped for ops with a raw `llm_schema`: their LLM shape
/// can diverge from `args` (different names, e.g. `trigger_id` vs the HTTP query
/// `id`), so the per-property descriptions in the schema carry requiredness
/// instead of a possibly-wrong flat hint.
fn required_arg_names(op: &Operation) -> Vec<String> {
    if op.llm_schema.is_some() {
        return Vec::new();
    }
    op.caller_args()
        .filter(|a| a.required)
        .map(|a| a.name.to_string())
        .collect()
}

pub fn build_llm_tool(domain: &Domain) -> ToolDefinition {
    let llm_ops: Vec<&Operation> = domain
        .operations
        .iter()
        .filter(|o| o.on_llm(domain))
        .collect();

    let mut description = String::from(domain.tool_summary);
    description.push_str("\n\nActions:");
    for op in &llm_ops {
        description.push_str(&format!("\n• {}: {}", op.action, op.summary));
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
    // INVARIANT: Union the per-operation properties. An operation whose LLM shape diverges
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

pub fn llm_tools() -> Vec<ToolDefinition> {
    llm_tools_for(&crate::llm::ToolCapabilities::default())
}

/// INVARIANT: The grouped LLM tools one workspace is offered: every domain whose gate
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
        let widget_sizes: Vec<&str> = crate::core::apps::WidgetSize::ALL
            .iter()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(WIDGET_SIZE_ARG.enum_values, widget_sizes.as_slice());
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
    fn every_fixed_body_value_is_json_of_its_type_and_hidden_from_callers() {
        for d in domains() {
            for op in d.operations {
                for arg in op.args {
                    let ArgIn::FixedBody(literal) = arg.loc else {
                        continue;
                    };
                    let value: serde_json::Value = serde_json::from_str(literal)
                        .unwrap_or_else(|e| panic!("{}.{} {}: {e}", d.name, op.action, arg.name));
                    let json_type = match &value {
                        serde_json::Value::Bool(_) => "boolean",
                        serde_json::Value::Number(n) if n.is_i64() => "integer",
                        serde_json::Value::String(_) => "string",
                        other => panic!("{}.{} {}: {other}", d.name, op.action, arg.name),
                    };
                    assert_eq!(json_type, arg.ty.json_type(), "{}.{}", d.name, op.action);
                    assert!(
                        !op_llm_properties(op)
                            .iter()
                            .any(|(name, _)| name == arg.name)
                            && !required_arg_names(op).contains(&arg.name.to_string()),
                        "{}.{} offers its fixed {} to the caller",
                        d.name,
                        op.action,
                        arg.name
                    );
                }
            }
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
        assert_eq!(
            domain_for_tool("notifications").unwrap().name,
            "notifications"
        );
        assert!(domain_for_tool("nope").is_none());
    }

    #[test]
    fn built_tool_exposes_action_enum_and_all_args() {
        let d = domain_for_tool("notifications").unwrap();
        let tool = build_llm_tool(d);
        assert_eq!(tool.name, "notifications");
        let props = &tool.parameters["properties"];
        assert_eq!(props["action"]["enum"], serde_json::json!(d.actions()));
        for name in ["filter", "limit", "id"] {
            assert!(
                props.get(name).is_some(),
                "expected arg '{name}' in built tool schema"
            );
        }
        assert_eq!(tool.parameters["required"], serde_json::json!(["action"]));
    }

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
        let pause = triggers
            .operations
            .iter()
            .find(|o| o.action == "pause")
            .unwrap();
        assert!(pause.on_llm(triggers) && !pause.on_cli(triggers) && !pause.on_sdk(triggers));
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

        let apps = domains().iter().find(|d| d.name == "apps").unwrap();
        assert!(!apps.llm && apps.cli && apps.sdk);
        assert!(apps.actions().is_empty(), "apps exposes no LLM actions");
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

        assert!(!op("setup").on_cli(mcp));

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
        assert_eq!(tq.actions(), vec!["list", "update_policy"]);
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
        assert_eq!(
            mem.actions(),
            vec!["correct", "correct_by_id", "source", "search"]
        );
        let cli_ops: Vec<&str> = mem
            .operations
            .iter()
            .filter(|o| o.on_cli(mem))
            .map(|o| o.cli_name)
            .collect();
        assert_eq!(
            cli_ops,
            vec!["correct-by-id", "stats", "entries", "source", "search"]
        );
        assert_eq!(domain_for_tool("correct_memory").unwrap().name, "memory");
        assert_eq!(
            mem.legacy_tool_for_action("correct_by_id"),
            Some("correct_memory_by_id")
        );
    }

    #[test]
    fn phase5e_threads_declared() {
        let threads = domains().iter().find(|d| d.name == "threads").unwrap();
        assert_eq!(
            threads.actions(),
            vec![
                "list",
                "count",
                "drafts",
                "submit_draft",
                "held_messages",
                "search",
                "detach_child",
                "archive",
                "cache_keep_alive",
                "context_ceiling",
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
        assert!(domain_for_tool("run_thread").is_none());
        assert!(domain_for_tool("run_coding_agent").is_none());
    }

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
        let submit = threads
            .operations
            .iter()
            .find(|o| o.action == "submit_draft")
            .expect("the threads domain sends a saved draft");
        assert!(submit.mutating && matches!(submit.method, Method::Post));
        assert_eq!(submit.path, "/threads/:thread_id/submit-draft");
        assert_eq!(
            threads.legacy_tool_for_action("submit_draft"),
            Some("submit_saved_draft")
        );
    }

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
        assert!(ev.llm && ev.cli && !ev.sdk);
        assert_eq!(ev.actions(), vec!["list", "set", "delete"]);
        let cli_ops: Vec<&str> = ev
            .operations
            .iter()
            .filter(|o| o.on_cli(ev))
            .map(|o| o.cli_name)
            .collect();
        assert_eq!(cli_ops, vec!["list", "set", "delete"]);
        assert_eq!(
            domain_for_tool("set_environment_variable").unwrap().name,
            "env_vars"
        );
        assert_eq!(
            ev.legacy_tool_for_action("set"),
            Some("set_environment_variable")
        );
        assert_eq!(ev.legacy_tool_for_action("list"), None);
        assert_eq!(ev.legacy_tool_for_action("delete"), None);
    }

    #[test]
    fn phase5g_models_and_repositories_migrated() {
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
        let add = models.operations.iter().find(|o| o.action == "add");
        let provider = add.unwrap().args.iter().find(|a| a.name == "provider");
        assert!(!provider.unwrap().required);
    }

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
