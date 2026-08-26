use super::*;
use crate::llm::tool_names as tn;

/// `todo_write` is registered on the chat-agent tool list (Lucidos Agent's
/// runtime todo list — chat + trigger surfaces). NOT registered on the CC
/// tool list, which has its own native `TodoWrite`.
#[test]
fn todo_write_is_in_chat_agent_default_tools() {
    let tools = get_default_tools(&ToolCapabilities::all_open());
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
    let tools = get_default_tools(&ToolCapabilities::all_open());
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
    let tools = get_default_tools(&ToolCapabilities::all_open());
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

/// Regression guard for the reported card: a timezone question whose fourth
/// option was "Other, I'll type it". There is no text-entry option kind, so
/// `answer_kind_to_hook_value` resolves that pick to its LABEL and the model gets
/// "Other, I'll type it" as the user's decision. The description used to say
/// "Always include an option that lets the user opt out", which is what
/// produced the button, so the ban has to replace it rather than sit beside it.
#[test]
fn ask_user_question_description_bans_a_text_entry_escape_option() {
    let tools = get_default_tools(&ToolCapabilities::all_open());
    let desc = &tools
        .iter()
        .find(|t| t.name == tn::ASK_USER_QUESTION)
        .expect("ask_user_question must be in the default chat tool set")
        .description;

    for needle in [
        "NEVER add an \"Other\"",
        "no text-entry option",
        "prompt textarea",
        "Cancel dismisses the question",
    ] {
        assert!(
            desc.contains(needle),
            "description must ban the escape-hatch option and name the real escapes \
             (missing: {needle:?}):\n{desc}"
        );
    }
    assert!(
        !desc.contains("Always include an option that lets the user opt out"),
        "the opt-out instruction must be gone, it is what produced the button:\n{desc}"
    );
    // The ban is about TEXT-ENTRY escapes only. An option carrying a decision
    // the agent can act on stays legal, or the agent drops real Cancel choices.
    assert!(
        desc.contains("None of these") && desc.contains("still welcome"),
        "description must keep a meaningful opt-out option legal:\n{desc}"
    );
}

/// Each question carries the CC-equivalent fields: `question` text, an
/// `options` array of `{label, description?}`, and an optional
/// `multiSelect` flag. `header` is the short chip CC uses to label the
/// question in the UI; we accept it too for parity even though Lucidos
/// doesn't currently render it.
#[test]
fn ask_user_question_per_question_schema_matches_cc() {
    let tools = get_default_tools(&ToolCapabilities::all_open());
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
    let item_required_names: Vec<&str> = item_required.iter().filter_map(|v| v.as_str()).collect();
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

/// `run_python` was for a long time the only one of the four exec tools whose
/// description named no limit, while its three siblings all pointed at
/// "run_python's 300s sync ceiling" that the python path did not actually
/// enforce. The mechanism exists now (`runtime::python`), so the tool that
/// owns it has to say so: an agent that only learns about the ceiling by
/// being killed at it has already burned the turn.
#[test]
fn run_python_states_its_hard_ceiling_and_the_escape_hatch() {
    let tools = get_default_tools(&ToolCapabilities::all_open());
    let tool = tools
        .iter()
        .find(|t| t.name == tn::RUN_PYTHON)
        .expect("run_python must be in get_default_tools()");
    let desc = tool.description.as_str();

    assert!(
        desc.contains(&format!("{}s", super::MAX_TIMEOUT_SECS)),
        "the description must name the ceiling it enforces: {desc:?}"
    );
    assert!(
        desc.contains("run_python_background"),
        "the description must name the tool to use for longer work: {desc:?}"
    );
    // `timeout_secs` is deliberately NOT on this tool: the ceiling is fixed,
    // and advertising a knob the handler ignores is worse than silence.
    let props = tool
        .parameters
        .get("properties")
        .expect("run_python must declare properties");
    assert!(
        props.get("timeout_secs").is_none(),
        "run_python's ceiling is fixed, so it must not advertise timeout_secs"
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
    let tools = get_default_tools(&ToolCapabilities::all_open());
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
    let tools = get_default_tools(&ToolCapabilities::all_open());
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

    // The terminal contract, which is what the drain-at-completion fix makes
    // honest. A drain landing at the instant a task finished used to return
    // `unknown task_id`, so the agent lost successful work; the registry now
    // retains the completion. Both halves of the promise have to survive a
    // reword: the shape the agent looks for, and the instruction to stop once
    // it sees it.
    assert!(
        desc.contains("finished=true"),
        "description must name the finished=true terminal shape: {desc:?}"
    );
    assert!(
        desc.contains("STOP polling"),
        "description must still tell the agent to stop at finished=true: {desc:?}"
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
            kind,
            desc
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
    let default_tools = get_default_tools(&ToolCapabilities::all_open());
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
        let n = grouped_names
            .iter()
            .filter(|g| g.as_str() == grouped)
            .count();
        assert_eq!(
            n, 1,
            "grouped tool {grouped} must be contributed exactly once, got {n}"
        );
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
        tn::FOLLOW_UP_CHILD_THREAD,
    ] {
        assert!(
            default_names.contains(&standalone),
            "hot single-purpose tool {standalone} must remain standalone in get_default_tools()"
        );
    }
}

/// The grouped `events` tool consolidated emit, query, count and event_types.
/// Pin its action enum and that the flat names still resolve as aliases.
///
/// `event_types` is load-bearing for a subscription: a name outside the engine's
/// set is refused, and this action is where the caller reads the real names.
#[test]
fn events_grouped_tool_exposes_every_action() {
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
    assert_eq!(action_names, vec!["emit", "query", "count", "event_types"]);
    for alias in [
        tn::EMIT_EVENT,
        tn::QUERY_EVENTS,
        tn::COUNT_EVENTS,
        tn::LIST_EVENT_TYPES,
    ] {
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
        "max_event_trigger_depth",
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

/// A delivery spends the subscription, and the description used to say only the
/// opposite case: "the subscription STAYS LIVE, so do not register it again",
/// which is true of a user message and the exact reverse of what a delivery
/// needs. That was the nearest matching instruction a model read at delivery
/// time, and on 2026-08-06 a live thread duly narrated a re-arm it never
/// performed.
///
/// The two-delivery fork this used to pin is gone with the attached shape (a
/// user message no longer re-opens anything, it just runs a turn), but the
/// hazard is not: a bare "do not register it again" anywhere near the delivery
/// case re-creates it. So the assertion is that the spent-and-resubscribe
/// statement is present AND that the do-not-register clause carries its own
/// scope.
#[test]
fn await_event_description_says_a_delivery_spends_the_subscription() {
    let tools = get_default_tools(&ToolCapabilities::all_open());
    let tool = tools
        .iter()
        .find(|t| t.name == tn::AWAIT_EVENT)
        .expect("await_event must be registered in get_default_tools()");
    let d = &tool.description;

    assert!(
        d.contains("THE SUBSCRIPTION IS SPENT once it delivers"),
        "a delivery consumes the wait, and that has to be stated:\n{d}"
    );
    assert!(
        d.contains("call this again before that turn ends"),
        "the re-subscribe has to name the call AND the deadline:\n{d}"
    );
    assert!(
        d.contains("Saying you will re-subscribe is not re-subscribing"),
        "prose in place of the call is the failure worth naming:\n{d}"
    );
    assert!(
        d.contains("survives it untouched, so do not register those again"),
        "the do-not-register clause must stay scoped to the user-message case, \
         or it reads as advice for the delivery case:\n{d}"
    );
}

/// The arming lookback is only useful if the model knows to read it. It is a
/// REPORT, not a delivery: the subscription watches forward, so a match named in
/// the result will never arrive as a turn, and a model that skims to
/// "Subscribed" ends the turn with the thing unhandled. That is precisely the
/// 2026-08-06 failure, one layer up.
///
/// **The scope of the promise is load-bearing and is asserted here.** The
/// lookback covers a few minutes, so telling the model it need not check at all
/// would be a bigger bug than the one being fixed: it would arm a forward-only
/// wait for something that happened an hour ago, which can never fire and idles
/// to the timeout. The text must close the check-to-arm RACE without excusing
/// the check.
#[test]
fn await_event_description_says_an_already_happened_match_is_reported_not_delivered() {
    let tools = get_default_tools(&ToolCapabilities::all_open());
    let tool = tools
        .iter()
        .find(|t| t.name == tn::AWAIT_EVENT)
        .expect("await_event must be registered in get_default_tools()");
    let d = &tool.description;

    assert!(
        d.contains("still check state before subscribing"),
        "a forward-only watch cannot cover the past, so the check survives:\n{d}"
    );
    assert!(
        d.contains("race between that check and this call"),
        "the race is the only thing the lookback excuses:\n{d}"
    );
    assert!(
        !d.contains("do not have to check first"),
        "an unscoped 'no need to check' arms waits that can never fire:\n{d}"
    );
    assert!(
        d.contains("WATCHES FORWARD ONLY"),
        "why a reported match will never reach it:\n{d}"
    );
    assert!(
        d.contains("report, not a delivery"),
        "the model owes the report an action inside this turn:\n{d}"
    );
}

/// The description promises a bound because there is one. A model that offers
/// to watch "forever" in a chat thread is wrong twice: the cap refuses the next
/// call, and an unbounded standing rule is a trigger's job.
#[test]
fn await_event_description_names_the_real_subscription_cap() {
    let tools = get_default_tools(&ToolCapabilities::all_open());
    let tool = tools
        .iter()
        .find(|t| t.name == tn::AWAIT_EVENT)
        .expect("await_event must be registered in get_default_tools()");
    let cap = crate::engine::event_wait::MAX_CONSECUTIVE_SUBSCRIPTIONS;

    assert!(
        tool.description
            .contains(&format!("After {cap} subscriptions in a row")),
        "the cap must be interpolated from MAX_CONSECUTIVE_SUBSCRIPTIONS, not \
         restated as a literal that drifts from the refusal the model actually \
         hits:\n{}",
        tool.description
    );
}

/// A direct child already re-opens its parent when it finishes (ADR 0011), so a
/// wait on its `ChildThreadCompleted` is redundant: the engine stands the fan-in
/// callback down when a live wait covers the card, and what is left is a spent
/// subscription slot plus a timeout that can fire while the child still works.
/// The use-list used to say "a thread finishing" with no carve-out, which is the
/// nearest matching instruction for exactly this case, and on 2026-08-06 a live
/// thread duly subscribed to its own coding-agent child.
///
/// **The exclusion's REASON is load-bearing and has to read as redundancy.**
/// Matching is workspace-wide, so any thread's completion is a wait that
/// genuinely fires; the matcher side of that is pinned by
/// `a_wait_matches_a_child_completion_belonging_to_another_thread`. Worded as a
/// bare prohibition, the carve-out was read as impossibility instead: a live
/// thread armed a cross-thread `child_thread_id` wait and then told the user,
/// unprompted, that it "may never fire". So the first assertion is the positive
/// claim, and the exclusion is only allowed after it.
#[test]
fn await_event_description_carves_out_the_threads_own_child() {
    let tools = get_default_tools(&ToolCapabilities::all_open());
    let tool = tools
        .iter()
        .find(|t| t.name == tn::AWAIT_EVENT)
        .expect("await_event must be registered in get_default_tools()");
    let d = &tool.description;

    assert!(
        d.contains("MATCHING IS WORKSPACE-WIDE"),
        "watching another thread is a supported use, and saying so is what stops \
         the exclusion below being read as 'a cross-thread wait might not \
         fire':\n{d}"
    );
    assert!(
        d.contains("NOT YOUR OWN CHILD'S"),
        "the redundant case has to be excluded by name, or the fan-in is \
         invisible to the model:\n{d}"
    );
    assert!(
        d.contains("so a wait duplicates it"),
        "the exclusion's reason must be redundancy, never impossibility:\n{d}"
    );
    assert!(
        d.contains("a thread you did not spawn finishing"),
        "the use-list must scope its thread case to a thread the caller did not \
         spawn, or it invites the very call the paragraph below forbids:\n{d}"
    );
    assert!(
        d.contains("child_thread_id"),
        "the legitimate case (a completion that is not your own child's) has to \
         name how to target it, or the exclusion reads as a blanket ban:\n{d}"
    );
}

/// Every built-in name must satisfy the Messages API pattern. One violation
/// rejects the whole request, so this cannot be caught in review or at
/// runtime: it has to fail here, before the name ever ships.
#[test]
fn every_built_in_tool_name_is_wire_safe() {
    let offenders: Vec<String> = get_default_tools(&ToolCapabilities::all_open())
        .into_iter()
        .chain(crate::capability_manifest::llm_tools())
        .map(|t| t.name)
        .filter(|n| !crate::llm::validate::is_wire_safe_tool_name(n))
        .collect();
    assert!(
        offenders.is_empty(),
        "tool names must match ^[a-zA-Z0-9_-]{{1,128}}$, or the Anthropic API \
         rejects every request carrying them: {:?}",
        offenders
    );
}

// ===== Capability gates (ADR 0088) =====

/// The names a workspace is offered, from both registry tables, in wire order.
fn offered_names(caps: &ToolCapabilities) -> Vec<String> {
    get_default_tools(caps)
        .into_iter()
        .chain(chat_tail_tools(caps))
        .map(|t| t.name)
        .collect()
}

/// The four schemas that resolve an account before they do anything.
const MAILBOX_TOOLS: [&str; 4] = [
    tn::SEND_EMAIL,
    tn::READ_EMAILS,
    tn::READ_EMAIL,
    tn::SAVE_EMAIL_ATTACHMENT,
];

/// A workspace with no email account is offered no mailbox schema. That is
/// 2,664 wire characters it pays for on every request of every thread and can
/// never act on.
#[test]
fn a_workspace_with_no_email_account_is_offered_no_mailbox_tool() {
    let names = offered_names(&ToolCapabilities::default());
    for tool in MAILBOX_TOOLS {
        assert!(
            !names.contains(&tool.to_string()),
            "{tool} needs an account to resolve, so a workspace with none must \
             not be offered it"
        );
    }
}

/// `configure_email` is the exception, and it is load-bearing. It is the only
/// writer of the first `email_accounts` row: the settings UI reaches
/// `EmailStore::upsert` only when editing an existing credential. Gate it and
/// a fresh workspace can never set email up at all.
#[test]
fn configure_email_survives_the_gate_so_email_can_be_set_up_at_all() {
    let names = offered_names(&ToolCapabilities::default());
    assert!(
        names.contains(&tn::CONFIGURE_EMAIL.to_string()),
        "gating configure_email makes the first email account unreachable"
    );
}

/// One account opens all five.
#[test]
fn one_email_account_opens_the_whole_family() {
    let caps = ToolCapabilities {
        email_account: true,
        ..ToolCapabilities::default()
    };
    let names = offered_names(&caps);
    for tool in MAILBOX_TOOLS.into_iter().chain([tn::CONFIGURE_EMAIL]) {
        assert!(
            names.contains(&tool.to_string()),
            "{tool} must be offered once the workspace has an account"
        );
    }
}

/// `execute_intent` takes an intent id, so a workspace with no intent has
/// nothing to pass it.
#[test]
fn execute_intent_is_offered_exactly_when_an_intent_exists() {
    assert!(
        !offered_names(&ToolCapabilities::default()).contains(&tn::EXECUTE_INTENT.to_string()),
        "no intent exists, so execute_intent has no argument that resolves"
    );

    let caps = ToolCapabilities {
        intent: true,
        ..ToolCapabilities::default()
    };
    assert!(
        offered_names(&caps).contains(&tn::EXECUTE_INTENT.to_string()),
        "one intent must open the tool that runs it"
    );
}

/// The gate ADR 0088 extends, asserted through the same table as the two it
/// adds rather than through the `if` it used to be.
#[test]
fn generate_image_is_offered_exactly_when_a_provider_is_configured() {
    assert!(
        !offered_names(&ToolCapabilities::default()).contains(&tn::GENERATE_IMAGE.to_string()),
        "no provider can synthesize an image, so nothing may offer to"
    );

    let caps = ToolCapabilities {
        image_provider: true,
        ..ToolCapabilities::default()
    };
    let names = offered_names(&caps);
    assert!(names.contains(&tn::GENERATE_IMAGE.to_string()));
    // Reading an image the thread already has needs no generation provider,
    // so those two must not have been swept up by the same gate.
    for always in [tn::VIEW_IMAGE, tn::SAVE_THREAD_IMAGE] {
        assert!(
            offered_names(&ToolCapabilities::default()).contains(&always.to_string()),
            "{always} reads an existing image and needs no provider"
        );
    }
}

/// Invariant 20. The mode ADDS no tool, and takes one away: the checklist
/// moved into the working understanding, so `todo_write` is shaped out.
///
/// Off, `todo_write` is byte-for-byte the schema every workspace had before the
/// mode landed, and so is every other schema in the array. That is what makes
/// the eval's control arm a real baseline rather than this build wearing a
/// label. Off is also the overwhelmingly common case, and their prose is billed
/// on every request of every thread that sees them.
#[test]
fn the_mode_on_array_is_a_strict_subset_of_the_mode_off_one() {
    // `all_open` is the widest array, which is the mode OFF: the mode closes a
    // family rather than opening one.
    let control = ToolCapabilities::all_open();
    let lean = ToolCapabilities {
        context_mode: true,
        ..ToolCapabilities::all_open()
    };

    let lean_names = offered_names(&lean);
    let control_names = offered_names(&control);
    for name in &lean_names {
        assert!(
            control_names.contains(name),
            "{name} is offered under the mode and nowhere else, so the mode adds a tool"
        );
    }
    assert!(!lean_names.contains(&tn::TODO_WRITE.to_string()));
    assert!(control_names.contains(&tn::TODO_WRITE.to_string()));
    for retired in ["keep_in_context", "scratchpad", "dismiss_from_context"] {
        assert!(
            !control_names.contains(&retired.to_string())
                && !lean_names.contains(&retired.to_string()),
            "{retired} is retired, in both arms"
        );
    }

    // And nothing ELSE moves with the flag. The array is the first cache
    // segment. A schema shifting on it would cost every lean workspace a tier
    // rewrite it was never told about.
    let others = |caps: &ToolCapabilities| {
        serde_json::to_string(
            &get_default_tools(caps)
                .into_iter()
                .chain(chat_tail_tools(caps))
                .filter(|t| t.name != tn::TODO_WRITE)
                .collect::<Vec<_>>(),
        )
        .expect("the tools array serializes")
    };
    assert_eq!(others(&control), others(&lean));
}

/// **The array is a pure function of workspace configuration.** Two callers in
/// one workspace, however they differ, resolve the same capabilities and so
/// must serialize to the same bytes. Anthropic keys the first cache segment on
/// exactly those bytes, and the measured cross-thread warmth depends on it
/// (ADR 0088 decision 2).
///
/// The type carries most of the guarantee: `get_default_tools` takes only
/// `&ToolCapabilities`, which holds nothing thread-shaped. This pins the
/// serialization too, since a builder reading a clock or a thread id would
/// pass the type check and fail here.
#[test]
fn two_callers_in_one_workspace_get_a_byte_identical_array() {
    let wire = |caps: &ToolCapabilities| {
        serde_json::to_string(
            &get_default_tools(caps)
                .into_iter()
                .chain(chat_tail_tools(caps))
                .collect::<Vec<_>>(),
        )
        .expect("the tools array serializes")
    };

    for caps in [
        ToolCapabilities::default(),
        ToolCapabilities::all_open(),
        ToolCapabilities {
            email_account: true,
            intent: false,
            image_provider: true,
            context_mode: false,
        },
    ] {
        assert_eq!(
            wire(&caps),
            wire(&caps),
            "one workspace's array must be byte-identical for every caller"
        );
    }
}

/// Closing a gate must only REMOVE schemas, never move the survivors. The
/// order is the cache prefix, so a reorder costs every workspace a rewrite of
/// the whole tools tier.
#[test]
fn a_closed_gate_removes_schemas_without_reordering_the_survivors() {
    let all = offered_names(&ToolCapabilities::all_open());
    let none = offered_names(&ToolCapabilities::default());

    assert!(none.len() < all.len(), "the gates must remove something");
    let surviving: Vec<&String> = all.iter().filter(|n| none.contains(n)).collect();
    let actual: Vec<&String> = none.iter().collect();
    assert_eq!(
        surviving, actual,
        "a gated array must be the ungated one with rows deleted, in order"
    );
}

/// Every schema the chat agent is offered comes from a table row. A family
/// added without a gate therefore does not reach the model at all. That is
/// ADR 0088's "state its gate, or state that it has none" made structural
/// rather than left to review.
#[test]
fn every_offered_schema_comes_from_a_gated_registry_row() {
    let from_rows: Vec<String> = FAMILIES
        .iter()
        .chain(CHAT_TAIL)
        .flat_map(|(_, build)| build.render(&ToolCapabilities::all_open()))
        .map(|t| t.name)
        .collect();

    assert_eq!(
        from_rows,
        offered_names(&ToolCapabilities::all_open()),
        "a schema reached the array from somewhere other than a registry row"
    );
}

/// The wire order of the array, frozen.
///
/// The order IS the prefix Anthropic keys its first cache segment on. Moving
/// a schema therefore rewrites the whole 27,000-token tier, for every
/// workspace on the build. Nothing else notices: the budget meter sums, the
/// gate tests check membership, and a reorder changes neither.
///
/// This list is a RATCHET, like the schema-size ceilings. Appending a tool is
/// a one-line diff here and a deliberate cache event; a reorder is a large
/// diff, which is the point.
///
/// `keep_in_context` and `scratchpad` left the list when the self-curated
/// context mode stopped adding tools: the keep became a `[KEEP OPEN]` line in
/// the working understanding, and the document became ordinary text. Both were
/// gated on the mode, so a control array is untouched by their going. A lean
/// array pays a rewrite on this build regardless, since the whole mode prompt
/// changed.
const FROZEN_WIRE_ORDER: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "run_python",
    "run_python_background",
    "run_bash",
    "run_bash_background",
    "bash_output",
    "bash_kill",
    "list_files",
    "glob_files",
    "grep_files",
    "copy_file",
    "delete_file",
    "reload_proxy_modules",
    "proxy_request",
    "http_request",
    "import_file",
    "git_clone",
    "get_backup_status",
    "fetch_news",
    "browser_open",
    "browser_extract",
    "browser_click",
    "browser_type",
    "browser_eval",
    "browser_screenshot",
    "browser_close",
    "browser_forget_login",
    "browser_clear_data",
    "web_search",
    "request_credential",
    "configure_email",
    "send_email",
    "read_emails",
    "read_email",
    "save_email_attachment",
    "create_app",
    "list_apps",
    "load_knowhow",
    "refresh_app",
    "capture_app",
    "connect_oauth_account",
    "run_thread",
    "run_coding_agent",
    "follow_up_child_thread",
    "execute_intent",
    "ask_user_question",
    "await_event",
    "list_event_waits",
    "cancel_event_wait",
    // The mode adds no tool now, so nothing sits here but `todo_write`, which
    // the mode takes AWAY. `dismiss_from_context`, `keep_in_context` and
    // `scratchpad` all left, each in its own change.
    "todo_write",
    "send_notification",
    "notifications",
    "preferences",
    "triggers",
    "trigger_groups",
    "events",
    "changes",
    "threads",
    "memory",
    "thread_queue",
    "env_vars",
    "manage_models",
    "manage_repositories",
    "mcp",
    "plugins",
    "navigate_ui",
    "save_thread_image",
    "view_image",
    "generate_image",
];

/// The whole engine-authored array, in the order `chat::process::run` splices
/// it. MCP tools are excluded: they are discovered from running servers, so
/// they are the workspace's own ordering rather than the engine's.
fn chat_wire_order(caps: &ToolCapabilities) -> Vec<String> {
    get_default_tools(caps)
        .into_iter()
        .chain([get_notification_tool()])
        .chain(crate::capability_manifest::llm_tools())
        .chain(chat_tail_tools(caps))
        .map(|t| t.name)
        .collect()
}

#[test]
fn the_wire_order_is_frozen() {
    assert_eq!(
        chat_wire_order(&ToolCapabilities::all_open()),
        FROZEN_WIRE_ORDER,
        "the tools array reordered. That rewrites the first cache segment for \
         every workspace on this build, so it is only ever deliberate: update \
         FROZEN_WIRE_ORDER in the same change, and say why the order moved."
    );
}

/// Exactly which tools may address a *registered repository*, checked as a
/// schema fact rather than as a policy nobody re-reads.
///
/// The three reads plus `edit_file` are the whole surface. `write_file`,
/// `delete_file` and `copy_file` are excluded on purpose: creating and deleting
/// repo files is structural work belonging to `run_coding_agent`, which lands
/// it as a reviewable change (ADR 0093).
#[test]
fn only_the_reads_and_edit_file_accept_a_repo_argument() {
    let tools = get_default_tools(&ToolCapabilities::all_open());
    let takes_repo = |name: &str| {
        tools
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name} must be registered"))
            .parameters
            .get("properties")
            .and_then(|p| p.get("repo"))
            .is_some()
    };

    for name in [tn::READ_FILE, tn::GLOB_FILES, tn::GREP_FILES, tn::EDIT_FILE] {
        assert!(takes_repo(name), "{name} must accept `repo`");
    }
    for name in [tn::WRITE_FILE, tn::DELETE_FILE, tn::COPY_FILE] {
        assert!(
            !takes_repo(name),
            "{name} must NOT accept `repo`: a repo write goes through run_coding_agent"
        );
    }
}

/// `commit` belongs to `edit_file` alone, and its description has to state the
/// consequence. The agent is about to modify a checkout the user has open, and
/// the only record is the working tree.
#[test]
fn edit_file_commit_flag_states_that_nothing_is_committed() {
    let tools = get_default_tools(&ToolCapabilities::all_open());
    let edit = tools.iter().find(|t| t.name == tn::EDIT_FILE).unwrap();
    let commit = edit
        .parameters
        .get("properties")
        .and_then(|p| p.get("commit"))
        .expect("edit_file must expose `commit`");
    let desc = commit
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    assert_eq!(commit.get("type").and_then(|v| v.as_str()), Some("boolean"));
    assert!(desc.contains("false"), "got: {desc:?}");
    assert!(desc.contains("commits nothing"), "got: {desc:?}");
    assert!(desc.contains("git diff"), "got: {desc:?}");

    for name in [tn::WRITE_FILE, tn::COPY_FILE, tn::DELETE_FILE] {
        let tool = tools.iter().find(|t| t.name == name).unwrap();
        assert!(
            tool.parameters
                .get("properties")
                .and_then(|p| p.get("commit"))
                .is_none(),
            "{name} must not offer `commit`: everything it writes is committed"
        );
    }
}

/// The `repo` argument is spelled once (`repo_arg`) so four schemas cannot
/// drift. This pins that they really are identical.
#[test]
fn every_repo_argument_is_the_same_schema() {
    let tools = get_default_tools(&ToolCapabilities::all_open());
    let schema_for = |name: &str| {
        tools
            .iter()
            .find(|t| t.name == name)
            .unwrap()
            .parameters
            .get("properties")
            .and_then(|p| p.get("repo"))
            .cloned()
            .unwrap_or_else(|| panic!("{name} must accept `repo`"))
    };

    let baseline = schema_for(tn::READ_FILE);
    for name in [tn::GLOB_FILES, tn::GREP_FILES, tn::EDIT_FILE] {
        assert_eq!(schema_for(name), baseline, "{name} drifted from read_file");
    }
    let desc = baseline
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        desc.contains("manage_repositories"),
        "the arg must name how to discover a repo: {desc:?}"
    );
}
