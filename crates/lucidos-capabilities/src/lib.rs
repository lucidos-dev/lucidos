pub mod codegen;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Ungated,
    EmailAccount,
    ImageProvider,
    JudgmentProvider,
    MemoryTree,
    MemoryClassic,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArgType {
    Str,
    Int,
    Bool,
    Json,
}

impl ArgType {
    pub fn json_type(self) -> &'static str {
        match self {
            ArgType::Str => "string",
            ArgType::Int => "integer",
            ArgType::Bool => "boolean",
            ArgType::Json => "object",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArgIn {
    Query,
    Body,
    Path,
    FixedBody(&'static str),
}

#[derive(Clone, Copy)]
pub struct Arg {
    pub name: &'static str,
    pub ty: ArgType,
    pub enum_values: &'static [&'static str],
    pub required: bool,
    pub loc: ArgIn,
    pub description: &'static str,
}

impl Arg {
    pub fn from_caller(&self) -> bool {
        !matches!(self.loc, ArgIn::FixedBody(_))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Operation {
    pub action: &'static str,
    pub summary: &'static str,
    pub method: Method,
    pub path: &'static str,
    pub args: &'static [Arg],
    pub cli_name: &'static str,
    pub sdk_name: &'static str,
    pub mutating: bool,
    pub llm_alias: Option<&'static str>,
    pub llm_schema: Option<&'static str>,
    pub llm: Option<bool>,
    pub cli: Option<bool>,
    pub sdk: Option<bool>,
}

impl Operation {
    pub fn caller_args(&self) -> impl Iterator<Item = &Arg> {
        self.args.iter().filter(|a| a.from_caller())
    }
    pub fn on_llm(&self, domain: &Domain) -> bool {
        self.llm.unwrap_or(domain.llm)
    }
    pub fn on_cli(&self, domain: &Domain) -> bool {
        self.cli.unwrap_or(domain.cli)
    }
    pub fn on_sdk(&self, domain: &Domain) -> bool {
        self.sdk.unwrap_or(domain.sdk)
    }
}

#[derive(Clone, Copy)]
pub struct Domain {
    pub name: &'static str,
    pub tool_name: &'static str,
    pub tool_summary: &'static str,
    pub llm: bool,
    pub cli: bool,
    pub sdk: bool,
    pub operations: &'static [Operation],
    pub llm_aliases: &'static [&'static str],
    pub llm_gate: Gate,
    pub llm_execution: LlmExecution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmExecution {
    Route,
    /// A hand-written in-process handler. Only the legacy domains the engine's
    /// shrink-only list names may say this (`docs/temporary-measures.md`).
    LegacyInProcess,
}

impl Domain {
    pub fn actions(&self) -> Vec<&'static str> {
        self.operations
            .iter()
            .filter(|o| o.on_llm(self))
            .map(|o| o.action)
            .collect()
    }

    pub fn legacy_tool_for_action(&self, action: &str) -> Option<&'static str> {
        self.operations
            .iter()
            .find(|o| o.action == action)
            .and_then(|o| o.llm_alias)
    }

    pub fn alias_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self
            .operations
            .iter()
            .filter(|o| o.on_llm(self))
            .filter_map(|o| o.llm_alias)
            .collect();
        names.extend_from_slice(self.llm_aliases);
        names
    }
}

const FILTER_ARG: Arg = Arg {
    name: "filter",
    ty: ArgType::Str,
    enum_values: &["unread", "all"],
    required: false,
    loc: ArgIn::Query,
    description: "'unread' (default) or 'all'.",
};
const LIMIT_ARG: Arg = Arg {
    name: "limit",
    ty: ArgType::Int,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "1-50, default 20.",
};
const NOTIFICATION_ID_ARG: Arg = Arg {
    name: "id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "UUID from the 'list' action.",
};

const NOTIFICATIONS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "Inbox notifications, unread by default: id, title, message, read, created_at.",
        method: Method::Get,
        path: "/notifications",
        args: &[FILTER_ARG, LIMIT_ARG],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: Some("read_notifications"),
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "mark_read",
        summary: "Mark a single notification read by id.",
        method: Method::Post,
        path: "/notification/read",
        args: &[NOTIFICATION_ID_ARG],
        cli_name: "read",
        sdk_name: "markRead",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "mark_all_read",
        summary: "Mark every unread one read, clearing the inbox badge.",
        method: Method::Post,
        path: "/notifications/read-all",
        args: &[],
        cli_name: "read-all",
        sdk_name: "markAllRead",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
];

const PREF_GET_DEVICE_ID_ARG: Arg = Arg {
    name: "device_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Read device-scoped overrides; omit for the global view.",
};
const PREF_KEY_ARG: Arg = Arg {
    name: "key",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "e.g. 'theme-mode', 'language', 'timezone', 'chat_model'.",
};
const PREF_VALUE_ARG: Arg = Arg {
    name: "value",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "A string: 'true'/'false', '125', or an allowed enum value.",
};
const PREF_SET_DEVICE_ID_ARG: Arg = Arg {
    name: "device_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "For a per-device key; omit for global ones.",
};

const PREFERENCES_OPS: &[Operation] = &[
    Operation {
        action: "get",
        summary: "Every settable key with its current value, allowed values, default and scope (global or per-device).",
        method: Method::Get,
        path: "/preferences",
        args: &[PREF_GET_DEVICE_ID_ARG],
        cli_name: "get",
        sdk_name: "get",
        mutating: false,
        llm_alias: Some("get_preferences"),
        llm_schema: Some("{}"),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "set",
        summary: "Change one preference. Call 'get' first if unsure of the key or its allowed values.",
        method: Method::Put,
        path: "/preferences",
        args: &[PREF_KEY_ARG, PREF_VALUE_ARG, PREF_SET_DEVICE_ID_ARG],
        cli_name: "set",
        sdk_name: "set",
        mutating: true,
        llm_alias: Some("set_preference"),
        llm_schema: Some(
            r#"{
              "key": {"type":"string","description":"e.g. 'theme-mode', 'language', 'timezone', 'chat_model'. The 'get' action lists every settable key."},
              "value": {"type":"string","description":"A string: 'true'/'false', '125', or an allowed enum value from the 'get' action."}
            }"#,
        ),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "reset",
        summary: "Put one global preference back to its default by deleting its stored value.",
        method: Method::Delete,
        path: "/preferences",
        args: &[PREF_KEY_ARG],
        cli_name: "reset",
        sdk_name: "reset",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: Some(false),
        cli: None,
        sdk: Some(false),
    },
];

const PREFERENCES_DOMAIN: Domain = Domain {
    name: "preferences",
    tool_name: "preferences",
    tool_summary: "Read and change user preferences (Settings). A device-scoped key (theme, font, ui-scale, push) applies to the calling device. NOT for secrets (request_credential), chat models (manage_models), or command-safety settings.",
    llm: true,
    cli: true,
    sdk: true,
    operations: PREFERENCES_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::LegacyInProcess,
};

const TRIGGER_ID_QUERY_ARG: Arg = Arg {
    name: "id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "UUID of the trigger.",
};
const TRIGGER_NAME_ARG: Arg = Arg {
    name: "name",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "A short, descriptive name for the trigger.",
};
const TRIGGER_NAME_OPT_ARG: Arg = Arg {
    name: "name",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "New name for the trigger.",
};
const TRIGGER_RUN_ARG: Arg = Arg {
    name: "run",
    ty: ArgType::Json,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "JSON: { \"type\": \"intent\", \"intent\": \"…\" } or { \"type\": \"script\", \"path\": \"name/run.py\" }.",
};
const TRIGGER_RUN_OPT_ARG: Arg = Arg {
    name: "run",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "JSON run config: { \"type\": \"intent\", … } or { \"type\": \"script\", … }.",
};
const TRIGGER_CRON_EXPRESSIONS_ARG: Arg = Arg {
    name: "cron_expressions",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "6-field cron strings in the user's local time, e.g. [\"0 0 8 * * *\"].",
};
const TRIGGER_ON_ARG: Arg = Arg {
    name: "on",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description:
        "Event subscriptions, e.g. [{\"event_type\":\"X\",\"condition\":{\"a.b\":\"c\"}}].",
};
const TRIGGER_PAUSED_ARG: Arg = Arg {
    name: "paused",
    ty: ArgType::Bool,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Pause (true) or resume (false) the trigger.",
};
const TRIGGER_APP_ID_ARG: Arg = Arg {
    name: "app_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Owning app directory name; deep-links notifications to that app.",
};
const TRIGGER_GO_TO_REVIEW_ARG: Arg = Arg {
    name: "go_to_review",
    ty: ArgType::Bool,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Threads this trigger spawns surface in REVIEW on completion, not ARCHIVE.",
};
const TRIGGER_GROUP_ID_ARG: Arg = Arg {
    name: "group_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Trigger-group id this trigger belongs to (organizational only).",
};
const TRIGGER_SIDE_EFFECT_GRANT_ARG: Arg = Arg {
    name: "side_effect_grant",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Send [] to revoke the trigger's legacy category grants.",
};
const TRIGGER_PROGRAM_GRANTS_ARG: Arg = Arg {
    name: "program_grants",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Program names to request on a permission card in this thread.",
};
const TRIGGER_FOLDER_GRANTS_ARG: Arg = Arg {
    name: "folder_grants",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Absolute folder paths to request on a permission card in this thread.",
};
const TRIGGER_REVOKE_PROGRAM_GRANTS_ARG: Arg = Arg {
    name: "revoke_program_grants",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Program grant scopes to revoke, as listed on this trigger.",
};
const TRIGGER_REVOKE_FOLDER_GRANTS_ARG: Arg = Arg {
    name: "revoke_folder_grants",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Folder grants to revoke, as listed on this trigger. Only the owner may.",
};
const TRIGGER_CODING_AGENT_GRANT_ARG: Arg = Arg {
    name: "coding_agent_grant",
    ty: ArgType::Bool,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Whether this trigger's runs may start coding-agent threads. Only the owner may \
                  set true: an agent or app caller is refused. Default false.",
};
const TRIGGER_MCP_SERVERS_ARG: Arg = Arg {
    name: "mcp_servers",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description:
        "MCP server ids its fires use, e.g. [\"slack\"]. Only these run silently in its threads.",
};
const TRIGGER_CONTINUE_KEY_ARG: Arg = Arg {
    name: "continue_key",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Payload paths whose values name one thread: a fire with the same values continues it. A part may list alternatives. e.g. [\"payload.repository.full_name\", [\"payload.issue.number\", \"payload.pull_request.number\"]].",
};
const TRIGGER_ACTOR_ARG: Arg = Arg {
    name: "actor",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Intent triggers: the payload path of the email of the person behind a fire, e.g. \"payload.sender.email\". The fire records it, so the thread credits that operator. Null clears.",
};
const TRIGGER_MODEL_ARG: Arg = Arg {
    name: "model",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description:
        "Chat model id this trigger's intent runs on, a Lucidos Agent ladder rung: an agent's off-ladder pick is refused. Omit or null for the account default.",
};
const TRIGGER_REASONING_EFFORT_ARG: Arg = Arg {
    name: "reasoning_effort",
    ty: ArgType::Str,
    enum_values: &["none", "low", "medium", "high", "xhigh", "max"],
    required: false,
    loc: ArgIn::Body,
    description:
        "Thinking budget for this trigger's intent runs. Omit or null for the account default.",
};
const TRIGGER_PROVIDER_ARG: Arg = Arg {
    name: "provider",
    ty: ArgType::Str,
    enum_values: MODEL_PROVIDER_ENUM,
    required: false,
    loc: ArgIn::Body,
    description: "Backend for the pinned model when it has more than one route. Requires a model pin and must be one of its routes. Omit or null for the model's own preferred provider.",
};
const TRIGGER_SLUG_ARG: Arg = Arg {
    name: "slug",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description:
        "Kebab-case slug, the directory segment for per-trigger knowhow. Derived from name.",
};

pub const TRIGGER_CREATE_LLM_SCHEMA: &str = r#"{
  "name": {"type":"string","description":"Short and descriptive."},
  "run": {"type":"object","description":"{ type: 'intent', intent: '…' } in the user's voice with the procedure left to knowhow, or { type: 'script', path: 'name/run.py' }."},
  "cron": {"description":"6 fields in the USER'S LOCAL TIME (second minute hour day-of-month month day-of-week); '0 0 8 * * *' is 8am daily. Fields AND within one expression, expressions OR across the array. A string, an array, or null.","oneOf":[{"type":"string"},{"type":"array","items":{"type":"string"},"minItems":1},{"type":"null"}]},
  "on": {"description":"Each { event_type: 'X', condition?: {…} }, operators $eq/$ne/$lt/$lte/$gt/$gte/$in/$nin/$regex plus $or. A condition key is a field path: 'workflow_run.event'. A string, an array, or null.","anyOf":[{"type":"null"},{"type":"string"},{"type":"array","items":{"anyOf":[{"type":"string"},{"type":"object","properties":{"event_type":{"type":"string"},"condition":{"type":"object"}},"required":["event_type"]}]}}]},
  "app_id": {"anyOf":[{"type":"null"},{"type":"string"}],"description":"Owning app directory name; notifications deep-link there. Null for standalone."},
  "go_to_review": {"type":"boolean","description":"Threads this trigger spawns land in REVIEW, not ARCHIVE. Default false."},
  "group_id": {"anyOf":[{"type":"null"},{"type":"string"}],"description":"Trigger-group id, organizational only. Null for ungrouped."},
  "program_grants": {"type":"array","items":{"type":"string"},"description":"Programs to ask the owner for, on one card."},
  "folder_grants": {"type":"array","items":{"type":"string"},"description":"Absolute folders to ask the owner for, on one card."},
  "mcp_servers": {"anyOf":[{"type":"null"},{"type":"array","items":{"type":"string"}}],"description":"MCP server ids its fires call; any other is refused."},
  "continue_key": {"anyOf":[{"type":"null"},{"type":"array","items":{"anyOf":[{"type":"string"},{"type":"array","items":{"type":"string"}}]}}],"description":"Intent triggers: payload paths naming one thread; a part may list alternatives. A fire with the same values continues its open thread. Null clears."},
  "actor": {"anyOf":[{"type":"null"},{"type":"string"}],"description":"Intent triggers: payload path of the email of the person behind a fire. The thread credits that operator. Null clears."},
  "model": {"anyOf":[{"type":"null"},{"type":"string"}],"description":"Model id, a Lucidos Agent rung, else REFUSED. Null = account default (on update, clears a pin). Set only if asked."},
  "reasoning_effort": {"anyOf":[{"type":"null"},{"type":"string","enum":["none","low","medium","high","xhigh","max"]}],"description":"Thinking budget for the intent. Null = account default (on update, clears a pin)."},
  "provider": {"anyOf":[{"type":"null"},{"type":"string","enum":["vertex","anthropic","openai","openrouter","xai","opencode-free","local","claude-gateway","chatgpt-plan"]}],"description":"Backend for the pinned model, one of its routes; needs model. Null = its preferred provider. Set only if asked."}
}"#;
const TRIGGER_UPDATE_LLM_SCHEMA: &str = r#"{
  "trigger_id": {"type":"string","description":"UUID of the trigger to act on."},
  "paused": {"type":"boolean","description":"Pause/resume inside a multi-field update; prefer the standalone actions."}
}"#;

