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

#[test]
fn register_plugin_marketplace_is_in_chat_agent_default_tools() {
    let tools = get_default_tools();
    let tool = tools
        .iter()
        .find(|t| t.name == tn::REGISTER_PLUGIN_MARKETPLACE)
        .expect("register_plugin_marketplace must be in the default chat tool set");

    let props = tool
        .parameters
        .get("properties")
        .expect("register_plugin_marketplace must declare properties");
    let source = props
        .get("source")
        .expect("register_plugin_marketplace must accept source");
    assert_eq!(
        source.get("type").and_then(|v| v.as_str()),
        Some("string"),
        "source must be a string"
    );
    assert!(
        props.get("name").is_some(),
        "register_plugin_marketplace must accept optional display name"
    );

    let required = tool
        .parameters
        .get("required")
        .and_then(|v| v.as_array())
        .expect("register_plugin_marketplace schema must declare required fields");
    let required_names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        required_names.contains(&"source"),
        "`source` must be required, got: {:?}",
        required_names,
    );
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
/// three `kind` values in its description so the LLM emits valid taps.
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
    for kind in ["modal", "none", "navigate"] {
        assert!(
            desc.contains(kind),
            "tap description must mention the `{}` kind so the LLM knows it's valid: got {}",
            kind, desc
        );
    }
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
fn tap_none_round_trips_as_kind_object() {
    use crate::scheduler::notifications::Tap;

    let serialized = serde_json::to_value(Tap::None).expect("serialize");
    assert_eq!(serialized, serde_json::json!({"kind": "none"}));

    let parsed: Tap = serde_json::from_value(serde_json::json!({"kind": "none"})).expect("parse");
    assert_eq!(parsed, Tap::None);
}

/// `list_changes` and `apply_change` are the in-thread mirror of the
/// `lucidos changes list` / `lucidos changes apply` CLI subcommands —
/// closing the one CLI⇄LLM-tool parity gap. Pin their registration so a
/// refactor that drops either tool from the chat surface trips here.
#[test]
fn change_tools_are_in_chat_agent_default_tools() {
    let tools = get_default_tools();
    for name in [tn::LIST_CHANGES, tn::APPLY_CHANGE] {
        assert!(
            tools.iter().any(|t| t.name == name),
            "{name} must be registered in get_default_tools()",
        );
    }
}

/// `apply_change` MUST require `change_id` (there's nothing to merge without
/// it) and `list_changes` MUST require nothing (it returns the whole
/// pending+applied set). Drift here would let the LLM call apply with no
/// target or force a needless arg on the read tool.
#[test]
fn apply_change_requires_change_id_and_list_changes_requires_nothing() {
    let tools = get_default_tools();

    let apply = tools
        .iter()
        .find(|t| t.name == tn::APPLY_CHANGE)
        .expect("apply_change must be in the default chat tool set");
    let change_id = apply
        .parameters
        .get("properties")
        .and_then(|p| p.get("change_id"))
        .expect("apply_change must expose a `change_id` property");
    assert_eq!(
        change_id.get("type").and_then(|v| v.as_str()),
        Some("string"),
        "change_id must be a string"
    );
    let apply_required: Vec<&str> = apply
        .parameters
        .get("required")
        .and_then(|v| v.as_array())
        .expect("apply_change must declare required")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        apply_required.contains(&"change_id"),
        "`change_id` must be required, got: {apply_required:?}"
    );

    let list = tools
        .iter()
        .find(|t| t.name == tn::LIST_CHANGES)
        .expect("list_changes must be in the default chat tool set");
    let list_required = list
        .parameters
        .get("required")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    assert!(
        list_required.is_empty(),
        "list_changes must require no args, got: {list_required:?}"
    );
}

/// Thread Queue policy changes need a typed in-process tool. Without this
/// surface, the LLM falls back to `curl` / `http_request` against local engine
/// ports and then tries to read temp files, which is brittle and poorly
/// attributed.
#[test]
fn thread_queue_tools_are_registered_and_policy_patch_is_partial() {
    let tools = get_default_tools();
    for name in [tn::LIST_THREAD_QUEUE, tn::UPDATE_THREAD_QUEUE_POLICY] {
        assert!(
            tools.iter().any(|t| t.name == name),
            "{name} must be registered in get_default_tools()",
        );
    }

    let update = tools
        .iter()
        .find(|t| t.name == tn::UPDATE_THREAD_QUEUE_POLICY)
        .expect("update_thread_queue_policy must be in the default chat tool set");
    assert_eq!(
        update
            .parameters
            .get("minProperties")
            .and_then(|v| v.as_u64()),
        Some(1),
        "update_thread_queue_policy must require at least one patched field"
    );
    let props = update
        .parameters
        .get("properties")
        .expect("update_thread_queue_policy must declare properties");
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
            "update_thread_queue_policy schema missing `{field}`"
        );
    }
    assert!(
        update
            .description
            .contains("Only fields you provide are changed"),
        "description must steer the LLM away from full-policy resets: {:?}",
        update.description
    );
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
