use super::*;
use crate::llm::tool_names as tn;

/// `todo_write` is registered on the chat-agent tool list (Lucidos Agent's
/// runtime todo list — chat + trigger surfaces). NOT registered on the CC
/// tool list, which has its own native `TodoWrite`.
#[test]
fn todo_write_is_in_chat_agent_default_tools() {
    let tools = get_default_tools();
    let found = tools.iter().find(|t| t.name == tn::TODO_WRITE);
    assert!(
        found.is_some(),
        "todo_write must be registered in get_default_tools()",
    );
    let tool = found.unwrap();
    let todos_schema = tool
        .parameters
        .get("properties")
        .and_then(|v| v.get("todos"))
        .expect("todo_write schema must expose a `todos` array");
    assert_eq!(
        todos_schema.get("type").and_then(|v| v.as_str()),
        Some("array"),
        "todos must be an array",
    );
    let required = tool
        .parameters
        .get("required")
        .and_then(|v| v.as_array())
        .expect("todo_write schema must declare required fields");
    let required_names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        required_names.contains(&"todos"),
        "`todos` must be required, got: {:?}",
        required_names,
    );
}

#[test]
fn load_knowhow_schema_example_resolves_to_shipped_knowhow() {
    let tools = get_default_tools();
    let tool = tools
        .iter()
        .find(|t| t.name == tn::LOAD_KNOWHOW)
        .expect("load_knowhow must be in the default chat tool set");
    let id_description = tool
        .parameters
        .get("properties")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.get("description"))
        .and_then(|v| v.as_str())
        .expect("load_knowhow.id must have a description");

    let removed_workspace_specific_example = ["lucidos", "cross-workspace"].join("/");
    assert!(
        !id_description.contains(&removed_workspace_specific_example),
        "schema must not point at the removed workspace-specific example: {id_description:?}"
    );

    let example = "system-knowhow/best-practices";
    assert!(
        id_description.contains(example),
        "schema should advertise a shipped knowhow example, got: {id_description:?}"
    );

    let system_id = example
        .strip_prefix(crate::core::knowhow::SYSTEM_KNOWHOW_PREFIX)
        .expect("example must use the system knowhow prefix");
    let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
    let doc = crate::core::SystemKnowhowStore::load(&repo.join("system-knowhow"), system_id);
    assert!(
        doc.is_some(),
        "load_knowhow schema example must resolve to a shipped system knowhow doc"
    );
}

/// Plugin management is the grouped `plugins` manifest tool (it consolidated the
/// five flat plugin tools). It's spliced from `capability_manifest::llm_tools()`,
/// not `get_default_tools()` — assert the grouped tool exposes the
/// register_marketplace action and its `source` property.
#[test]
fn plugins_grouped_tool_exposes_register_marketplace() {
    let tools = crate::capability_manifest::llm_tools();
    let tool = tools
        .iter()
        .find(|t| t.name == tn::PLUGINS)
        .expect("grouped `plugins` tool must be contributed by the manifest");

    let props = tool
        .parameters
        .get("properties")
        .expect("plugins tool must declare properties");
    let actions = props
        .get("action")
        .and_then(|a| a.get("enum"))
        .and_then(|v| v.as_array())
        .expect("plugins tool must declare an action enum");
    let action_names: Vec<&str> = actions.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        action_names.contains(&"register_marketplace"),
        "plugins tool must expose register_marketplace action, got: {action_names:?}"
    );
    assert!(
        props.get("source").is_some(),
        "plugins tool must accept a `source` property"
    );

    // The retired flat name still resolves to the plugins domain (back-compat).
    let domain = crate::capability_manifest::domain_for_tool("register_plugin_marketplace")
        .expect("register_plugin_marketplace alias must resolve");
    assert_eq!(domain.name, "plugins");
}