const TRIGGERS_OPS: &[Operation] = &[
    Operation {
        action: "create",
        summary: "Create a NEW trigger: cron, event-based `on`, or both.",
        method: Method::Post,
        path: "/triggers",
        args: &[
            TRIGGER_NAME_ARG,
            TRIGGER_RUN_ARG,
            TRIGGER_CRON_EXPRESSIONS_ARG,
            TRIGGER_ON_ARG,
            TRIGGER_APP_ID_ARG,
            TRIGGER_GO_TO_REVIEW_ARG,
            TRIGGER_GROUP_ID_ARG,
            TRIGGER_CODING_AGENT_GRANT_ARG,
            TRIGGER_MCP_SERVERS_ARG,
            TRIGGER_PROGRAM_GRANTS_ARG,
            TRIGGER_FOLDER_GRANTS_ARG,
            TRIGGER_CONTINUE_KEY_ARG,
            TRIGGER_ACTOR_ARG,
            TRIGGER_SLUG_ARG,
            TRIGGER_MODEL_ARG,
            TRIGGER_REASONING_EFFORT_ARG,
            TRIGGER_PROVIDER_ARG,
        ],
        cli_name: "create",
        sdk_name: "create",
        mutating: true,
        llm_alias: Some("create_trigger"),
        llm_schema: Some(TRIGGER_CREATE_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "list",
        summary: "Every trigger with its schedule, subscriptions and what it runs.",
        method: Method::Get,
        path: "/triggers",
        args: &[],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: Some("list_triggers"),
        llm_schema: Some("{}"),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "update",
        summary: "Update name, schedule, subscriptions or run config in place, keeping run history. Send the full replacement 'on' array.",
        method: Method::Put,
        path: "/triggers",
        args: &[
            TRIGGER_ID_QUERY_ARG,
            TRIGGER_NAME_OPT_ARG,
            TRIGGER_RUN_OPT_ARG,
            TRIGGER_CRON_EXPRESSIONS_ARG,
            TRIGGER_ON_ARG,
            TRIGGER_PAUSED_ARG,
            TRIGGER_APP_ID_ARG,
            TRIGGER_GO_TO_REVIEW_ARG,
            TRIGGER_GROUP_ID_ARG,
            TRIGGER_SIDE_EFFECT_GRANT_ARG,
            TRIGGER_REVOKE_PROGRAM_GRANTS_ARG,
            TRIGGER_REVOKE_FOLDER_GRANTS_ARG,
            TRIGGER_CODING_AGENT_GRANT_ARG,
            TRIGGER_MCP_SERVERS_ARG,
            TRIGGER_PROGRAM_GRANTS_ARG,
            TRIGGER_FOLDER_GRANTS_ARG,
            TRIGGER_CONTINUE_KEY_ARG,
            TRIGGER_ACTOR_ARG,
            TRIGGER_SLUG_ARG,
            TRIGGER_MODEL_ARG,
            TRIGGER_REASONING_EFFORT_ARG,
            TRIGGER_PROVIDER_ARG,
        ],
        cli_name: "update",
        sdk_name: "update",
        mutating: true,
        llm_alias: Some("update_trigger"),
        llm_schema: Some(TRIGGER_UPDATE_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "delete",
        summary: "Delete a trigger; it orphans the run history, so prefer update for tweaks.",
        method: Method::Delete,
        path: "/triggers",
        args: &[TRIGGER_ID_QUERY_ARG],
        cli_name: "delete",
        sdk_name: "delete",
        mutating: true,
        llm_alias: Some("delete_trigger"),
        llm_schema: Some(r#"{"trigger_id":{"type":"string","description":"UUID of the trigger to delete."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "pause",
        summary: "Stop it firing; config preserved.",
        method: Method::Put,
        path: "/triggers",
        args: &[],
        cli_name: "pause",
        sdk_name: "pause",
        mutating: true,
        llm_alias: Some("pause_trigger"),
        llm_schema: Some(r#"{"trigger_id":{"type":"string","description":"UUID of the trigger to pause."}}"#),
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "resume",
        summary: "Fire on schedule and match events again.",
        method: Method::Put,
        path: "/triggers",
        args: &[],
        cli_name: "resume",
        sdk_name: "resume",
        mutating: true,
        llm_alias: Some("resume_trigger"),
        llm_schema: Some(r#"{"trigger_id":{"type":"string","description":"UUID of the trigger to resume."}}"#),
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "run",
        summary: "Fire it ONCE now, off-schedule. Refused inside a trigger fire, on a paused trigger, and on an event-only trigger (emit its event instead).",
        method: Method::Post,
        path: "/triggers/run",
        args: &[TRIGGER_ID_QUERY_ARG],
        cli_name: "run",
        sdk_name: "run",
        mutating: true,
        llm_alias: Some("run_trigger"),
        llm_schema: Some(
            r#"{"trigger_id":{"type":"string","description":"UUID of the trigger to run now, off-schedule."}}"#,
        ),
        llm: None,
        cli: None,
        sdk: None,
    },
];

const TRIGGERS_DOMAIN: Domain = Domain {
    name: "triggers",
    tool_name: "triggers",
    tool_summary: "Create and manage triggers: scheduled (cron) and event-driven automations. Panel folders are the trigger_groups tool.",
    llm: true,
    cli: true,
    sdk: true,
    operations: TRIGGERS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::LegacyInProcess,
};

const TG_ID_QUERY_ARG: Arg = Arg {
    name: "id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "UUID of the trigger group.",
};
const TG_NAME_ARG: Arg = Arg {
    name: "name",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "Group name (the section header shown in the triggers panel).",
};
const TG_ORDER_ARG: Arg = Arg {
    name: "order",
    ty: ArgType::Int,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Sort position in the panel (ascending). Omit to sink to the bottom.",
};
const TG_ORDERING_ARG: Arg = Arg {
    name: "ordering",
    ty: ArgType::Json,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "JSON array of { id, order } entries to reorder atomically.",
};

const TRIGGER_GROUPS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "Groups with id, name, order and member_count.",
        method: Method::Get,
        path: "/trigger-groups",
        args: &[],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: Some("list_trigger_groups"),
        llm_schema: Some("{}"),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "create",
        summary: "Create a named folder. Names are unique, case-insensitively.",
        method: Method::Post,
        path: "/trigger-groups",
        args: &[TG_NAME_ARG, TG_ORDER_ARG],
        cli_name: "create",
        sdk_name: "create",
        mutating: true,
        llm_alias: Some("create_trigger_group"),
        llm_schema: Some(
            r#"{
              "name": {"type":"string","description":"Human-facing label shown as the section header."},
              "order": {"type":"integer","description":"Sort position in the panel (ascending). Omit to default to the bottom."}
            }"#,
        ),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "rename",
        summary: "Rename a group. Fails if another already uses the name, case-insensitively.",
        method: Method::Put,
        path: "/trigger-groups",
        args: &[TG_ID_QUERY_ARG, TG_NAME_ARG],
        cli_name: "rename",
        sdk_name: "rename",
        mutating: true,
        llm_alias: Some("rename_trigger_group"),
        llm_schema: Some(
            r#"{
              "group_id": {"type":"string","description":"UUID of the group to rename."},
              "name": {"type":"string","description":"New display name."}
            }"#,
        ),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "reorder",
        summary: "Atomic batch reorder: an array of { id, order } entries.",
        method: Method::Post,
        path: "/trigger-groups/reorder",
        args: &[TG_ORDERING_ARG],
        cli_name: "reorder",
        sdk_name: "reorder",
        mutating: true,
        llm_alias: Some("reorder_trigger_groups"),
        llm_schema: Some(
            r#"{
              "ordering": {"type":"array","description":"Array of { id, order } entries.","items":{"type":"object","properties":{"id":{"type":"string"},"order":{"type":"integer"}},"required":["id","order"]}}
            }"#,
        ),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "delete",
        summary: "Refused, with member ids, while the group still holds triggers: move them first.",
        method: Method::Delete,
        path: "/trigger-groups",
        args: &[TG_ID_QUERY_ARG],
        cli_name: "delete",
        sdk_name: "delete",
        mutating: true,
        llm_alias: Some("delete_trigger_group"),
        llm_schema: Some(
            r#"{"group_id":{"type":"string","description":"UUID of the group to delete."}}"#,
        ),
        llm: None,
        cli: None,
        sdk: None,
    },
];