/// The chat agent's `ask_user_question` MUST expose the same schema CC's
/// `AskUserQuestion` does, because the same engine-side parser
/// (`parse_ask_user_question_inputs`) consumes both. Drift here would make
/// the chat tool either parse-fail or render question cards without
/// buttons.
#[test]
fn ask_user_question_tool_is_in_default_set() {
    let tools = get_default_tools();
    let tool = tools
        .iter()
        .find(|t| t.name == tn::ASK_USER_QUESTION)
        .expect("ask_user_question must be in the default chat tool set");

    // Top-level shape: an array of `questions`.
    let props = tool
        .parameters
        .get("properties")
        .expect("schema must have properties");
    let questions = props
        .get("questions")
        .expect("schema must expose `questions` array (mirrors CC's AskUserQuestion)");
    assert_eq!(
        questions.get("type").and_then(|v| v.as_str()),
        Some("array"),
        "`questions` must be an array"
    );

    let required = tool
        .parameters
        .get("required")
        .and_then(|v| v.as_array())
        .expect("schema must have required array");
    let required_names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        required_names.contains(&"questions"),
        "`questions` must be required, got: {:?}",
        required_names
    );
}

/// Each question carries the CC-equivalent fields: `question` text, an
/// `options` array of `{label, description?}`, and an optional
/// `multiSelect` flag. `header` is the short chip CC uses to label the
/// question in the UI; we accept it too for parity even though Lucidos
/// doesn't currently render it.
#[test]
fn ask_user_question_per_question_schema_matches_cc() {
    let tools = get_default_tools();
    let tool = tools
        .iter()
        .find(|t| t.name == tn::ASK_USER_QUESTION)
        .expect("ask_user_question must be in the default chat tool set");
    let item_schema = tool
        .parameters
        .get("properties")
        .and_then(|p| p.get("questions"))
        .and_then(|q| q.get("items"))
        .expect("`questions` must declare `items`");
    let item_props = item_schema
        .get("properties")
        .expect("each question item must have `properties`");
    for field in ["question", "options", "multiSelect", "header"] {
        assert!(
            item_props.get(field).is_some(),
            "per-question schema missing `{}` — must match CC's AskUserQuestion",
            field
        );
    }

    // Per-question required: at minimum `question` and `options`.
    let item_required = item_schema
        .get("required")
        .and_then(|v| v.as_array())
        .expect("each question item must declare `required`");
    let item_required_names: Vec<&str> =
        item_required.iter().filter_map(|v| v.as_str()).collect();
    for must in ["question", "options"] {
        assert!(
            item_required_names.contains(&must),
            "per-question required must include `{}`, got: {:?}",
            must,
            item_required_names
        );
    }

    // Each option carries `label` and optional `description` — the shape
    // QuestionCard renders verbatim.
    let option_schema = item_props
        .get("options")
        .and_then(|o| o.get("items"))
        .expect("`options` must declare `items`");
    let option_props = option_schema
        .get("properties")
        .expect("each option must have `properties`");
    for field in ["label", "description"] {
        assert!(
            option_props.get(field).is_some(),
            "option schema missing `{}` — QuestionCard reads both",
            field
        );
    }
    let option_required = option_schema
        .get("required")
        .and_then(|v| v.as_array())
        .expect("each option must declare `required`");
    let option_required_names: Vec<&str> =
        option_required.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        option_required_names.contains(&"label"),
        "option `label` must be required, got: {:?}",
        option_required_names
    );
}