const TRIGGER_GROUPS_DOMAIN: Domain = Domain {
    name: "trigger_groups",
    tool_name: "trigger_groups",
    tool_summary: "User-visible folders organizing triggers in the panel. Purely a label: a group fires and schedules nothing. Assign a trigger to one with the triggers tool's group_id.",
    llm: true,
    cli: true,
    sdk: false,
    operations: TRIGGER_GROUPS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::LegacyInProcess,
};

const APP_ID_QUERY_ARG: Arg = Arg {
    name: "id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "App id (the folder name under data/apps/, e.g. 'habit-tracker').",
};
const APP_NAME_ARG: Arg = Arg {
    name: "name",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "New display name for the app.",
};
const APP_DESCRIPTION_ARG: Arg = Arg {
    name: "description",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "New one-line description for the app.",
};

const APPS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "All apps: id, name, description, icon.",
        method: Method::Get,
        path: "/apps",
        args: &[],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "get",
        summary: "One app's metadata by id.",
        method: Method::Get,
        path: "/app",
        args: &[APP_ID_QUERY_ARG],
        cli_name: "get",
        sdk_name: "get",
        mutating: false,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "update",
        summary: "Update an app's name or description.",
        method: Method::Put,
        path: "/app",
        args: &[APP_ID_QUERY_ARG, APP_NAME_ARG, APP_DESCRIPTION_ARG],
        cli_name: "update",
        sdk_name: "update",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: Some(false),
    },
    Operation {
        action: "delete",
        summary: "Delete an app by id; a plugin-installed one goes through the plugin.",
        method: Method::Delete,
        path: "/app",
        args: &[APP_ID_QUERY_ARG],
        cli_name: "delete",
        sdk_name: "delete",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: Some(false),
    },
];

const APPS_DOMAIN: Domain = Domain {
    name: "apps",
    tool_name: "apps",
    tool_summary: "Manage apps. Creating one is the separate create_app tool, and app source is edited in the app's coding-agent worktree.",
    llm: false,
    cli: true,
    sdk: true,
    operations: APPS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::Route,
};

const EVENTS_EMIT_LLM_SCHEMA: &str = r#"{
  "event_type": {"type":"string","description":"PascalCase past tense, e.g. GoogleDocEdited."},
  "payload": {"type":"object","description":"REQUIRED. Enough context to understand what happened.","properties":{"summary":{"type":"string","description":"What happened, in one line."}},"required":["summary"]}
}"#;
const EVENTS_QUERY_LLM_SCHEMA: &str = r#"{
  "event_id": {"type":"string","description":"One event by id: the 'evt-<32 hex>' a tool result ends with, or a bare uuid. A tool call's address returns the pair, call then result. Errors if nothing matches."},
  "event_type": {"type":"string","description":"Omitting it queries all, worth avoiding on a busy workspace."},
  "thread_id": {"type":"string","description":"One thread: 'current' is this one. Read it back with event_type 'MessageReceived'. Leads with its liveness."},
  "since": {"type":"string","description":"After this RFC 3339 timestamp."},
  "until": {"type":"string","description":"Before this RFC 3339 timestamp."},
  "limit": {"type":"integer","description":"1-200, default 50. Raise only to fully enumerate a small type."},
  "byte_limit": {"type":"integer","description":"Response byte budget (1024-524288, default 131072). On truncation, narrow the query first."}
}"#;
const EVENTS_COUNT_LLM_SCHEMA: &str = r#"{
  "event_type": {"type":"string","description":"Omit for a per-type breakdown across all types."},
  "thread_id": {"type":"string","description":"One thread: 'current' is this one. Omit to count every thread."},
  "since": {"type":"string","description":"After this RFC 3339 timestamp."},
  "until": {"type":"string","description":"Before this RFC 3339 timestamp."}
}"#;

const EVENTS_OPS: &[Operation] = &[
    Operation {
        action: "emit",
        summary: "Record an immutable past-tense fact. The payload must include a 'summary'. (requires: event_type, payload)",
        method: Method::Post,
        path: "/events/emit",
        args: &[],
        cli_name: "emit",
        sdk_name: "emit",
        mutating: true,
        llm_alias: Some("emit_event"),
        llm_schema: Some(EVENTS_EMIT_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "query",
        summary: "Events newest-first as {events, total_matching, returned, byte_size, truncated, hint?}. Three calls a turn is a soft ceiling.",
        method: Method::Get,
        path: "/events/query",
        args: &[],
        cli_name: "query",
        sdk_name: "query",
        mutating: false,
        llm_alias: Some("query_events"),
        llm_schema: Some(EVENTS_QUERY_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "count",
        summary: "Count by type and time without materialising payloads. With event_type: {count, byte_total}; without: a per-type breakdown, count desc.",
        method: Method::Get,
        path: "/events/count",
        args: &[],
        cli_name: "count",
        sdk_name: "count",
        mutating: false,
        llm_alias: Some("count_events"),
        llm_schema: Some(EVENTS_COUNT_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "event_types",
        summary: "Which event types exist here: {engine, workspace, retired}. Read a name off 'engine' before subscribing.",
        method: Method::Get,
        path: "/events/types",
        args: &[],
        cli_name: "event-types",
        sdk_name: "eventTypes",
        mutating: false,
        llm_alias: Some("list_event_types"),
        llm_schema: Some("{}"),
        llm: None,
        cli: None,
        sdk: None,
    },
];

const EVENTS_DOMAIN: Domain = Domain {
    name: "events",
    tool_name: "events",
    tool_summary: "The workspace's event store, domain and engine events alike in one table. On a busy workspace call 'count' first, then 'query' the narrowest types.",
    llm: true,
    cli: false,
    sdk: false,
    operations: EVENTS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::LegacyInProcess,
};

const CHANGES_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "Pending, set-aside and applied changes; thread_unsettled means still working.",
        method: Method::Get,
        path: "/changes",
        args: &[],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: Some("list_changes"),
        llm_schema: Some(r#"{"sub_threads_of":{"type":"string","description":"Only its sub-threads' changes. A thread id or 'current'."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "apply",
        summary: "Merge into main as the Apply button does; returns status, SHAs, restart_required. Refused while its thread is unsettled; the error says why. (requires: change_id)",
        method: Method::Post,
        path: "/changes/:change_id/apply",
        args: &[],
        cli_name: "apply",
        sdk_name: "apply",
        mutating: true,
        llm_alias: Some("apply_change"),
        llm_schema: Some(r#"{"change_id":{"type":"string","description":"Change UUID, from 'list'."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "apply_when_settled",
        summary: "Apply a thread's change once it settles, through event waits. Drops on a question or failure. (requires: thread_id)",
        method: Method::Post,
        path: "/standing-applies",
        args: &[],
        cli_name: "apply-when-settled",
        sdk_name: "applyWhenSettled",
        mutating: true,
        llm_alias: Some("apply_when_settled"),
        llm_schema: Some(r#"{"thread_id":{"type":"string","description":"The thread to act on."},"change_id":{"type":"string","description":"Omit if nothing is proposed yet."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "apply_as_they_settle",
        summary: "Apply every settled change, then each settling thread's as it lands.",
        method: Method::Post,
        path: "/changes/apply-all?keep_going=true",
        args: &[],
        cli_name: "apply-as-they-settle",
        sdk_name: "applyAsTheySettle",
        mutating: true,
        llm_alias: Some("apply_as_they_settle"),
        llm_schema: Some("{}"),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "cancel_standing_apply",
        summary: "Take back a standing apply: thread_id's, or every one here. Stops future applies only.",
        method: Method::Delete,
        path: "/standing-applies",
        args: &[],
        cli_name: "cancel-standing-apply",
        sdk_name: "cancelStandingApply",
        mutating: true,
        llm_alias: Some("cancel_standing_apply"),
        llm_schema: Some(r#"{"thread_id":{"type":"string","description":"This thread's standing apply."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "set_aside",
        summary: "Keep a pending change for later, out of Review and Apply All. (requires: change_id)",
        method: Method::Post,
        path: "/changes/:change_id/set-aside",
        args: &[],
        cli_name: "set-aside",
        sdk_name: "setAside",
        mutating: true,
        llm_alias: Some("set_aside_change"),
        llm_schema: Some(r#"{"change_id":{"type":"string","description":"Change UUID, from 'list'."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "bring_back",
        summary: "Return a set-aside change to pending. (requires: change_id)",
        method: Method::Post,
        path: "/changes/:change_id/bring-back",
        args: &[],
        cli_name: "bring-back",
        sdk_name: "bringBack",
        mutating: true,
        llm_alias: Some("bring_back_change"),
        llm_schema: Some(r#"{"change_id":{"type":"string","description":"Change UUID, from 'list'."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
];

const CHANGES_DOMAIN: Domain = Domain {
    name: "changes",
    tool_name: "changes",
    tool_summary: "Changes: coding-agent branches awaiting Apply. 'list' finds ids; the rest ONLY when the user asked.",
    llm: true,
    cli: false,
    sdk: false,
    operations: CHANGES_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::LegacyInProcess,
};

pub const MODEL_PROVIDER_ENUM: &[&str] = &[
    "vertex",
    "anthropic",
    "openai",
    "openrouter",
    "xai",
    "opencode-free",
    "local",
    "claude-gateway",
    "chatgpt-plan",
];

const MODEL_ID_BODY_ARG: Arg = Arg {
    name: "id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "The string sent in API requests (e.g. 'z-ai/glm-5.2').",
};
const MODEL_ID_QUERY_ARG: Arg = Arg {
    name: "id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "Model id (the request string, e.g. 'z-ai/glm-5.2').",
};
const MODEL_LABEL_ARG: Arg = Arg {
    name: "label",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Display name; defaults to the id.",
};
const MODEL_PROVIDER_ARG: Arg = Arg {
    name: "provider",
    ty: ArgType::Str,
    enum_values: MODEL_PROVIDER_ENUM,
    required: false,
    loc: ArgIn::Body,
    description:
        "Backend that serves the model: the single-route shorthand, applied to the first route.",
};
const MODEL_ROUTES_ARG: Arg = Arg {
    name: "routes",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Every backend that serves the model, in priority order, e.g. [{\"provider\":\"vertex\"},{\"provider\":\"openrouter\",\"id\":\"anthropic/claude-opus-5-5\",\"context_window\":200000}]. `id` defaults to the model id. Replaces provider and context_window when given.",
};
const MODEL_PREFERRED_PROVIDER_ARG: Arg = Arg {
    name: "preferred_provider",
    ty: ArgType::Str,
    enum_values: MODEL_PROVIDER_ENUM,
    required: false,
    loc: ArgIn::Body,
    description: "The backend to use for this model when more than one route is configured. Must be one of its routes.",
};
const MODEL_SORT_ORDER_ARG: Arg = Arg {
    name: "sort_order",
    ty: ArgType::Int,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Lower sorts first; user models default to 1000.",
};
const MODEL_ON_CHAT_LADDER_ARG: Arg = Arg {
    name: "on_chat_ladder",
    ty: ArgType::Bool,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "On or off the ladder.",
};
const MODEL_ONTO_CHAT_LADDER_ARG: Arg = Arg {
    loc: ArgIn::FixedBody("true"),
    ..MODEL_ON_CHAT_LADDER_ARG
};
const MODEL_OFF_CHAT_LADDER_ARG: Arg = Arg {
    loc: ArgIn::FixedBody("false"),
    ..MODEL_ON_CHAT_LADDER_ARG
};
const MODEL_LADDER_REASON: &str = "Why you change the ladder. The owner reads it on the card.";
const MODEL_REASON_BODY_ARG: Arg = Arg {
    name: "reason",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: MODEL_LADDER_REASON,
};
const MODEL_REASON_QUERY_ARG: Arg = Arg {
    loc: ArgIn::Query,
    ..MODEL_REASON_BODY_ARG
};
const MODEL_CONTEXT_WINDOW_ARG: Arg = Arg {
    name: "context_window",
    ty: ArgType::Int,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Context window in tokens (e.g. 1048576), what the model actually serves. Omitting it guesses from the model id: 1M for an id carrying [1m], 400k for gpt-5*, 200k for everything else including OpenRouter, xAI, Gemini and local ids however large they are. The guess errs low on purpose.",
};
const MODEL_VISION_ARG: Arg = Arg {
    name: "vision",
    ty: ArgType::Bool,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Whether the model reads images. Image description offers and calls only such models. Omitted on add means false; on update, keeps the stored value.",
};

pub const MODELS_ADD_LLM_SCHEMA: &str = r#"{
  "id": {"type":"string","description":"Model id, the API string unless a route overrides it. All but list need it."},
  "label": {"type":"string","description":"Display name; defaults to the id."},
  "provider": {"type":"string","enum":["vertex","anthropic","openai","openrouter","xai","opencode-free","local","claude-gateway","chatgpt-plan"],"description":"Single-route shorthand for the first route. Add needs it or routes."},
  "sort_order": {"type":"integer","description":"Lower sorts first."},
  "context_window": {"anyOf":[{"type":"null"},{"type":"integer"}],"description":"First route's window in tokens; omitted, most non-Claude ids get 200k. Null clears."},
  "routes": {"type":"array","description":"Backends in priority order. Replaces the whole list.","items":{"type":"object","properties":{"provider":{"type":"string"},"id":{"type":"string","description":"This backend's id if different, e.g. 'anthropic/claude-opus-5-5'."},"context_window":{"type":"integer"}},"required":["provider"]}},
  "vision": {"type":"boolean","description":"Reads images; only such models describe images."}
}"#;
const MODELS_UPDATE_LLM_SCHEMA: &str = r#"{
  "preferred_provider": {"anyOf":[{"type":"null"},{"type":"string"}],"description":"Backend to use when several routes are configured. Null clears."}
}"#;

const MODELS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "Every model, on the ladder or off.",
        method: Method::Get,
        path: "/models",
        args: &[],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "add",
        summary: "Register a new model; needs provider or routes.",
        method: Method::Post,
        path: "/models",
        args: &[
            MODEL_ID_BODY_ARG,
            MODEL_LABEL_ARG,
            MODEL_PROVIDER_ARG,
            MODEL_SORT_ORDER_ARG,
            MODEL_CONTEXT_WINDOW_ARG,
            MODEL_ROUTES_ARG,
            MODEL_VISION_ARG,
        ],
        cli_name: "add",
        sdk_name: "add",
        mutating: true,
        llm_alias: None,
        llm_schema: Some(MODELS_ADD_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "enable",
        summary: "Put it on the ladder.",
        method: Method::Put,
        path: "/models",
        args: &[
            MODEL_ID_QUERY_ARG,
            MODEL_ONTO_CHAT_LADDER_ARG,
            MODEL_REASON_BODY_ARG,
        ],
        cli_name: "enable",
        sdk_name: "enable",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "disable",
        summary: "Take it off the ladder.",
        method: Method::Put,
        path: "/models",
        args: &[
            MODEL_ID_QUERY_ARG,
            MODEL_OFF_CHAT_LADDER_ARG,
            MODEL_REASON_BODY_ARG,
        ],
        cli_name: "disable",
        sdk_name: "disable",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "update",
        summary: "Edit any field; label and sort_order on user models only.",
        method: Method::Put,
        path: "/models",
        args: &[
            MODEL_ID_QUERY_ARG,
            MODEL_LABEL_ARG,
            MODEL_PROVIDER_ARG,
            MODEL_SORT_ORDER_ARG,
            MODEL_ON_CHAT_LADDER_ARG,
            MODEL_CONTEXT_WINDOW_ARG,
            MODEL_ROUTES_ARG,
            MODEL_PREFERRED_PROVIDER_ARG,
            MODEL_VISION_ARG,
            MODEL_REASON_BODY_ARG,
        ],
        cli_name: "update",
        sdk_name: "update",
        mutating: true,
        llm_alias: None,
        llm_schema: Some(MODELS_UPDATE_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: Some(false),
    },
    Operation {
        action: "remove",
        summary: "Delete a user-added model.",
        method: Method::Delete,
        path: "/models",
        args: &[MODEL_ID_QUERY_ARG, MODEL_REASON_QUERY_ARG],
        cli_name: "delete",
        sdk_name: "remove",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: Some(false),
    },
];

const MODELS_DOMAIN: Domain = Domain {
    name: "models",
    tool_name: "manage_models",
    tool_summary: "The chat-model registry the Lucidos Agent ladder picks from. A builtin can leave it but not be removed. Switch the ACTIVE model with set_preference(key='chat_model').",
    llm: true,
    cli: true,
    sdk: false,
    operations: MODELS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::Route,
};

pub const REPO_NAME_ARG: Arg = Arg {
    name: "name",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "Display name. Required for 'add', and what 'remove' looks up.",
};
const REPO_PATH_ARG: Arg = Arg {
    name: "path",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "Absolute path to the repo on disk, ~ allowed. Required for 'add'.",
};
const REPO_DESC_ARG: Arg = Arg {
    name: "description",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Optional description of the repository (for 'add').",
};

const REPOSITORIES_OPS: &[Operation] = &[
    Operation {
        action: "add",
        summary: "Register a local git repo so coding agents can work on it.",
        method: Method::Post,
        path: "/repositories",
        args: &[REPO_NAME_ARG, REPO_PATH_ARG, REPO_DESC_ARG],
        cli_name: "add",
        sdk_name: "add",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: Some(false),
        sdk: None,
    },
    Operation {
        action: "list",
        summary: "List registered repositories.",
        method: Method::Get,
        path: "/repositories",
        args: &[],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "remove",
        summary: "Unregister one by name.",
        method: Method::Delete,
        path: "/repositories",
        args: &[REPO_NAME_ARG],
        cli_name: "remove",
        sdk_name: "remove",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: Some(false),
        sdk: None,
    },
];

const REPOSITORIES_DOMAIN: Domain = Domain {
    name: "repositories",
    tool_name: "manage_repositories",
    tool_summary: "External git repositories registered for coding-agent sessions, so a coding agent can work on a local repo.",
    llm: true,
    cli: true,
    sdk: false,
    operations: REPOSITORIES_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::LegacyInProcess,
};

const ENV_NAME_BODY_ARG: Arg = Arg {
    name: "name",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "Uppercase letters, digits and underscores, not starting with a digit. An engine-owned name (CRED_*, OAUTH_*, PATH, LUCIDOS_*) is rejected.",
};
const ENV_VALUE_BODY_ARG: Arg = Arg {
    name: "value",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "Plaintext, non-secret. Use a credential for a secret.",
};
const ENV_NAME_QUERY_ARG: Arg = Arg {
    name: "name",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "Name of the variable to delete.",
};

const ENV_VARS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "Every variable with its value.",
        method: Method::Get,
        path: "/env-vars",
        args: &[],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "set",
        summary: "Create or replace one.",
        method: Method::Post,
        path: "/env-vars",
        args: &[ENV_NAME_BODY_ARG, ENV_VALUE_BODY_ARG],
        cli_name: "set",
        sdk_name: "set",
        mutating: true,
        llm_alias: Some("set_environment_variable"),
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "delete",
        summary: "Remove one by name.",
        method: Method::Delete,
        path: "/env-vars",
        args: &[ENV_NAME_QUERY_ARG],
        cli_name: "delete",
        sdk_name: "delete",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
];

const ENV_VARS_DOMAIN: Domain = Domain {
    name: "env_vars",
    tool_name: "env_vars",
    tool_summary: "Non-secret environment variables. A subprocess Lucidos spawns sees a change on its next spawn, no restart. The engine's own env gets them only at startup, so an engine-read var needs an engine restart. They appear in logs and events, so use request_credential for an API key, token or password.",
    llm: true,
    cli: true,
    sdk: false,
    operations: ENV_VARS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::LegacyInProcess,
};

const WIDGET_APP_ID_ARG: Arg = Arg {
    name: "app_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "The widget's id.",
};
const WIDGET_THREAD_BODY_ARG: Arg = Arg {
    name: "thread_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Thread uuid. Default: yours.",
};
const WIDGET_PARAMS_ARG: Arg = Arg {
    name: "params",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Widget params, a JSON object. One set is one instance.",
};
pub const WIDGET_SIZE_ARG: Arg = Arg {
    name: "size",
    ty: ArgType::Str,
    enum_values: &["small", "medium", "large", "xl"],
    required: true,
    loc: ArgIn::Body,
    description: "One the widget declares. Only a size the user named this turn.",
};
const WIDGET_CARD_SIZE_ARGS: &[Arg] = &[
    WIDGET_APP_ID_ARG,
    WIDGET_THREAD_BODY_ARG,
    WIDGET_PARAMS_ARG,
    WIDGET_SIZE_ARG,
];
pub const DASHBOARD_QUERY_ARG: Arg = Arg {
    name: "dashboard",
    ty: ArgType::Str,
    enum_values: &["phone", "desktop"],
    required: false,
    loc: ArgIn::Query,
    description: "Default: your device's.",
};
const DASHBOARD_BODY_ARG: Arg = Arg {
    loc: ArgIn::Body,
    ..DASHBOARD_QUERY_ARG
};
const DASHBOARD_LABEL_ARG: Arg = Arg {
    name: "label",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Tile label. Default: the name.",
};
const DASHBOARD_APP_ID_ARG: Arg = Arg {
    description: "The widget's or app's id. An app joins as a shortcut and takes no params.",
    ..WIDGET_APP_ID_ARG
};
const DASHBOARD_ADD_ARGS: &[Arg] = &[
    DASHBOARD_APP_ID_ARG,
    DASHBOARD_BODY_ARG,
    WIDGET_PARAMS_ARG,
    DASHBOARD_LABEL_ARG,
    Arg {
        required: false,
        description: "A widget's tile size. Default: its default size.",
        ..WIDGET_SIZE_ARG
    },
];
const DASHBOARD_REMOVE_ARGS: &[Arg] =
    &[DASHBOARD_APP_ID_ARG, DASHBOARD_BODY_ARG, WIDGET_PARAMS_ARG];
const DASHBOARD_TILES_ARG: Arg = Arg {
    name: "tiles",
    ty: ArgType::Json,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "Tiles in order: [{app_id, params?, size}].",
};
pub const DASHBOARD_ARRANGE_LLM_SCHEMA: &str = r#"{"dashboard":{"type":"string","enum":["phone","desktop"]},"tiles":{"type":"array","description":"Tiles in order.","items":{"type":"object","properties":{"app_id":{"type":"string"},"params":{"type":"object"},"size":{"enum":["quarter","small","medium","large","xl"]}},"required":["app_id","size"]}}}"#;

const fn widget_op(
    action: &'static str,
    summary: &'static str,
    method: Method,
    path: &'static str,
    args: &'static [Arg],
    cli_name: &'static str,
) -> Operation {
    Operation {
        action,
        summary,
        method,
        path,
        args,
        cli_name,
        sdk_name: action,
        mutating: !matches!(method, Method::Get),
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    }
}

const WIDGETS_OPS: &[Operation] = &[
    widget_op(
        "list",
        "The reusable widgets.",
        Method::Get,
        "/widgets",
        &[],
        "list",
    ),
    widget_op(
        "size",
        "Resize its cards in a thread.",
        Method::Post,
        "/widgets/size",
        WIDGET_CARD_SIZE_ARGS,
        "size",
    ),
    widget_op(
        "fit",
        "What each size's box cuts off.",
        Method::Post,
        "/widgets/fit",
        &[WIDGET_APP_ID_ARG, WIDGET_THREAD_BODY_ARG],
        "fit",
    ),
    widget_op(
        "make_reusable",
        "Offer it to every thread.",
        Method::Post,
        "/widgets/make-reusable",
        &[WIDGET_APP_ID_ARG],
        "make-reusable",
    ),
    widget_op(
        "stop_reusing",
        "Hand it back to its origin thread.",
        Method::Post,
        "/widgets/stop-reusing",
        &[WIDGET_APP_ID_ARG],
        "stop-reusing",
    ),
    widget_op(
        "dashboard",
        "Read a dashboard.",
        Method::Get,
        "/dashboard",
        &[DASHBOARD_QUERY_ARG],
        "dashboard",
    ),
    widget_op(
        "add_to_dashboard",
        "Add to a dashboard.",
        Method::Post,
        "/widgets/dashboard-add",
        DASHBOARD_ADD_ARGS,
        "dashboard-add",
    ),
    widget_op(
        "remove_from_dashboard",
        "Remove from a dashboard.",
        Method::Post,
        "/widgets/dashboard-remove",
        DASHBOARD_REMOVE_ARGS,
        "dashboard-remove",
    ),
    Operation {
        llm_schema: Some(DASHBOARD_ARRANGE_LLM_SCHEMA),
        ..widget_op(
            "arrange",
            "Arrange a dashboard, only when asked this turn.",
            Method::Post,
            "/widgets/arrange",
            &[DASHBOARD_BODY_ARG, DASHBOARD_TILES_ARG],
            "arrange",
        )
    },
];

const WIDGETS_DOMAIN: Domain = Domain {
    name: "widgets",
    tool_name: "widgets",
    tool_summary:
        "Reusable widgets, a thread's widget card sizes and the dashboards. Make one with create_app kind='widget'.",
    llm: true,
    cli: true,
    sdk: false,
    operations: WIDGETS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::LegacyInProcess,
};

pub const THREADS_LIST_LLM_SCHEMA: &str = r#"{
  "active": {"type":"boolean","description":"UNION of running and waiting_for_user_answer; false inverts. For 'is the workspace busy?' use status ['running']: a thread awaiting an answer is blocked on the human, not working."},
  "status": {"type":"array","items":{"type":"string","enum":["idle","running","waiting","waiting_for_user_answer","paused","failed"]},"description":"Turn status. Idle means no open turn; waiting is legacy. Conflicts with active."},
  "source": {"type":"string","description":"Comma-separated 'chat', 'trigger', 'coding-agent' (legacy 'claude_code' accepted). Omit for all."},
  "my_children": {"type":"boolean","description":"Restrict to this thread's DIRECT children, not grandchildren; resolved from the calling thread, so no id. How you recover a child's thread_id."},
  "has_draft": {"type":"boolean","description":"Holds an unsent draft; false inverts."},
  "change_state": {"type":"string","enum":["none","unproposed","proposed"],"description":"Coding-agent branch work."},
  "limit": {"type":"integer","description":"1-1000, default 100."}
}"#;
pub const THREADS_COUNT_LLM_SCHEMA: &str = r#"{
  "active": {"type":"boolean","description":"UNION of running and waiting_for_user_answer; false inverts. Omit for the total. Nonzero does NOT mean work is in flight: for 'is anything still running?' use status ['running']."},
  "status": {"type":"array","items":{"type":"string","enum":["idle","running","waiting","waiting_for_user_answer","paused","failed"]},"description":"Turn status, as in list."},
  "source": {"type":"string","description":"Comma-separated 'chat', 'trigger', 'coding-agent'. Omit for all."},
  "my_children": {"type":"boolean","description":"Restrict to this thread's DIRECT children; resolved from the calling thread, so no id."}
}"#;

const THREADS_SEARCH_Q_ARG: Arg = Arg {
    name: "q",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "What was discussed.",
};
const THREADS_SEARCH_LIMIT_ARG: Arg = Arg {
    name: "limit",
    ty: ArgType::Int,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Max threads (1-50, default 20).",
};
const THREADS_SEARCH_LLM_SCHEMA: &str = r#"{
  "q": {"type":"string","description":"What was discussed, in the words it would have been said in. Matches titles and content."},
  "limit": {"type":"integer","description":"1-50, default 20."}
}"#;

const THREADS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "Thread summaries newest-first, with liveness, turn status and draft previews.",
        method: Method::Get,
        path: "/threads/list",
        args: &[],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: Some("list_threads"),
        llm_schema: Some(THREADS_LIST_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "count",
        summary: "Same filters as 'list', returning { count: N }.",
        method: Method::Get,
        path: "/threads/count",
        args: &[],
        cli_name: "count",
        sdk_name: "count",
        mutating: false,
        llm_alias: Some("count_threads"),
        llm_schema: Some(THREADS_COUNT_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "drafts",
        summary: "Unsent drafts, newest edit first, each with preview and link. thread_id \
                  returns that draft whole. A draft has no link: give its thread's.",
        method: Method::Get,
        path: "/threads/drafts",
        args: &[],
        cli_name: "drafts",
        sdk_name: "drafts",
        mutating: false,
        llm_alias: Some("list_drafts"),
        llm_schema: None,
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "submit_draft",
        summary: "Send a saved draft exactly as saved, only on the owner's say-so. Read it \
                  with 'drafts' first. (requires: thread_id, expected_revision, expected_epoch)",
        method: Method::Post,
        path: "/threads/:thread_id/submit-draft",
        args: &[],
        cli_name: "submit-draft",
        sdk_name: "submitDraft",
        mutating: true,
        llm_alias: Some("submit_saved_draft"),
        llm_schema: Some(
            r#"{
              "expected_revision": {"type":"integer","description":"submit_draft: snapshot.draft_revision from 'drafts'."},
              "expected_epoch": {"type":"integer","description":"submit_draft: snapshot.compose_epoch from 'drafts'."},
              "retry_key": {"type":"string","description":"submit_draft: a uuid naming this send. Reuse it to retry."}
            }"#,
        ),
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "held_messages",
        summary: "Agent messages held until the user answers a question, with link.",
        method: Method::Get,
        path: "/threads/held-messages",
        args: &[],
        cli_name: "held-messages",
        sdk_name: "heldMessages",
        mutating: false,
        llm_alias: Some("list_held_messages"),
        llm_schema: None,
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "search",
        summary: "Find threads by topic (text plus semantic), with liveness and change state. \
                  Read one via `events` 'query' with its thread_id. (requires: q)",
        method: Method::Get,
        path: "/threads/search",
        args: &[THREADS_SEARCH_Q_ARG, THREADS_SEARCH_LIMIT_ARG],
        cli_name: "search",
        sdk_name: "search",
        mutating: false,
        llm_alias: Some("search_threads"),
        llm_schema: Some(THREADS_SEARCH_LLM_SCHEMA),
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "detach_child",
        summary: "Stop waiting for YOUR direct child: it moves to top level and keeps \
                  running, holding its slot until done. (requires: thread_id)",
        method: Method::Post,
        path: "/threads/:thread_id/detach",
        args: &[],
        cli_name: "detach",
        sdk_name: "detachChild",
        mutating: true,
        llm_alias: Some("detach_child_thread"),
        llm_schema: Some(r#"{"thread_id":{"type":"string","description":"The child's uuid."}}"#),
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "archive",
        summary: "Archive your thread ('current', after this turn) or another thread you have authority \
                  to archive. Authority comes from your subtree, the owner's standing instruction, or a \
                  matching permission. The Archive button's refusals apply. (requires: thread_id)",
        method: Method::Post,
        path: "/threads/:thread_id/archive",
        args: &[],
        cli_name: "archive",
        sdk_name: "archive",
        mutating: true,
        llm_alias: Some("archive_thread"),
        llm_schema: Some(
            r#"{"thread_id":{"type":"string","description":"'current', or the UUID of a thread you have authority to archive."}}"#,
        ),
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "cache_keep_alive",
        summary: "Turn a thread's cache keep-alive 'off' until it next runs, or 'on'. \
                  Not your own. (requires: thread_id, state)",
        method: Method::Post,
        path: "/threads/:thread_id/cache-keep-alive",
        args: &[],
        cli_name: "cache-keep-alive",
        sdk_name: "cacheKeepAlive",
        mutating: true,
        llm_alias: Some("switch_cache_keep_alive"),
        llm_schema: Some(r#"{"state":{"type":"string","enum":["off","on"]}}"#),
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "context_ceiling",
        summary: "Set the context size where a thread's coding-agent session hands off: \
                  'off', '<N>k' or '<P>%'. Lower only: your own, or a child's to at most \
                  yours. (requires: thread_id, ceiling)",
        method: Method::Put,
        path: "/threads/:thread_id/context-ceiling",
        args: &[],
        cli_name: "context-ceiling",
        sdk_name: "contextCeiling",
        mutating: true,
        llm_alias: Some("set_context_ceiling"),
        llm_schema: Some(r#"{"ceiling":{"type":"string"}}"#),
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "triage",
        summary: "Propose an action per inbox thread, each with a reason. Show the user; \
                  apply only after they reply.",
        method: Method::Post,
        path: "/threads/triage",
        args: &[],
        cli_name: "triage",
        sdk_name: "triage",
        mutating: true,
        llm_alias: Some("triage_threads"),
        llm_schema: None,
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "apply_triage",
        summary:
            "Apply what the user approved from this thread's newest triage. (requires: entries)",
        method: Method::Post,
        path: "/threads/triage/apply",
        args: &[],
        cli_name: "apply-triage",
        sdk_name: "applyTriage",
        mutating: true,
        llm_alias: Some("apply_thread_triage"),
        llm_schema: Some(
            r#"{"entries":{"type":"array","items":{"type":"object","properties":{"thread_id":{"type":"string"},"action":{"type":"string","enum":["archive","pin","dismiss_question"]}}}}}"#,
        ),
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
];

const THREADS_DOMAIN: Domain = Domain {
    name: "threads",
    tool_name: "threads",
    tool_summary: "Read threads and unsent drafts; each row has a link. Read what a thread is doing from liveness.state. A waiting thread wakes itself on the waits liveness names. Detach a child, archive one, or triage. Start via run_thread or run_coding_agent, redirect via follow_up_child_thread.",
    llm: true,
    cli: false,
    sdk: false,
    operations: THREADS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::LegacyInProcess,
};

const MEM_LIMIT_ARG: Arg = Arg {
    name: "limit",
    ty: ArgType::Int,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Max entries to return (default 50, capped at 200).",
};
const MEM_OFFSET_ARG: Arg = Arg {
    name: "offset",
    ty: ArgType::Int,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Row offset for pagination (default 0).",
};
const MEM_ENTRIES_SOURCE_TYPE_ARG: Arg = Arg {
    name: "source_type",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Filter entries by source type (e.g. 'event', 'artifact').",
};
const MEM_SORT_ARG: Arg = Arg {
    name: "sort",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Sort order for the entries page.",
};
const MEM_IMPORTANCE_ARG: Arg = Arg {
    name: "importance",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Importance levels to include: low,medium,high,critical.",
};
const MEM_SEARCH_Q_ARG: Arg = Arg {
    name: "q",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "What you are trying to find out.",
};
const MEM_SEARCH_LIMIT_ARG: Arg = Arg {
    name: "limit",
    ty: ArgType::Int,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Max entries (1-20, default 10).",
};
const MEMORY_SEARCH_LLM_SCHEMA: &str = r#"{
  "q": {"type":"string","description":"What you want to know. A bare entity name discriminates nothing in a workspace mostly about it; ask about state or an outcome."},
  "limit": {"type":"integer","description":"1-20, default 10."}
}"#;
const MEMORY_SOURCE_LLM_SCHEMA: &str = r#"{
  "id": {"type":"string","description":"The `[id: <uuid>]` on the memory, or the source event's uuid."}
}"#;

const MEM_SOURCE_ID_ARG: Arg = Arg {
    name: "source_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "The memory's `[id: <uuid>]`, or the source event's UUID; either resolves. \
                  Required when source_type is 'event'.",
};
const MEM_SOURCE_SOURCE_TYPE_ARG: Arg = Arg {
    name: "source_type",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Which source to inspect: 'event' (default) or 'artifact'.",
};
const MEM_PATH_ARG: Arg = Arg {
    name: "path",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Artifact path (required when source_type is 'artifact').",
};
const MEM_COMMIT_ARG: Arg = Arg {
    name: "commit",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Artifact commit SHA (required when source_type is 'artifact').",
};

const MEM_CORRECT_ID_ARG: Arg = Arg {
    name: "id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "The memory entry's UUID, as `entries` and `search` print it.",
};
const MEM_CORRECTION_ARG: Arg = Arg {
    name: "correction",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "A corrected fact to store after the delete. Omit to only delete.",
};

const MEMORY_OPS: &[Operation] = &[
    Operation {
        action: "correct",
        summary: "Delete the entries semantically matching a wrong claim. (requires: search_query, wrong_fact)",
        method: Method::Post,
        path: "/memory/correct",
        args: &[],
        cli_name: "correct",
        sdk_name: "correct",
        mutating: true,
        llm_alias: Some("correct_memory"),
        llm_schema: Some(
            r#"{
              "search_query": {"type":"string","description":"Keyword to find candidate memories (e.g., 'Acme Corp'). Broad is OK — semantic filtering narrows it down."},
              "wrong_fact": {"type":"string","description":"The specific wrong claim (e.g. 'User works at Acme Corp'). Only memories semantically similar to it are deleted."},
              "correction": {"type":"string","description":"Optional corrected fact to store after deleting the wrong memories. Omit to just delete."}
            }"#,
        ),
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "correct_by_id",
        summary: "Delete (and optionally replace) ONE memory by its id, the precise path when the [id: <uuid>] is visible. (requires: id)",
        method: Method::Post,
        path: "/memory/correct-by-id",
        args: &[MEM_CORRECT_ID_ARG, MEM_CORRECTION_ARG],
        cli_name: "correct-by-id",
        sdk_name: "correctById",
        mutating: true,
        llm_alias: Some("correct_memory_by_id"),
        llm_schema: Some(
            r#"{
              "id": {"type":"string","description":"The entry's UUID, copied verbatim from the [id: <uuid>] at the end of its bullet."},
              "correction": {"type":"string","description":"Optional corrected fact to store after deleting this entry. Omit to just delete."}
            }"#,
        ),
        llm: None,
        cli: None,
        sdk: Some(false),
    },
    Operation {
        action: "stats",
        summary: "Index stats: entry counts and sources.",
        method: Method::Get,
        path: "/memory/stats",
        args: &[],
        cli_name: "stats",
        sdk_name: "stats",
        mutating: false,
        llm_alias: None,
        llm_schema: None,
        llm: Some(false),
        cli: None,
        sdk: Some(false),
    },
    Operation {
        action: "entries",
        summary: "Paginated entries with their importance and source.",
        method: Method::Get,
        path: "/memory/entries",
        args: &[
            MEM_LIMIT_ARG,
            MEM_OFFSET_ARG,
            MEM_ENTRIES_SOURCE_TYPE_ARG,
            MEM_SORT_ARG,
            MEM_IMPORTANCE_ARG,
        ],
        cli_name: "entries",
        sdk_name: "entries",
        mutating: false,
        llm_alias: None,
        llm_schema: None,
        llm: Some(false),
        cli: None,
        sdk: Some(false),
    },
    Operation {
        action: "source",
        summary: "Where one memory came from: its event WITH thread_id, plus the other facts \
                  from that moment. Takes the `[id: <uuid>]` you were shown. (requires: id)",
        method: Method::Get,
        path: "/memory/source",
        args: &[
            MEM_SOURCE_ID_ARG,
            MEM_SOURCE_SOURCE_TYPE_ARG,
            MEM_PATH_ARG,
            MEM_COMMIT_ARG,
        ],
        cli_name: "source",
        sdk_name: "source",
        mutating: false,
        llm_alias: Some("memory_source"),
        llm_schema: Some(MEMORY_SOURCE_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: Some(false),
    },
    Operation {
        action: "search",
        summary: "Search long-term memory: what web_search is to the outside world, this is to \
                  what has happened HERE. (requires: q)",
        method: Method::Get,
        path: "/memory/search",
        args: &[MEM_SEARCH_Q_ARG, MEM_SEARCH_LIMIT_ARG],
        cli_name: "search",
        sdk_name: "search",
        mutating: false,
        llm_alias: Some("search_memory"),
        llm_schema: Some(MEMORY_SEARCH_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: Some(false),
    },
];

const MEMORY_DOMAIN: Domain = Domain {
    name: "memory",
    tool_name: "memory",
    tool_summary: "Read and correct long-term memory. Memories are injected before every turn, so 'search' is for when that missed something. Prefer 'correct_by_id' when the [id: <uuid>] is visible.",
    llm: true,
    cli: true,
    sdk: false,
    operations: MEMORY_OPS,
    llm_aliases: &[],
    llm_gate: Gate::MemoryClassic,
    llm_execution: LlmExecution::LegacyInProcess,
};

const RECALL_ID_ARG: Arg = Arg {
    name: "id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "A node id from a memory view or a recall result: w/<start>+<span> for the \
                  workspace tree, <thread id>/<start>+<span> for a thread's.",
};
const RECALL_N_ARG: Arg = Arg {
    name: "n",
    ty: ArgType::Int,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Levels to open, 1-6 (default 1). Each level halves the span of the lines.",
};
const RECALL_THREAD_ARG: Arg = Arg {
    name: "thread",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "The thread a bare <start>+<span> id names.",
};
const RECALL_COST_THREAD_ARG: Arg = Arg {
    name: "thread",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "The thread the judgment calls' cost is filed under.",
};
const RECALL_QUERY_ARG: Arg = Arg {
    name: "query",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "What to find, in plain words.",
};
const RECALL_TEXT_ARG: Arg = Arg {
    name: "text",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "Words the messages hold. Every word must appear.",
};
const RECALL_LIMIT_ARG: Arg = Arg {
    name: "limit",
    ty: ArgType::Int,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Max results (1-20, default 10).",
};

const RECALL_ZOOM_LLM_SCHEMA: &str = r#"{
  "id": {"type":"string","description":"A line's id, as the view or a result shows it. A bare start+span names this thread."},
  "n": {"type":"integer","description":"Levels to open, 1-6, default 1."}
}"#;
const RECALL_DATE_LLM_SCHEMA: &str = r#"{
  "id": {"type":"string","description":"A line's id, as the view or a result shows it. A bare start+span names this thread."}
}"#;
const RECALL_FIND_LLM_SCHEMA: &str = r#"{
  "query": {"type":"string","description":"What you want to find, in plain words."},
  "limit": {"type":"integer","description":"1-20, default 10."}
}"#;
const RECALL_SEARCH_LLM_SCHEMA: &str = r#"{
  "text": {"type":"string","description":"Words the messages hold. Every word must appear."},
  "limit": {"type":"integer","description":"1-20, default 10."}
}"#;

const RECALL_OPS: &[Operation] = &[
    Operation {
        action: "zoom",
        summary: "Open a memory view line into the lines it summarises, down to the exact message.",
        method: Method::Get,
        path: "/recall/zoom",
        args: &[RECALL_ID_ARG, RECALL_N_ARG, RECALL_THREAD_ARG],
        cli_name: "zoom",
        sdk_name: "zoom",
        mutating: false,
        llm_alias: Some("recall_zoom"),
        llm_schema: Some(RECALL_ZOOM_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "find",
        summary:
            "Walk the workspace tree for lines about something, judging each line on the way down.",
        method: Method::Get,
        path: "/recall/find",
        args: &[RECALL_QUERY_ARG, RECALL_LIMIT_ARG, RECALL_COST_THREAD_ARG],
        cli_name: "find",
        sdk_name: "find",
        mutating: false,
        llm_alias: Some("recall_find"),
        llm_schema: Some(RECALL_FIND_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "search",
        summary: "Find messages holding exact words. Returns each one's id.",
        method: Method::Get,
        path: "/recall/search",
        args: &[RECALL_TEXT_ARG, RECALL_LIMIT_ARG],
        cli_name: "search",
        sdk_name: "search",
        mutating: false,
        llm_alias: Some("recall_search"),
        llm_schema: Some(RECALL_SEARCH_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "date",
        summary: "When the entries under a line happened, first to last.",
        method: Method::Get,
        path: "/recall/date",
        args: &[RECALL_ID_ARG, RECALL_THREAD_ARG],
        cli_name: "date",
        sdk_name: "date",
        mutating: false,
        llm_alias: Some("recall_date"),
        llm_schema: Some(RECALL_DATE_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: None,
    },
];

const RECALL_DOMAIN: Domain = Domain {
    name: "recall",
    tool_name: "recall",
    tool_summary: "Open and search the summary trees behind this turn's memory views. Each view line starts with its id.",
    llm: true,
    cli: true,
    sdk: false,
    operations: RECALL_OPS,
    llm_aliases: &[],
    llm_gate: Gate::MemoryTree,
    llm_execution: LlmExecution::LegacyInProcess,
};

const TQ_ENTRY_ID_ARG: Arg = Arg {
    name: "entry_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "UUID of the queued entry (from the 'list' action's entries[].id).",
};

const TQ_POLICY_LLM_SCHEMA: &str = r#"{
  "max_concurrent_total": {"type":"integer","minimum":0,"description":"Background and user work alike."},
  "max_concurrent_event_trigger": {"type":"integer","minimum":0},
  "max_concurrent_cron": {"type":"integer","minimum":0},
  "max_concurrent_sub_thread": {"type":"integer","minimum":0},
  "max_concurrent_coding_agent": {"type":"integer","minimum":0},
  "max_concurrent_per_trigger": {"type":"integer","minimum":0,"description":"1 keeps per-trigger FIFO."},
  "max_queued_per_trigger": {"type":"integer","minimum":1,"description":"Backlog before overflow applies."},
  "reserved_background": {"type":"integer","minimum":0,"description":"Reclaimed ahead of user work; 0 is pure user priority."},
  "max_event_trigger_depth": {"type":"integer","minimum":1,"description":"Trigger fires one event chain may make."},
  "max_concurrent_children_per_thread": {"type":"integer","minimum":1},
  "overflow": {"type":"string","enum":["drop-oldest","pause-trigger"]}
}"#;

const THREAD_QUEUE_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "Live queue and active policy as { entries, policy }, user-initiated occupants included.",
        method: Method::Get,
        path: "/thread-queue",
        args: &[],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: Some("list_thread_queue"),
        llm_schema: Some("{}"),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "update_policy",
        summary: "Only the cap fields you send, merged with the live policy.",
        method: Method::Put,
        path: "/thread-queue/policy",
        args: &[],
        cli_name: "policy",
        sdk_name: "updatePolicy",
        mutating: true,
        llm_alias: Some("update_thread_queue_policy"),
        llm_schema: Some(TQ_POLICY_LLM_SCHEMA),
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "run_now",
        summary: "Force-admit a queued entry now, ignoring every cap.",
        method: Method::Post,
        path: "/thread-queue/run-now",
        args: &[TQ_ENTRY_ID_ARG],
        cli_name: "run-now",
        sdk_name: "runNow",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: Some(false),
        cli: None,
        sdk: Some(false),
    },
    Operation {
        action: "drop",
        summary: "Drop a queued entry without running it.",
        method: Method::Post,
        path: "/thread-queue/drop",
        args: &[TQ_ENTRY_ID_ARG],
        cli_name: "drop",
        sdk_name: "drop",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: Some(false),
        cli: None,
        sdk: Some(false),
    },
];

const THREAD_QUEUE_DOMAIN: Domain = Domain {
    name: "thread_queue",
    tool_name: "thread_queue",
    tool_summary: "The Thread Queue: admission control for background spawns AND user-initiated work. Call 'list' before a relative change like 'double capacity'. A cap of 0 holds admission.",
    llm: true,
    cli: true,
    sdk: false,
    operations: THREAD_QUEUE_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::LegacyInProcess,
};

const MCP_ID_ARG: Arg = Arg {
    name: "id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Path,
    description: "MCP server id, as shown by `list`.",
};

const MCP_THREAD_ID_ARG: Arg = Arg {
    name: "thread_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Path,
    description: "Thread uuid whose enabled MCP servers change.",
};

const MCP_THREAD_VERB_LLM_SCHEMA: &str =
    r#"{"id":{"type":"string","description":"Server id, as shown in [MCP SERVERS]"}}"#;

const MCP_SETUP_LLM_SCHEMA: &str = r#"{
  "id": {"type":"string","description":"Lowercase with hyphens, e.g. 'blender-mcp'."},
  "name": {"type":"string","description":"Display name."},
  "command": {"type":"string","description":"e.g. 'npx', 'uvx'."},
  "args": {"type":"array","items":{"type":"string"},"description":"e.g. ['blender-mcp']."},
  "env": {"type":"object","additionalProperties":{"anyOf":[{"type":"string"},{"type":"object","properties":{"credential":{"type":"string"}},"required":["credential"]}]},"description":"Env vars. Pass a secret as {\"credential\": \"<name>\"} naming a stored credential, never its value."}
}"#;

const MCP_OPS: &[Operation] = &[
    Operation {
        action: "setup",
        summary: "Register a server, spawn it and discover its tools. (requires: id, name, command, args)",
        method: Method::Post,
        path: "/mcp/servers",
        args: &[],
        cli_name: "setup",
        sdk_name: "setup",
        mutating: true,
        llm_alias: Some("setup_mcp_server"),
        llm_schema: Some(MCP_SETUP_LLM_SCHEMA),
        llm: None,
        cli: Some(false),
        sdk: None,
    },
    Operation {
        action: "list",
        summary: "List servers with their status and tools.",
        method: Method::Get,
        path: "/mcp/servers",
        args: &[],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: Some("list_mcp_servers"),
        llm_schema: Some("{}"),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "enable",
        summary: "Use a server's tools in this thread, starting it if needed. (requires: id)",
        method: Method::Post,
        path: "/threads/:thread_id/mcp-servers/:id/enable",
        args: &[MCP_THREAD_ID_ARG, MCP_ID_ARG],
        cli_name: "enable",
        sdk_name: "enable",
        mutating: true,
        llm_alias: Some("enable_mcp_server"),
        llm_schema: Some(MCP_THREAD_VERB_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "disable",
        summary: "Stop sending a server's tools in this thread. Other threads keep it. (requires: id)",
        method: Method::Post,
        path: "/threads/:thread_id/mcp-servers/:id/disable",
        args: &[MCP_THREAD_ID_ARG, MCP_ID_ARG],
        cli_name: "disable",
        sdk_name: "disable",
        mutating: true,
        llm_alias: Some("disable_mcp_server"),
        llm_schema: Some(MCP_THREAD_VERB_LLM_SCHEMA),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "start",
        summary: "Start a stopped server. Thread enablement is a separate action. (requires: id)",
        method: Method::Post,
        path: "/mcp/servers/:id/start",
        args: &[MCP_ID_ARG],
        cli_name: "start",
        sdk_name: "start",
        mutating: true,
        llm_alias: Some("start_mcp_server"),
        llm_schema: Some(r#"{"id":{"type":"string","description":"Server id to start"}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "stop",
        summary: "Stop a server and switch it off in every thread. (requires: id)",
        method: Method::Post,
        path: "/mcp/servers/:id/stop",
        args: &[MCP_ID_ARG],
        cli_name: "stop",
        sdk_name: "stop",
        mutating: true,
        llm_alias: Some("stop_mcp_server"),
        llm_schema: Some(r#"{"id":{"type":"string","description":"Server id to stop"}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "remove",
        summary: "Remove a server, stopping it first. (requires: id)",
        method: Method::Delete,
        path: "/mcp/servers/:id",
        args: &[MCP_ID_ARG],
        cli_name: "remove",
        sdk_name: "remove",
        mutating: true,
        llm_alias: Some("remove_mcp_server"),
        llm_schema: Some(r#"{"id":{"type":"string","description":"Server id to remove"}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
];

const MCP_DOMAIN: Domain = Domain {
    name: "mcp",
    tool_name: "mcp",
    tool_summary: "Manage MCP (Model Context Protocol) servers. web_search first for the right package and command.",
    llm: true,
    cli: true,
    sdk: false,
    operations: MCP_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::LegacyInProcess,
};

const PLUGINS_OPS: &[Operation] = &[
    Operation {
        action: "install",
        summary: "Stage an install for the user to confirm in a panel, which resolves the source. (requires: source)",
        method: Method::Post,
        path: "/plugins/install-request",
        args: &[],
        cli_name: "install",
        sdk_name: "install",
        mutating: true,
        llm_alias: Some("install_plugin"),
        llm_schema: Some(r#"{"source":{"type":"string","description":"A GitHub tree URL, a plain git URL, or an absolute path to a .lucidos-plugin file."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "register_marketplace",
        summary: "Register or rename a marketplace, a git or GitHub tree URL the Plugins panel scans for manifests. (requires: source)",
        method: Method::Post,
        path: "/plugins/marketplaces",
        args: &[],
        cli_name: "register-marketplace",
        sdk_name: "registerMarketplace",
        mutating: true,
        llm_alias: Some("register_plugin_marketplace"),
        llm_schema: Some(
            r#"{
              "source": {"type":"string","description":"Git repository URL or GitHub tree URL to register as a marketplace."},
              "name": {"type":"string","description":"Optional display name. Omit to derive a name from the repository."}
            }"#,
        ),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "check_updates",
        summary: "Check installed plugins for newer versions at their source URL; omit id for all. A per-plugin fetch failure is an `error` entry, not an abort.",
        method: Method::Get,
        path: "/plugins/updates",
        args: &[],
        cli_name: "check-updates",
        sdk_name: "checkUpdates",
        mutating: false,
        llm_alias: Some("check_plugin_updates"),
        llm_schema: Some(r#"{"id":{"type":"string","description":"Optional plugin id (e.g. 'browser-learning'). Omit to check every installed plugin."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "update",
        summary: "Re-fetch one plugin's manifest and re-install if newer; already-at-latest is a no-op. (requires: id)",
        method: Method::Post,
        path: "/plugins/update",
        args: &[],
        cli_name: "update",
        sdk_name: "update",
        mutating: true,
        llm_alias: Some("update_plugin"),
        llm_schema: Some(r#"{"id":{"type":"string","description":"The plugin id to update (e.g. 'browser-learning')."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "uninstall",
        summary: "Stage an uninstall for the user to confirm in a panel. (requires: id)",
        method: Method::Post,
        path: "/plugins/uninstall-request",
        args: &[],
        cli_name: "uninstall",
        sdk_name: "uninstall",
        mutating: true,
        llm_alias: Some("uninstall_plugin"),
        llm_schema: Some(r#"{"id":{"type":"string","description":"Plugin id, manifest name, or the app folder it installed. Case- and separator-insensitive."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
];

const PLUGINS_DOMAIN: Domain = Domain {
    name: "plugins",
    tool_name: "plugins",
    tool_summary: "Lucidos plugins: bundles of workspace content (apps, knowhow, triggers, scripts) another author shipped.",
    llm: true,
    cli: false,
    sdk: false,
    operations: PLUGINS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::LegacyInProcess,
};

const WEBHOOK_ID_ARG: Arg = Arg {
    name: "id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Path,
    description: "Webhook UUID, as shown by `list`.",
};

const WEBHOOK_DEDUPE_ARG: Arg = Arg {
    name: "dedupe",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Recognise a resend instead of emitting twice: {header?, window_secs?}. `header` names the header carrying the sender's delivery id (e.g. 'X-GitHub-Delivery'); with none, the key is a digest of the body. `window_secs` defaults to 3600, is capped at 604800, and 0 switches deduping off. Omit the whole block and every arrival emits, which is what keeps a sender's retries visible on the log.",
};

const WEBHOOK_SIGNING_SECRET_ARG: Arg = Arg {
    name: "signing_secret",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Save the secret the `hmac` block names, instead of pointing at one you saved earlier. `{\"mode\":\"generate\"}` mints 32 bytes and prints them ONCE, which is right for GitHub, where you choose the secret. `{\"mode\":\"provided\",\"value\":\"...\"}` stores a secret the sender issued, which is the only option for Slack and Stripe. For Slack, create the hook first, then paste its URL into Event Subscriptions: Slack checks the URL at once, and a hook verifying `X-Slack-Signature` answers that `url_verification` challenge. The value is stored byte for byte, so a value with surrounding whitespace is refused rather than trimmed. On `create` it refuses to overwrite an existing credential; on `update` it replaces the value, which is how you rotate.",
};

const WEBHOOK_HEADERS_ARG: Arg = Arg {
    name: "headers",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Header names to copy into the event payload under `headers`, as a JSON array (e.g. [\"X-GitHub-Event\"]). A condition then reads `headers.X-GitHub-Event`. An allow-list: `Authorization` and the hook's own signature header are refused, because the events table is append-only.",
};

const WEBHOOKS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "Every webhook: id, name, pinned event type, whether it is signed and enabled, and the path a sender posts to.",
        method: Method::Get,
        path: "/webhooks",
        args: &[],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "create",
        summary: "Create a webhook. An unsigned one prints its token ONCE, since only the digest is stored. A SIGNED one gets no token: a sender like GitHub cannot present one.",
        method: Method::Post,
        path: "/webhooks",
        args: &[
            Arg {
                name: "name",
                ty: ArgType::Str,
                enum_values: &[],
                required: true,
                loc: ArgIn::Body,
                description: "What to call this webhook in the list.",
            },
            Arg {
                name: "event_type",
                ty: ArgType::Str,
                enum_values: &[],
                required: true,
                loc: ArgIn::Body,
                description: "The domain event every delivery emits, PascalCase past tense (e.g. 'DeployFinished'). Pinned: a caller cannot change it.",
            },
            Arg {
                name: "hmac",
                ty: ArgType::Json,
                enum_values: &[],
                required: false,
                loc: ArgIn::Body,
                description: "Signature config: {credential, signature_header, prefix?, signature_key?, timestamp_header?, timestamp_key?, template?, algorithm?, encoding?, tolerance_secs?}. `credential` names a saved credential; the secret is never copied here. Pass `--signing-secret` alongside to save that credential in the same call.",
            },
            WEBHOOK_SIGNING_SECRET_ARG,
            WEBHOOK_DEDUPE_ARG,
            WEBHOOK_HEADERS_ARG,
        ],
        cli_name: "create",
        sdk_name: "create",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "update",
        summary: "Rename a webhook, repin its event type, or switch it off. Omitted fields keep their stored value.",
        method: Method::Put,
        path: "/webhooks/:id",
        args: &[
            WEBHOOK_ID_ARG,
            Arg {
                name: "name",
                ty: ArgType::Str,
                enum_values: &[],
                required: false,
                loc: ArgIn::Body,
                description: "New name.",
            },
            Arg {
                name: "event_type",
                ty: ArgType::Str,
                enum_values: &[],
                required: false,
                loc: ArgIn::Body,
                description: "New pinned event type.",
            },
            Arg {
                name: "enabled",
                ty: ArgType::Bool,
                enum_values: &[],
                required: false,
                loc: ArgIn::Body,
                description: "false stops the endpoint accepting deliveries, without deleting it.",
            },
            Arg {
                name: "hmac",
                ty: ArgType::Json,
                enum_values: &[],
                required: false,
                loc: ArgIn::Body,
                description: "Change what this hook verifies with, keeping its delivery URL. Same object `create` takes. Pass `null` to stop signing, which mints a bearer token and prints it ONCE, since a hook always carries exactly one verifier. Setting a signature drops any token for the same reason: a sender that signs attaches no bearer token, so a hook holding both would refuse every real delivery.",
            },
            WEBHOOK_SIGNING_SECRET_ARG,
            WEBHOOK_DEDUPE_ARG,
            WEBHOOK_HEADERS_ARG,
        ],
        cli_name: "update",
        sdk_name: "update",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "delete",
        summary: "Delete a webhook. Its URL answers nothing from then on.",
        method: Method::Delete,
        path: "/webhooks/:id",
        args: &[WEBHOOK_ID_ARG],
        cli_name: "delete",
        sdk_name: "delete",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
];

const WEBHOOKS_DOMAIN: Domain = Domain {
    name: "webhooks",
    tool_name: "webhooks",
    tool_summary:
        "Inbound webhooks: endpoints a third party posts to, each emitting one pinned domain event.",
    llm: false,
    cli: true,
    sdk: false,
    operations: WEBHOOKS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::Route,
};

const WORKSPACE_PROMPT_FOOTPRINT_OPS: &[Operation] = &[Operation {
    action: "show",
    summary: "Each workspace-grown prompt section's chars against its ceiling, with its items: clipped descriptions, unused apps and reusable widgets, and knowhow docs not loaded by name. Plus the system prompt footprint, for scale.",
    method: Method::Get,
    path: "/workspace-prompt-footprint",
    args: &[],
    cli_name: "show",
    sdk_name: "show",
    mutating: false,
    llm_alias: None,
    llm_schema: None,
    llm: None,
    cli: None,
    sdk: None,
}];

const WORKSPACE_PROMPT_FOOTPRINT_DOMAIN: Domain = Domain {
    name: "workspace_prompt_footprint",
    tool_name: "workspace_prompt_footprint",
    tool_summary:
        "The workspace prompt footprint: what the workspace's own content adds to every chat turn.",
    llm: false,
    cli: true,
    sdk: false,
    operations: WORKSPACE_PROMPT_FOOTPRINT_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::Route,
};

const PERMISSIONS_ALL_ARG: Arg = Arg {
    name: "all",
    ty: ArgType::Bool,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Include ended permissions: spent, revoked, or ended when their plan landed.",
};
const PERMISSION_ID_ARG: Arg = Arg {
    name: "permission_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Path,
    description: "The permission to revoke, from `list`.",
};
const BATCH_ITEMS_ARG: Arg = Arg {
    name: "items",
    ty: ArgType::Json,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "JSON array of items. A pick on an open card in your sub-threads: { \"kind\": \"card_answer\", \"thread_id\": \"…\", \"option\": \"<label>\" }, adding \"tool_use_id\" when the thread has more than one open card. A trigger's grants: { \"kind\": \"trigger_grant\", \"trigger\": \"<id or slug>\", \"coding_agents\": true }. Workspaces' origin, for workspaces not yet classified: { \"kind\": \"workspace_classification\", \"workspaces\": [\"<slug>\"], \"origin\": \"agent|demo|eval|e2e|user\", \"evidence\": \"…\" }. Move machine-made workspaces to Trash after the owner confirms: { \"kind\": \"workspace_deletion\", \"workspaces\": [\"<slug>\"] }.",
};

const PERMISSIONS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "Live permissions, newest first: action, breadth, lifetime, holder, and the thread that granted it.",
        method: Method::Get,
        path: "/permissions",
        args: &[PERMISSIONS_ALL_ARG],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "revoke",
        summary: "Revoke one permission, ending it now. Revoking only takes power away.",
        method: Method::Delete,
        path: "/permissions/:permission_id",
        args: &[PERMISSION_ID_ARG],
        cli_name: "revoke",
        sdk_name: "revoke",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "batch",
        summary: "Raise a batch on your own thread: one card that runs exact picks on your sub-threads' open cards once the owner confirms it. Each pick is checked again as it runs.",
        method: Method::Post,
        path: "/permissions/batches",
        args: &[BATCH_ITEMS_ARG],
        cli_name: "batch",
        sdk_name: "batch",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
];

const PERMISSIONS_DOMAIN: Domain = Domain {
    name: "permissions",
    tool_name: "permissions",
    tool_summary: "Permissions: what the owner's answers to permission requests granted, and batches for the owner to confirm.",
    llm: false,
    cli: true,
    sdk: false,
    operations: PERMISSIONS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::Route,
};

const HOOK_SLUG_ARG: Arg = Arg {
    name: "slug",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Path,
    description: "The hook's folder name under data/hooks/, as `list` shows it.",
};

const HOOK_REASON_ARG: Arg = Arg {
    name: "reason",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Why you ask. An agent must give one: the owner reads it on the card.",
};

const HOOKS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "Every hook under data/hooks/, valid or not, with its arming: disarmed, armed, or changed since it was armed. A row also says why it cannot be armed now.",
        method: Method::Get,
        path: "/hooks",
        args: &[],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "choices",
        summary: "Search the values for an argument on an armed first-message hook.",
        method: Method::Get,
        path: "/hooks/:slug/arguments/:argument/choices",
        args: &[
            HOOK_SLUG_ARG,
            Arg {
                name: "argument",
                ty: ArgType::Str,
                enum_values: &[],
                required: true,
                loc: ArgIn::Path,
                description: "The declared argument name.",
            },
            Arg {
                name: "query",
                ty: ArgType::Str,
                enum_values: &[],
                required: false,
                loc: ArgIn::Query,
                description: "Text to search for. Omit to list the first choices.",
            },
        ],
        cli_name: "choices",
        sdk_name: "choices",
        mutating: false,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "arm",
        summary: "Arm a valid hook at its current content, so Lucidos asks it before it records its event. Owner only: an agent needs the owner's Allow once, and an edit asks again.",
        method: Method::Post,
        path: "/hooks/:slug/arm",
        args: &[HOOK_SLUG_ARG, HOOK_REASON_ARG],
        cli_name: "arm",
        sdk_name: "arm",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "disarm",
        summary: "Disarm an armed hook. Its folder stays. Owner only: an agent needs the owner's Allow once.",
        method: Method::Post,
        path: "/hooks/:slug/disarm",
        args: &[HOOK_SLUG_ARG, HOOK_REASON_ARG],
        cli_name: "disarm",
        sdk_name: "disarm",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "remove",
        summary: "Delete a hook's folder from data/hooks/, commit that, and end any arming of it. Owner only: an agent needs the owner's Allow once.",
        method: Method::Delete,
        path: "/hooks/:slug",
        args: &[HOOK_SLUG_ARG, HOOK_REASON_ARG],
        cli_name: "remove",
        sdk_name: "remove",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
];

const HOOKS_DOMAIN: Domain = Domain {
    name: "hooks",
    tool_name: "hooks",
    tool_summary:
        "Event hooks: the scripts a workspace defines to stop progress until its own rule holds.",
    llm: false,
    cli: true,
    sdk: false,
    operations: HOOKS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::Route,
};

const PIN_DEVICE_QUERY_ARG: Arg = Arg {
    name: "device_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Query,
    description: "The device whose pins to list, as `lucidos devices list` shows it.",
};
const PIN_DEVICE_ARG: Arg = Arg {
    name: "device_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "The device to pin or unpin for, as `lucidos devices list` shows it.",
};
const PIN_APP_ARG: Arg = Arg {
    name: "app_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "The app's id, as `lucidos apps list` shows it.",
};

const PINNED_APPS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "The apps one device has pinned, in pin order.",
        method: Method::Get,
        path: "/pinned-apps",
        args: &[PIN_DEVICE_QUERY_ARG],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "pin",
        summary: "Pin an app for one device. Pinning a pinned app changes nothing.",
        method: Method::Post,
        path: "/pinned-apps",
        args: &[PIN_APP_ARG, PIN_DEVICE_ARG],
        cli_name: "pin",
        sdk_name: "pin",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "unpin",
        summary: "Unpin an app for one device. Unpinning an unpinned app changes nothing.",
        method: Method::Delete,
        path: "/pinned-apps",
        args: &[PIN_APP_ARG, PIN_DEVICE_ARG],
        cli_name: "unpin",
        sdk_name: "unpin",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        llm: None,
        cli: None,
        sdk: None,
    },
];

const PINNED_APPS_DOMAIN: Domain = Domain {
    name: "pinned_apps",
    tool_name: "pinned_apps",
    tool_summary: "The apps each device has pinned.",
    llm: false,
    cli: true,
    sdk: false,
    operations: PINNED_APPS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::Route,
};

const BROWSER_DOMAIN_ARG: Arg = Arg {
    name: "domain",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "The site's domain, such as example.com.",
};

const BROWSER_OPS: &[Operation] = &[Operation {
    action: "forget_login",
    summary: "Forget that the built-in browser is signed in to one domain. The chat tool browser_forget_login does the same.",
    method: Method::Post,
    path: "/browser/forget-login",
    args: &[BROWSER_DOMAIN_ARG],
    cli_name: "forget-login",
    sdk_name: "forgetLogin",
    mutating: true,
    llm_alias: None,
    llm_schema: None,
    llm: None,
    cli: None,
    sdk: None,
}];

const BROWSER_DOMAIN: Domain = Domain {
    name: "browser",
    tool_name: "browser",
    tool_summary: "The built-in browser's record of signed-in domains.",
    llm: false,
    cli: true,
    sdk: false,
    operations: BROWSER_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::Route,
};

const TERMINAL_COMMAND_ARG: Arg = Arg {
    name: "command",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "The exact command line, as the user would type it. One line of printable text.",
};
const TERMINAL_LABEL_ARG: Arg = Arg {
    name: "label",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Optional. What the command does, in a few words, shown above it.",
};

const TERMINAL_OPS: &[Operation] = &[Operation {
    action: "suggest",
    summary: "Offer a command on your own thread with a Run in terminal button. A tap types it at the thread terminal's prompt and never presses Enter: the user reads it and runs it. Works with the Terminal switch off, and the row then tells the user to turn it on.",
    method: Method::Post,
    path: "/terminal-suggestions",
    args: &[TERMINAL_COMMAND_ARG, TERMINAL_LABEL_ARG],
    cli_name: "suggest",
    sdk_name: "suggest",
    mutating: true,
    llm_alias: None,
    llm_schema: None,
    llm: None,
    cli: None,
    sdk: None,
}];

const TERMINAL_DOMAIN: Domain = Domain {
    name: "terminal",
    tool_name: "terminal_suggestions",
    tool_summary: "Terminal suggestions: commands for the user to run in this thread's terminal. You cannot read the terminal or send it keys.",
    llm: true,
    cli: true,
    sdk: false,
    operations: TERMINAL_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
    llm_execution: LlmExecution::Route,
};

const DOMAINS: &[Domain] = &[
    Domain {
        name: "notifications",
        tool_name: "notifications",
        tool_summary:
            "Read and clear the notification inbox. SENDING is the separate send_notification tool.",
        llm: true,
        cli: true,
        sdk: true,
        operations: NOTIFICATIONS_OPS,
        llm_aliases: &[],
        llm_gate: Gate::Ungated,
        llm_execution: LlmExecution::LegacyInProcess,
    },
    PREFERENCES_DOMAIN,
    TRIGGERS_DOMAIN,
    TRIGGER_GROUPS_DOMAIN,
    APPS_DOMAIN,
    EVENTS_DOMAIN,
    CHANGES_DOMAIN,
    THREADS_DOMAIN,
    MEMORY_DOMAIN,
    RECALL_DOMAIN,
    THREAD_QUEUE_DOMAIN,
    ENV_VARS_DOMAIN,
    MODELS_DOMAIN,
    REPOSITORIES_DOMAIN,
    MCP_DOMAIN,
    PLUGINS_DOMAIN,
    WEBHOOKS_DOMAIN,
    WIDGETS_DOMAIN,
    WORKSPACE_PROMPT_FOOTPRINT_DOMAIN,
    PERMISSIONS_DOMAIN,
    HOOKS_DOMAIN,
    PINNED_APPS_DOMAIN,
    BROWSER_DOMAIN,
    TERMINAL_DOMAIN,
];

pub fn domains() -> &'static [Domain] {
    DOMAINS
}

pub fn domain_for_tool(tool_name: &str) -> Option<&'static Domain> {
    DOMAINS
        .iter()
        .find(|d| (d.llm && d.tool_name == tool_name) || d.alias_names().contains(&tool_name))
}