/// `run_python_background` mirrors `run_bash_background`'s task_id /
/// drain / kill contract, but lifts the venv + `packages` auto-install
/// from `run_python`. Pin its registration + schema shape so a refactor
/// that drops the tool, renames its args, or makes `code` optional trips
/// this test instead of leaving the LLM unable to spawn long-running
/// scientific-python workloads.
#[test]
fn run_python_background_is_in_chat_agent_default_tools() {
    let tools = get_default_tools();
    let tool = tools
        .iter()
        .find(|t| t.name == tn::RUN_PYTHON_BACKGROUND)
        .expect("run_python_background must be in get_default_tools()");

    // `code` must be a required string — without it the tool has nothing
    // to spawn. `packages` and `timeout_secs` are optional (zero-package
    // scripts are valid; default timeout is BG_DEFAULT_TIMEOUT_SECS).
    let props = tool
        .parameters
        .get("properties")
        .expect("run_python_background must declare `properties`");
    let code = props
        .get("code")
        .expect("run_python_background must accept `code`");
    assert_eq!(
        code.get("type").and_then(|v| v.as_str()),
        Some("string"),
        "code must be a string"
    );
    assert!(
        props.get("packages").is_some(),
        "run_python_background must accept `packages`"
    );
    assert!(
        props.get("timeout_secs").is_some(),
        "run_python_background must accept `timeout_secs`"
    );

    let required = tool
        .parameters
        .get("required")
        .and_then(|v| v.as_array())
        .expect("run_python_background must declare `required`");
    let required_names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        required_names.contains(&"code"),
        "`code` must be required, got: {:?}",
        required_names
    );

    // The description must steer the LLM toward this tool for the gap
    // that motivated it: scientific-python tasks that may exceed
    // run_python's 300s sync ceiling. Without these triggers in the
    // text, the LLM will keep reaching for run_python and getting
    // killed mid-stream.
    let desc = tool.description.as_str();
    assert!(
        desc.contains("background"),
        "description must mention 'background' so LLM picks vs run_python: {desc:?}"
    );
    assert!(
        desc.contains("task_id"),
        "description must mention task_id so LLM knows to drain: {desc:?}"
    );
    assert!(
        desc.contains("bash_output"),
        "description must reference bash_output as the drain tool: {desc:?}"
    );
}

/// `bash_output(wait_secs)` is the server-side block that replaces the
/// chat-agent sleep-poll antipattern (spawn `run_python_background`,
/// then issue a fresh `run_python` with `time.sleep(N)` — two wasted
/// tool calls per wait). Pin the schema + description steer so a
/// refactor that drops the param or weakens the wording fails here
/// instead of silently re-enabling the antipattern.
#[test]
fn bash_output_schema_advertises_wait_secs_and_steers_away_from_sleep_poll() {
    let tools = get_default_tools();
    let tool = tools
        .iter()
        .find(|t| t.name == tn::BASH_OUTPUT)
        .expect("bash_output must be in get_default_tools()");

    let props = tool
        .parameters
        .get("properties")
        .expect("bash_output must declare properties");
    let wait = props
        .get("wait_secs")
        .expect("bash_output must advertise the wait_secs param so the LLM stops sleep-polling");
    assert_eq!(
        wait.get("type").and_then(|v| v.as_str()),
        Some("integer"),
        "wait_secs must be an integer (clamped at the dispatcher)"
    );

    // wait_secs is optional — the legacy non-blocking drain still
    // works without it.
    let required = tool
        .parameters
        .get("required")
        .and_then(|v| v.as_array())
        .expect("bash_output must declare required");
    let required_names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        !required_names.contains(&"wait_secs"),
        "wait_secs must remain optional, got: {:?}",
        required_names
    );

    let desc = tool.description.as_str();
    assert!(
        desc.contains("wait_secs"),
        "description must reference wait_secs by name so the LLM finds it: {desc:?}"
    );
    assert!(
        desc.contains("time.sleep"),
        "description must explicitly warn against the time.sleep poll antipattern: {desc:?}"
    );
    assert!(
        desc.contains("run_python"),
        "description must name run_python as the wrong place to poll: {desc:?}"
    );
}

/// The chat tool's declared schema must round-trip through the same parser
/// CC's hook uses — guarantees zero drift in what the engine actually
/// understands.
#[test]
fn ask_user_question_schema_parses_with_cc_parser() {
    use crate::engine::agent_session::parse_ask_user_question_inputs;
    let sample = serde_json::json!({
        "questions": [
            {
                "question": "Which approach should I take?",
                "header": "Approach",
                "options": [
                    { "label": "Approach A", "description": "fast" },
                    { "label": "Approach B" }
                ],
                "multiSelect": false
            }
        ]
    });
    let parsed = parse_ask_user_question_inputs(&sample);
    assert_eq!(
        parsed.len(),
        1,
        "single question round-trip; got: {:?}",
        parsed
    );
    assert_eq!(parsed[0].question, "Which approach should I take?");
    assert_eq!(parsed[0].options.len(), 2);
    assert_eq!(parsed[0].options[0].label, "Approach A");
    assert_eq!(parsed[0].options[0].description.as_deref(), Some("fast"));
    assert_eq!(parsed[0].options[1].label, "Approach B");
    assert!(parsed[0].options[1].description.is_none());
    assert!(!parsed[0].multi_select);
}

/// The `tap` param in the `send_notification` tool schema is a structured
/// `{kind, to?}` object (not an enum-of-strings) — see the locked plan-mode
/// design. Verify the schema's `tap` property advertises type=object and the
/// two supported `kind` values in its description so the LLM emits valid taps.
/// (The passive `none` kind was retired —
/// docs/plans/2026-07-02-remove-notification-tap-none.md — so the schema must
/// NOT advertise it.)
#[test]
fn send_notification_schema_tap_documents_object_shape() {
    let tool = get_notification_tool();
    let tap_schema = tool
        .parameters
        .get("properties")
        .and_then(|p| p.get("tap"))
        .expect("send_notification schema must expose `tap`");
    assert_eq!(
        tap_schema.get("type").and_then(|v| v.as_str()),
        Some("object"),
        "tap must be advertised as an object, not a string"
    );
    let desc = tap_schema
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    for kind in ["modal", "navigate"] {
        assert!(
            desc.contains(kind),
            "tap description must mention the `{}` kind so the LLM knows it's valid: got {}",
            kind, desc
        );
    }
    assert!(
        !desc.contains("\"none\""),
        "tap description must NOT advertise the retired `none` kind: got {desc}"
    );
}

/// The delivery semantics (notifications.md §2/§4) must be in the tool
/// description so the agent communicates accurately: an active device gets
/// an in-app TOAST and the OS push is suppressed everywhere; the push fires
/// only when no device is active. Without this steer the agent tells a user
/// who is clearly using the app to "check your device for the push" — the
/// exact mistake that motivated this (the user was active, so no push was
/// ever sent, only a toast).
#[test]
fn send_notification_description_explains_active_device_gets_toast_not_push() {
    let desc = get_notification_tool().description;
    assert!(
        desc.contains("toast"),
        "description must mention the in-app toast surface: {desc:?}"
    );
    assert!(
        desc.contains("suppressed"),
        "description must say the OS push is suppressed when a device is active: {desc:?}"
    );
    assert!(
        desc.contains("check your device for the push"),
        "description must warn against telling an active user to check for a push: {desc:?}"
    );
}

#[test]
fn tap_modal_round_trips_as_kind_object() {
    use crate::scheduler::notifications::Tap;

    let serialized = serde_json::to_value(Tap::Modal).expect("serialize");
    assert_eq!(serialized, serde_json::json!({"kind": "modal"}));

    let parsed: Tap = serde_json::from_value(serde_json::json!({"kind": "modal"})).expect("parse");
    assert_eq!(parsed, Tap::Modal);
}

#[test]
fn tap_legacy_none_coerces_to_modal() {
    // `Tap::None` was retired (docs/plans/2026-07-02-remove-notification-tap-none.md).
    // Historical `{"kind":"none"}` event/row payloads must still deserialize — as
    // Modal — so a projection rebuild never fails or re-emits a `none` row.
    use crate::scheduler::notifications::Tap;

    let parsed: Tap = serde_json::from_value(serde_json::json!({"kind": "none"})).expect("parse");
    assert_eq!(parsed, Tap::Modal);
    assert_eq!(
        serde_json::to_value(&parsed).expect("serialize"),
        serde_json::json!({"kind": "modal"})
    );
}

/// `list_changes` / `apply_change` consolidated into the grouped `changes`
/// manifest tool (action enum list/apply, exposing `change_id` for apply). It's
/// spliced from `capability_manifest::llm_tools()`. Pin the grouped tool's shape
/// and that the flat names still resolve as back-compat aliases.
#[test]
fn changes_grouped_tool_exposes_list_and_apply() {
    let tools = crate::capability_manifest::llm_tools();
    let tool = tools
        .iter()
        .find(|t| t.name == tn::CHANGES)
        .expect("grouped `changes` tool must be contributed by the manifest");

    let props = tool
        .parameters
        .get("properties")
        .expect("changes tool must declare properties");
    let action_names: Vec<&str> = props
        .get("action")
        .and_then(|a| a.get("enum"))
        .and_then(|v| v.as_array())
        .expect("changes tool must declare an action enum")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(action_names, vec!["list", "apply"]);
    let change_id = props
        .get("change_id")
        .expect("changes tool must expose a `change_id` property for apply");
    assert_eq!(
        change_id.get("type").and_then(|v| v.as_str()),
        Some("string"),
        "change_id must be a string"
    );

    // Flat aliases still resolve to the changes domain (back-compat).
    for alias in [tn::LIST_CHANGES, tn::APPLY_CHANGE] {
        let domain = crate::capability_manifest::domain_for_tool(alias)
            .unwrap_or_else(|| panic!("alias {alias} must resolve"));
        assert_eq!(domain.name, "changes");
    }
}

/// Active-consolidation guard: the flat per-verb tools that were folded into
/// grouped manifest tools must NOT reappear in `get_default_tools()` — otherwise
/// the model would see both the flat tool AND the grouped tool, growing the
/// selection surface the consolidation exists to shrink. The flat names stay
/// wired as dispatch aliases (resolved via the manifest), but are no longer
/// advertised. Each consolidated capability is instead offered as exactly one
/// grouped tool from `capability_manifest::llm_tools()`.
#[test]
fn consolidated_flat_tools_are_not_advertised() {
    let default_tools = get_default_tools();
    let default_names: Vec<&str> = default_tools.iter().map(|t| t.name.as_str()).collect();
    let grouped_names: Vec<String> = crate::capability_manifest::llm_tools()
        .iter()
        .map(|t| t.name.clone())
        .collect();

    // Folded flat tools must be absent from the advertised default set.
    for flat in [
        tn::EMIT_EVENT,
        tn::QUERY_EVENTS,
        tn::COUNT_EVENTS,
        tn::LIST_CHANGES,
        tn::APPLY_CHANGE,
        tn::LIST_THREAD_QUEUE,
        tn::UPDATE_THREAD_QUEUE_POLICY,
        tn::CORRECT_MEMORY,
        tn::CORRECT_MEMORY_BY_ID,
        tn::LIST_THREADS,
        tn::COUNT_THREADS,
        tn::SET_ENVIRONMENT_VARIABLE,
    ] {
        assert!(
            !default_names.contains(&flat),
            "{flat} was consolidated into a grouped tool but is still advertised in get_default_tools()"
        );
    }

    // Each grouped tool is contributed exactly once by the manifest.
    for grouped in [
        tn::EVENTS,
        tn::CHANGES,
        tn::THREAD_QUEUE,
        tn::MEMORY,
        tn::THREADS,
        tn::ENV_VARS,
        tn::MCP,
        tn::PLUGINS,
    ] {
        let n = grouped_names.iter().filter(|g| g.as_str() == grouped).count();
        assert_eq!(n, 1, "grouped tool {grouped} must be contributed exactly once, got {n}");
    }

    // The hot single-purpose tools the guardrail protects stay standalone.
    for standalone in [
        tn::READ_FILE,
        tn::WRITE_FILE,
        tn::EDIT_FILE,
        tn::RUN_BASH,
        tn::RUN_PYTHON,
        tn::GREP_FILES,
        tn::RUN_THREAD,
        tn::RUN_CODING_AGENT,
    ] {
        assert!(
            default_names.contains(&standalone),
            "hot single-purpose tool {standalone} must remain standalone in get_default_tools()"
        );
    }
}

/// The grouped `events` tool consolidated emit/query/count. Pin its action enum
/// and that the flat names still resolve as aliases.
#[test]
fn events_grouped_tool_exposes_emit_query_count() {
    let tools = crate::capability_manifest::llm_tools();
    let tool = tools
        .iter()
        .find(|t| t.name == tn::EVENTS)
        .expect("grouped `events` tool must be contributed by the manifest");
    let action_names: Vec<&str> = tool
        .parameters
        .get("properties")
        .and_then(|p| p.get("action"))
        .and_then(|a| a.get("enum"))
        .and_then(|v| v.as_array())
        .expect("events tool must declare an action enum")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(action_names, vec!["emit", "query", "count"]);
    for alias in [tn::EMIT_EVENT, tn::QUERY_EVENTS, tn::COUNT_EVENTS] {
        let domain = crate::capability_manifest::domain_for_tool(alias)
            .unwrap_or_else(|| panic!("alias {alias} must resolve"));
        assert_eq!(domain.name, "events");
    }
}

/// Thread Queue read/tune is the grouped `thread_queue` manifest tool (list +
/// update_policy; run-now/drop are CLI-only). Without this surface, the LLM
/// falls back to `curl` against local engine ports. Pin the grouped tool's
/// action enum, every policy field on the update_policy union, and that the flat
/// names still resolve as aliases.
#[test]
fn thread_queue_grouped_tool_exposes_list_and_update_policy() {
    let tools = crate::capability_manifest::llm_tools();
    let tool = tools
        .iter()
        .find(|t| t.name == tn::THREAD_QUEUE)
        .expect("grouped `thread_queue` tool must be contributed by the manifest");

    let props = tool
        .parameters
        .get("properties")
        .expect("thread_queue tool must declare properties");
    let action_names: Vec<&str> = props
        .get("action")
        .and_then(|a| a.get("enum"))
        .and_then(|v| v.as_array())
        .expect("thread_queue tool must declare an action enum")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    // run-now/drop are CLI-only, so the LLM action enum is list + update_policy.
    assert_eq!(action_names, vec!["list", "update_policy"]);

    // The update_policy union contributes every capacity-policy field.
    for field in [
        "max_concurrent_total",
        "max_concurrent_event_trigger",
        "max_concurrent_cron",
        "max_concurrent_sub_thread",
        "max_concurrent_coding_agent",
        "max_concurrent_per_trigger",
        "max_queued_per_trigger",
        "reserved_background",
        "overflow",
    ] {
        assert!(
            props.get(field).is_some(),
            "thread_queue tool schema missing policy field `{field}`"
        );
    }

    for alias in [tn::LIST_THREAD_QUEUE, tn::UPDATE_THREAD_QUEUE_POLICY] {
        let domain = crate::capability_manifest::domain_for_tool(alias)
            .unwrap_or_else(|| panic!("alias {alias} must resolve"));
        assert_eq!(domain.name, "thread_queue");
    }
}

/// env-var management consolidated into the grouped `env_vars` manifest tool
/// (action enum list/set/delete, exposing name+value for set). It's spliced from
/// `capability_manifest::llm_tools()`. Pin the grouped tool's shape and that the
/// retired `set_environment_variable` name still resolves as a back-compat alias.
#[test]
fn env_vars_grouped_tool_exposes_list_set_delete() {
    let tools = crate::capability_manifest::llm_tools();
    let tool = tools
        .iter()
        .find(|t| t.name == tn::ENV_VARS)
        .expect("grouped `env_vars` tool must be contributed by the manifest");

    let props = tool
        .parameters
        .get("properties")
        .expect("env_vars tool must declare properties");
    let action_names: Vec<&str> = props
        .get("action")
        .and_then(|a| a.get("enum"))
        .and_then(|v| v.as_array())
        .expect("env_vars tool must declare an action enum")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(action_names, vec!["list", "set", "delete"]);

    // `set` contributes name + value; `delete` contributes name.
    for field in ["name", "value"] {
        assert!(
            props.get(field).is_some(),
            "env_vars tool schema missing property `{field}`"
        );
    }

    // The retired flat tool still resolves to this domain.
    let domain = crate::capability_manifest::domain_for_tool(tn::SET_ENVIRONMENT_VARIABLE)
        .expect("set_environment_variable alias must resolve");
    assert_eq!(domain.name, "env_vars");
}

#[test]
fn send_notification_schema_exposes_optional_app_id() {
    let tool = get_notification_tool();
    let props = tool
        .parameters
        .get("properties")
        .expect("schema must have properties");
    let app_id = props
        .get("app_id")
        .expect("send_notification must expose app_id so the LLM can deep-link the push");
    assert_eq!(
        app_id.get("type").and_then(|v| v.as_str()),
        Some("string"),
        "app_id must be a string"
    );
    let required = tool
        .parameters
        .get("required")
        .and_then(|v| v.as_array())
        .expect("schema must have required array");
    let required_names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        !required_names.contains(&"app_id"),
        "app_id must be optional, got required: {:?}",
        required_names
    );
}
