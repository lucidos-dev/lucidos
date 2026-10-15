//! The capability parity manifest's data: which agent surfaces (LLM tools, the
//! `lucidos` CLI, the JS SDK) expose each capability (ADR 0018).
//!
//! It lives in this leaf crate so the CLI's `build.rs` can render its command
//! module from it without compiling the engine. The engine re-exports it from
//! `capability_manifest` and builds the grouped LLM tools there. Why each
//! domain is shaped as it is, and which capabilities are deliberately absent,
//! is documented on that engine module.

pub mod codegen;

/// Why a tool family is, or is not, offered to a workspace.
///
/// Every row of the engine's tool registry carries one, so a new family cannot
/// arrive resident by omission: the row does not compile without it.
/// [`Gate::Ungated`] is how a family states that it has none. The engine
/// decides whether a gate is open (`llm::tools::gate_is_open`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// No gate. Every workspace can act on this family.
    Ungated,
    /// At least one configured email account.
    EmailAccount,
    /// At least one intent.
    Intent,
    /// A configured image-generation provider.
    ImageProvider,
    /// A configured judgment provider, switched on.
    JudgmentProvider,
    /// The Tree memory module, with its summary trees ready.
    MemoryTree,
    /// Every turn that is not on Tree. A Tree turn never reads Classic's
    /// memory, so a tool acting on it would report work that changed nothing.
    MemoryClassic,
}

/// Wire type of a capability argument. Maps to the JSON-schema `type` the LLM
/// tool advertises and to the CLI flag parser.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArgType {
    Str,
    Int,
    Bool,
    /// A complex JSON value (object / array / union) that scalar `Str`/`Int`/
    /// `Bool` can't express — e.g. a trigger's `run` object or `on` array. The
    /// CLI takes it as a `--flag '<JSON-STRING>'`. The generated command parses
    /// it and rides it on the request body. The SDK facade types it as the
    /// author decides. The LLM grouped tool never derives its schema from a
    /// `Json` arg — diverging domains supply a raw `llm_schema` instead.
    Json,
}

impl ArgType {
    /// JSON-schema `type` keyword for the LLM tool parameter.
    pub fn json_type(self) -> &'static str {
        match self {
            ArgType::Str => "string",
            ArgType::Int => "integer",
            ArgType::Bool => "boolean",
            ArgType::Json => "object",
        }
    }
}

/// Where an argument rides on the HTTP request the surface ultimately calls.
/// Only the CLI/SDK generators care (they build the request); the LLM handler
/// runs in-process and reads the args object directly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArgIn {
    /// `?name=value` query parameter.
    Query,
    /// JSON body field.
    Body,
    /// `:name` path segment substitution.
    Path,
}

/// One argument of one operation.
#[derive(Clone, Copy)]
pub struct Arg {
    /// snake_case canonical name (LLM property name + CLI `--flag` + SDK param).
    pub name: &'static str,
    pub ty: ArgType,
    /// Allowed values for an enum-typed string arg; empty = free-form.
    pub enum_values: &'static [&'static str],
    pub required: bool,
    /// Where it rides on the underlying HTTP request.
    pub loc: ArgIn,
    pub description: &'static str,
}

/// HTTP method the operation maps to (the substrate route).
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

/// One capability: a single verb within a domain (e.g. notifications →
/// `mark_all_read`).
#[derive(Clone, Copy)]
pub struct Operation {
    /// snake_case discriminator value the LLM passes as `action` and the handler
    /// matches on (e.g. `mark_all_read`).
    pub action: &'static str,
    /// One-line description, reused in the LLM op list, CLI help, and SDK doc.
    pub summary: &'static str,
    pub method: Method,
    /// Path AFTER `/api/v1` (e.g. `/notifications/read-all`). `:name` segments
    /// are filled from `ArgIn::Path` args. An LLM-only operation may have no
    /// HTTP route of its own (trigger `pause` folds into a PUT). Its path is
    /// then the route the verb maps to. The CLI/SDK generators never read it,
    /// because the op is `cli`/`sdk` = false.
    pub path: &'static str,
    /// Wire arguments that ride on the underlying HTTP request — the source of
    /// truth for the CLI flags and SDK params. The grouped LLM tool's schema
    /// comes from `llm_schema` (below) when the LLM shape diverges. Otherwise
    /// it is derived from these args.
    pub args: &'static [Arg],
    /// kebab-case CLI sub-subcommand (e.g. `read-all`).
    pub cli_name: &'static str,
    /// camelCase SDK method name (e.g. `markAllRead`).
    pub sdk_name: &'static str,
    /// Whether the operation mutates state (drives actor stamping + CLI hints).
    pub mutating: bool,
    /// The retired flat LLM tool name this operation supersedes (e.g.
    /// `create_trigger`). It has two roles:
    ///
    /// - The grouped tool's handler maps `action` to this legacy name and
    ///   delegates to the existing per-verb handler, with no logic rewrite.
    /// - The legacy name keeps resolving to this domain via [`domain_for_tool`],
    ///   so cached prompts and threads still work.
    ///
    /// `None` for brand-new operations with no predecessor.
    ///
    /// It doubles as the **handler key** in every domain that dispatches by
    /// flat name, which is most of them. `triggers`, `trigger_groups` and
    /// `preferences` have bespoke handler arms (`engine/tools/mod.rs`). The
    /// generic `grouped_legacy_name` path covers the rest (`mcp`, `plugins`,
    /// `events`, `changes`, `threads`, `thread_queue`, `memory`). It also
    /// resolves the action via [`Domain::legacy_tool_for_action`]. Either way, a
    /// new operation in a grouped domain needs a name here, predecessor or not.
    /// Without one the dispatch rejects it as an unknown action.
    pub llm_alias: Option<&'static str>,
    /// Raw JSON *properties object* this operation contributes to the grouped
    /// LLM tool schema, used verbatim when the LLM-facing shape diverges from
    /// `args` — e.g. a trigger's `cron` (string|array shorthand) vs the HTTP
    /// `cron_expressions` (array), or omitting a context-injected `device_id`.
    /// `None` = derive the LLM properties from `args` (aligned ops like
    /// notifications). Must be a JSON object (`{ "name": { …schema… }, … }`).
    pub llm_schema: Option<&'static str>,
    /// Per-operation surface overrides. `None` inherits the [`Domain`] flag;
    /// `Some(false)` removes this op from that surface even though the domain is
    /// on it (e.g. trigger `pause`/`resume` are LLM-only conveniences with no
    /// dedicated CLI/SDK route). `Some(true)` is rarely needed but symmetric.
    pub llm: Option<bool>,
    pub cli: Option<bool>,
    pub sdk: Option<bool>,
}

impl Operation {
    /// Whether this operation is exposed on the LLM grouped tool.
    pub fn on_llm(&self, domain: &Domain) -> bool {
        self.llm.unwrap_or(domain.llm)
    }
    /// Whether this operation generates a CLI sub-subcommand.
    pub fn on_cli(&self, domain: &Domain) -> bool {
        self.cli.unwrap_or(domain.cli)
    }
    /// Whether this operation appears in the SDK capability table.
    pub fn on_sdk(&self, domain: &Domain) -> bool {
        self.sdk.unwrap_or(domain.sdk)
    }
}

/// A domain groups its operations into one LLM tool, one CLI subcommand, and one
/// SDK namespace.
#[derive(Clone, Copy)]
pub struct Domain {
    /// Canonical domain name (e.g. `notifications`) — CLI top-level subcommand,
    /// SDK namespace.
    pub name: &'static str,
    /// Grouped LLM tool name (usually == `name`).
    pub tool_name: &'static str,
    /// Top-level description of the grouped LLM tool.
    pub tool_summary: &'static str,
    pub llm: bool,
    pub cli: bool,
    pub sdk: bool,
    pub operations: &'static [Operation],
    /// Retired flat LLM tool names that still dispatch to this domain (back-compat
    /// aliases so existing prompts/threads keep working after consolidation).
    pub llm_aliases: &'static [&'static str],
    /// Whether a workspace's chat turns are offered the grouped tool. A gated
    /// domain still has its routes and CLI commands in every workspace.
    pub llm_gate: Gate,
}

impl Domain {
    /// The set of `action` discriminator values the grouped LLM tool accepts —
    /// only operations exposed on the LLM surface.
    pub fn actions(&self) -> Vec<&'static str> {
        self.operations
            .iter()
            .filter(|o| o.on_llm(self))
            .map(|o| o.action)
            .collect()
    }

    /// Map a grouped-tool `action` to the legacy flat tool name its handler
    /// delegates to (e.g. `create` → `create_trigger`). `None` when the action
    /// is unknown or has no legacy predecessor.
    pub fn legacy_tool_for_action(&self, action: &str) -> Option<&'static str> {
        self.operations
            .iter()
            .find(|o| o.action == action)
            .and_then(|o| o.llm_alias)
    }

    /// Every legacy flat LLM tool name that resolves to this domain — the
    /// per-operation `llm_alias` values for LLM-exposed ops, plus any
    /// domain-level extras in `llm_aliases`.
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

// ---------------------------------------------------------------------------
// The manifest. Add a capability here and the generated surfaces + the parity
// tests follow. Keep entries grouped by domain.
// ---------------------------------------------------------------------------

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
        // `read_notifications` was the pre-consolidation flat tool (list-only).
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

// ---------------------------------------------------------------------------
// preferences — get/set user settings. The LLM `get_preferences`/`set_preference`
// tools omit `device_id`, which comes from the calling device's context. So
// both ops supply a raw `llm_schema`, and the HTTP `args` carry `device_id` for
// the CLI/SDK. See engine/tools/preferences.rs.
// ---------------------------------------------------------------------------

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
        // The LLM tool takes no args — the calling device is injected.
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
        // Device-scoped keys auto-apply to the calling device — the LLM never
        // passes a device id, so the grouped tool omits it (unlike CLI/SDK).
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
};

// ---------------------------------------------------------------------------
// triggers — create/list/update/delete + pause/resume. The HTTP body shape
// (cron_expressions array, on array-of-objects, slug, side_effect_grant) drives
// the CLI/SDK. The grouped LLM tool keeps the chat agent's shorthand shape
// (cron string|array, on shorthand, run object) via raw llm_schema. It
// delegates each action to the existing execute_scheduler_tool handler.
// pause/resume have no dedicated HTTP route (the engine folds them into a PUT),
// so they're LLM-only (cli/sdk = false). See engine/tools/scheduler.rs.
// ---------------------------------------------------------------------------

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
    description:
        "Irreversible side-effect categories this trigger may perform unattended, e.g. [\"email\"].",
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
const TRIGGER_MODEL_ARG: Arg = Arg {
    name: "model",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description:
        "Chat model id this trigger's intent runs on. Omit or null for the account default.",
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

// The grouped LLM tool keeps the existing flat-tool shapes (shorthand cron/on,
// run object) so execute_scheduler_tool reads the args unchanged. `cron`/`on`
// allow null so the unioned property serves both create and update (clearing).
pub const TRIGGER_CREATE_LLM_SCHEMA: &str = r#"{
  "name": {"type":"string","description":"Short and descriptive."},
  "run": {"type":"object","description":"{ type: 'intent', intent: '…' } in the user's voice with the procedure left to knowhow, or { type: 'script', path: 'name/run.py' }."},
  "cron": {"description":"6 fields in the USER'S LOCAL TIME (second minute hour day-of-month month day-of-week); '0 0 8 * * *' is 8am daily. Fields AND within one expression, expressions OR across the array. A string, an array, or null.","oneOf":[{"type":"string"},{"type":"array","items":{"type":"string"},"minItems":1},{"type":"null"}]},
  "on": {"description":"Each { event_type: 'X', condition?: {…} }, operators $eq/$ne/$lt/$lte/$gt/$gte/$in/$nin/$regex plus $or. A condition key is a field path: 'workflow_run.event'. A string, an array, or null.","anyOf":[{"type":"null"},{"type":"string"},{"type":"array","items":{"anyOf":[{"type":"string"},{"type":"object","properties":{"event_type":{"type":"string"},"condition":{"type":"object"}},"required":["event_type"]}]}}]},
  "app_id": {"anyOf":[{"type":"null"},{"type":"string"}],"description":"Owning app directory name; notifications deep-link there. Null for standalone."},
  "go_to_review": {"type":"boolean","description":"Threads this trigger spawns land in REVIEW, not ARCHIVE. Default false."},
  "group_id": {"anyOf":[{"type":"null"},{"type":"string"}],"description":"Trigger-group id, organizational only. Null for ungrouped."},
  "mcp_servers": {"anyOf":[{"type":"null"},{"type":"array","items":{"type":"string"}}],"description":"MCP server ids its fires call; any other is refused."},
  "model": {"anyOf":[{"type":"null"},{"type":"string"}],"description":"Chat model id for the intent, e.g. 'claude-sonnet-5'. Null = account default (on update, clears a pin). Set only if asked."},
  "reasoning_effort": {"anyOf":[{"type":"null"},{"type":"string","enum":["none","low","medium","high","xhigh","max"]}],"description":"Thinking budget for the intent. Null = account default (on update, clears a pin)."},
  "provider": {"anyOf":[{"type":"null"},{"type":"string","enum":["vertex","anthropic","openai","openrouter","xai","opencode-free","local"]}],"description":"Backend for the pinned model, one of its routes; needs model. Null = its preferred provider. Set only if asked."}
}"#;
// `model` / `reasoning_effort` / `provider` are deliberately NOT repeated here.
// Properties are unioned across a domain's operations first-wins (see
// `build_llm_tool`). So a second copy under the same name is dropped before the
// model sees it, and only the create schema's wording would ship. Update's
// null-clears semantics is stated there instead.
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
            TRIGGER_SIDE_EFFECT_GRANT_ARG,
            TRIGGER_MCP_SERVERS_ARG,
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
            TRIGGER_MCP_SERVERS_ARG,
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
        // LLM-only: no dedicated HTTP route (the engine folds pause into a PUT).
        // CLI/SDK users pause via the `update` op's `paused` field.
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
        // Not a retired flat tool: `run` is new, and the name is the handler key
        // the `triggers` domain dispatches on (see `grouped_legacy_name`).
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
};

// ---------------------------------------------------------------------------
// trigger_groups — user-visible folders that organize triggers in the panel.
// Pure organizational label; no firing. The HTTP `update` route covers both
// rename and reorder; the LLM surface keeps them as distinct rename/reorder
// actions (mapping to PUT and POST /reorder). No SDK consumer → sdk = false.
// ---------------------------------------------------------------------------

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
    // No app/SDK consumer manages trigger groups — declared N/A (parity is per
    // surface, not blanket). LLM + CLI cover the agent + subprocess paths.
    sdk: false,
    operations: TRIGGER_GROUPS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
};

// ---------------------------------------------------------------------------
// apps — list/get/update/delete app lifecycle + metadata. Asymmetric across
// surfaces (declared parity, not blanket). App *creation* is the LLM-only
// `create_app` tool: a file write with a large html_content arg, no HTTP route,
// kept standalone per the hot-single-purpose-tool guardrail. Editing app
// *source* is the app-coding-agent's worktree job. So this domain is `llm`-false
// and carries no create/source ops.
//
// It closes the real gap: a subprocess/chat
// agent can `lucidos apps list|get|update|delete` instead of reverse-engineering
// curl. `list`/`get` are also in the SDK (facade already present); `update`/
// `delete` are CLI-only. See api/apps.rs + engine/tools/apps.rs.
// ---------------------------------------------------------------------------

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
        // No SDK consumer renames apps; CLI-only (parity is per surface).
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
    // LLM keeps the standalone create_app + list_apps tools; nothing to group
    // here (create has no HTTP peer). This domain enforces CLI + SDK parity.
    llm: false,
    cli: true,
    sdk: true,
    operations: APPS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
};

// ---------------------------------------------------------------------------
// events — domain-event emit/query/count. Consolidates the three flat tools into
// one grouped LLM tool; each action delegates to the existing
// execute_emit_event/execute_query_events/execute_count_events via the flat
// alias. LLM-only: the `lucidos events` CLI is a richer hand-written command
// (pagination cursors) that the generator can't reproduce, so cli/sdk = false.
// See engine/tools/mod.rs.
// ---------------------------------------------------------------------------

const EVENTS_EMIT_LLM_SCHEMA: &str = r#"{
  "event_type": {"type":"string","description":"PascalCase past tense, e.g. GoogleDocEdited."},
  "payload": {"type":"object","description":"REQUIRED. Enough context to understand what happened.","properties":{"summary":{"type":"string","description":"What happened, in one line."}},"required":["summary"]}
}"#;
const EVENTS_QUERY_LLM_SCHEMA: &str = r#"{
  "event_id": {"type":"string","description":"One event by id: the 'evt-<32 hex>' a tool result ends with, or a bare uuid. A tool call's address returns the pair, call then result. Errors if nothing matches."},
  "event_type": {"type":"string","description":"Omitting it queries all, worth avoiding on a busy workspace."},
  "thread_id": {"type":"string","description":"One thread: 'current' is this one. Read it back with event_type 'MessageReceived'."},
  "since": {"type":"string","description":"After this RFC 3339 timestamp."},
  "until": {"type":"string","description":"Before this RFC 3339 timestamp."},
  "limit": {"type":"integer","description":"1-200, default 50. Raise only to fully enumerate a small type."},
  "byte_limit": {"type":"integer","description":"Response byte budget (1024-524288, default 131072). On truncation follow the hint and narrow the query before raising it."}
}"#;
const EVENTS_COUNT_LLM_SCHEMA: &str = r#"{
  "event_type": {"type":"string","description":"Omit for a per-type breakdown across all types."},
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
    // The `lucidos events` CLI is a richer hand-written command (before/after
    // cursors); not regenerated. No SDK consumer. Grouped LLM tool only.
    cli: false,
    sdk: false,
    operations: EVENTS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
};

// ---------------------------------------------------------------------------
// changes — list/apply pending coding-agent changes. Consolidates the two flat
// tools into one grouped LLM tool; delegates to execute_list_changes /
// execute_apply_change via the flat alias. LLM-only: `lucidos changes` is a
// hand-written CLI, so cli/sdk = false. See engine/tools/mod.rs.
// ---------------------------------------------------------------------------

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
    // `lucidos changes list|apply` is a hand-written CLI; not regenerated. No SDK
    // consumer. Grouped LLM tool only.
    cli: false,
    sdk: false,
    operations: CHANGES_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
};

// ---------------------------------------------------------------------------
// models — chat-model registry (Settings → Models). The grouped
// `manage_models` LLM tool keeps its name, and its schema is built from the
// manifest. The domain also generates a `lucidos models` CLI over the /models
// CRUD. execute_tool routes manage_models to execute_manage_models, which reads
// `action` itself, so there is no grouped-alias delegation.
//
// LLM actions (list/add/enable/disable/update/remove) and CLI ops
// (list/add/update/delete) diverge: enable/disable are LLM-only PUT
// conveniences. `id` is a Body arg for `add` and a Query arg for update/delete
// (two Args, same name/type). `routes` is a Json arg, which the LLM tool never
// derives, so `add` and `update` carry a raw `llm_schema`. See
// engine/tools/models.rs + api/settings.rs.
// ---------------------------------------------------------------------------

/// Every provider name, in `ProviderKind::ALL` order. A test holds the two in
/// lockstep, and holds every raw `llm_schema` spelling of it to this list.
pub const MODEL_PROVIDER_ENUM: &[&str] = &[
    "vertex",
    "anthropic",
    "openai",
    "openrouter",
    "xai",
    "opencode-free",
    "local",
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
const MODEL_ENABLED_ARG: Arg = Arg {
    name: "enabled",
    ty: ArgType::Bool,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Whether the model is enabled (shown in the picker).",
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

// `routes` is a Json arg, so the LLM shape of `add` is spelled out here. `id`
// keeps the structure `args` would derive, since enable / disable / remove still
// derive it and the union must agree. Only the top-level `provider` spells the
// enum: the handler refuses an unknown name in a route with the list.
pub const MODELS_ADD_LLM_SCHEMA: &str = r#"{
  "id": {"type":"string","description":"Model id, the API string unless a route overrides it. All but list need it."},
  "label": {"type":"string","description":"Display name; defaults to the id."},
  "provider": {"type":"string","enum":["vertex","anthropic","openai","openrouter","xai","opencode-free","local"],"description":"Single-route shorthand for the first route. Add needs it or routes."},
  "sort_order": {"type":"integer","description":"Lower sorts first."},
  "context_window": {"anyOf":[{"type":"null"},{"type":"integer"}],"description":"First route's window in tokens; omitted, most non-Claude ids get 200k. Null clears."},
  "routes": {"type":"array","description":"Backends in priority order. Replaces the whole list.","items":{"type":"object","properties":{"provider":{"type":"string"},"id":{"type":"string","description":"This backend's id if different, e.g. 'anthropic/claude-opus-5-5'."},"context_window":{"type":"integer"}},"required":["provider"]}},
  "vision": {"type":"boolean","description":"Reads images; only such models describe images."}
}"#;
// The properties `add` already declares are not repeated: the union is
// first-wins, so a second copy would be dropped unseen.
const MODELS_UPDATE_LLM_SCHEMA: &str = r#"{
  "preferred_provider": {"anyOf":[{"type":"null"},{"type":"string"}],"description":"Backend to use when several routes are configured. Null clears."}
}"#;

const MODELS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "Every model, enabled or not, builtin or user.",
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
        summary: "Show it in the picker.",
        method: Method::Put,
        path: "/models",
        args: &[MODEL_ID_QUERY_ARG],
        cli_name: "enable",
        sdk_name: "enable",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        // LLM-only convenience (the in-process handler toggles enabled); the CLI
        // uses `update --enabled true|false` instead.
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "disable",
        summary: "Hide it from the picker; builtins cannot be deleted.",
        method: Method::Put,
        path: "/models",
        args: &[MODEL_ID_QUERY_ARG],
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
            MODEL_ENABLED_ARG,
            MODEL_CONTEXT_WINDOW_ARG,
            MODEL_ROUTES_ARG,
            MODEL_PREFERRED_PROVIDER_ARG,
            MODEL_VISION_ARG,
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
        args: &[MODEL_ID_QUERY_ARG],
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
    // Keep the existing LLM tool name so cached prompts/threads don't churn; the
    // schema is now built from this manifest entry (no more get_manage_models_tool).
    tool_name: "manage_models",
    tool_summary: "The chat-model registry behind the Lucidos Agent's model picker. A builtin can be disabled but not removed. Switch the ACTIVE model with set_preference(key='chat_model').",
    llm: true,
    cli: true,
    sdk: false,
    operations: MODELS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
};

// ---------------------------------------------------------------------------
// repositories — registered external git repos for coding-agent sessions.
// The grouped `manage_repositories` LLM tool builds its schema from here, and
// execute_tool routes it to the unchanged execute_manage_repositories handler
// (engine/tools/mod.rs).
//
// The CLI is READ-ONLY. `add` and `remove` carry `cli: Some(false)`, so only
// `list` generates. That is a decision, not a limit: `POST /repositories`
// takes exactly the body `add` declares. Registering a repo changes the
// platform under the user, so a CLI write verb needs its own decision.
//
// `remove`'s path is the conceptual mapping, not an emittable route. The real
// one is `DELETE /api/v1/repositories/:id`, keyed by a path segment rather
// than the body `name` this tool takes. Nothing reads it while the op is off
// both generators.
//
// No SDK: an app frame may not reach `/repositories` at all (ADR 0231).
// ---------------------------------------------------------------------------

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
        // Read-only CLI: see the block comment above `REPO_NAME_ARG`.
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
        // Read-only CLI, and the path here is not the real route either.
        cli: Some(false),
        sdk: None,
    },
];

const REPOSITORIES_DOMAIN: Domain = Domain {
    name: "repositories",
    tool_name: "manage_repositories",
    tool_summary: "External git repositories registered for coding-agent sessions, so a coding agent can work on a local repo.",
    llm: true,
    // `list` only. The two write ops opt out per operation, so a script can
    // read the registry while registering a repo stays the LLM tool's job.
    cli: true,
    // No app frame reaches `/repositories`, so there is nobody to serve.
    sdk: false,
    operations: REPOSITORIES_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
};

// ---------------------------------------------------------------------------
// env_vars — user-managed non-secret environment variables. New generated CLI
// (list / set / delete) closing a real subprocess-agent gap; routed through the
// gateway-safe http client. `llm = false` — the standalone set_environment_variable
// LLM tool stays as-is (the `apps` precedent: CLI parity without touching the LLM
// surface). The CLI subcommand is `env-vars` (kebab, derived from the snake
// domain name so the generated `dispatch_env_vars` ident is valid). The full
// CRUD lives at /env-vars (GET/POST/DELETE). See api/settings.rs.
// ---------------------------------------------------------------------------

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
        // The retired standalone tool this `set` action supersedes — kept wired
        // as a back-compat alias so cached prompts/in-flight threads still work.
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
    // Full LLM/CLI parity (list/set/delete). The retired standalone
    // set_environment_variable tool stays wired as a back-compat alias to the
    // `set` action (see ENV_VARS_OPS). No SDK consumer (apps don't manage env vars).
    llm: true,
    cli: true,
    sdk: false,
    operations: ENV_VARS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
};

// ---------------------------------------------------------------------------
// widgets: a thread's widgets, its shelf and reusable widgets (ADRs 0402, 0407,
// 0415). Show, pin and unpin name a widget instance: the widget and its params.
// Every widget action the UI offers has an action here, for the agent and the
// CLI. Open in Canvas is navigate_ui to the widget's id. `thread_id` is
// optional so the CLI can default it to the session's own thread; the HTTP
// route requires it.
// ---------------------------------------------------------------------------

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
const WIDGET_LABEL_ARG: Arg = Arg {
    name: "label",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Chip label. Default: the widget's name.",
};
/// What pin and unpin name: one widget instance in one thread.
const WIDGET_INSTANCE_ARGS: &[Arg] = &[
    WIDGET_APP_ID_ARG,
    WIDGET_THREAD_BODY_ARG,
    WIDGET_PARAMS_ARG,
    WIDGET_LABEL_ARG,
];
const WIDGET_SHOWN_EVENT_ARG: Arg = Arg {
    name: "shown_event_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Reply turn's event id, for Show in thread.",
};
/// A pin names its instance, and may name the reply that draws it.
const WIDGET_PIN_ARGS: &[Arg] = &[
    WIDGET_APP_ID_ARG,
    WIDGET_THREAD_BODY_ARG,
    WIDGET_PARAMS_ARG,
    WIDGET_LABEL_ARG,
    WIDGET_SHOWN_EVENT_ARG,
];
const WIDGET_THREAD_QUERY_ARG: Arg = Arg {
    name: "thread_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Thread uuid. Default: yours.",
};

pub const HOME_BOARD_LAYOUT_QUERY_ARG: Arg = Arg {
    name: "layout",
    ty: ArgType::Str,
    enum_values: &["phone", "desktop"],
    required: true,
    loc: ArgIn::Query,
    description: "Layout class.",
};
const HOME_BOARD_LAYOUT_BODY_ARG: Arg = Arg {
    loc: ArgIn::Body,
    ..HOME_BOARD_LAYOUT_QUERY_ARG
};
const HOME_BOARD_TILES_ARG: Arg = Arg {
    name: "tiles",
    ty: ArgType::Json,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "Tiles in board order: [{app_id, params?, size, hidden?}].",
};
/// The arrange action's LLM shape: `tiles` is an array, which a `Json` arg
/// cannot describe.
pub const HOME_BOARD_ARRANGE_LLM_SCHEMA: &str = r#"{"layout":{"type":"string","enum":["phone","desktop"]},"tiles":{"type":"array","description":"Home's pins in order, each at a declared size.","items":{"type":"object","properties":{"app_id":{"type":"string"},"params":{"type":"object"},"size":{"enum":["small","medium","large"]},"hidden":{"type":"boolean"}},"required":["app_id","size"]}}}"#;

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
        "thread",
        "Every widget in a thread.",
        Method::Get,
        "/widgets/thread",
        &[WIDGET_THREAD_QUERY_ARG],
        "thread",
    ),
    widget_op(
        "pin",
        "Add its chip to the shelf.",
        Method::Post,
        "/widgets/pin",
        WIDGET_PIN_ARGS,
        "pin",
    ),
    widget_op(
        "unpin",
        "Remove its chip. The card stays.",
        Method::Post,
        "/widgets/unpin",
        WIDGET_INSTANCE_ARGS,
        "unpin",
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
        "board",
        "Read the Home board.",
        Method::Get,
        "/home-board",
        &[HOME_BOARD_LAYOUT_QUERY_ARG],
        "board",
    ),
    Operation {
        llm_schema: Some(HOME_BOARD_ARRANGE_LLM_SCHEMA),
        ..widget_op(
            "arrange",
            "Arrange the Home board, only when asked this turn.",
            Method::Post,
            "/widgets/arrange",
            &[HOME_BOARD_LAYOUT_BODY_ARG, HOME_BOARD_TILES_ARG],
            "arrange",
        )
    },
];

const WIDGETS_DOMAIN: Domain = Domain {
    name: "widgets",
    tool_name: "widgets",
    tool_summary:
        "A thread's widgets, shelf and reusable widgets. Make one with create_app kind='widget'.",
    llm: true,
    cli: true,
    sdk: false,
    operations: WIDGETS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
};

// ---------------------------------------------------------------------------
// threads — thread-summary introspection. Groups the two flat read tools
// (list_threads / count_threads) into one grouped LLM tool, delegating to the
// existing handlers via the flat alias. The spawn tools (run_thread /
// run_coding_agent) stay STANDALONE per the hot-single-purpose guardrail — they
// run through handle_special_tool, not execute_tool, and are not folded here.
// LLM-only: the `lucidos threads list|count` CLI is hand-written, so cli/sdk =
// false. See engine/tools/mod.rs + llm/tools/threads.rs.
// ---------------------------------------------------------------------------

// The `status` enum in both schemas below is spelled out rather than composed,
// because `llm_schema` is a `const` JSON literal. It is pinned to
// `ThreadStatus::ALL` by `threads_status_enum_matches_the_thread_status_enum`.
pub const THREADS_LIST_LLM_SCHEMA: &str = r#"{
  "active": {"type":"boolean","description":"UNION of running and waiting_for_user_answer; false inverts. For 'is the workspace busy?' use status ['running']: a thread awaiting an answer is blocked on the human, not working."},
  "status": {"type":"array","items":{"type":"string","enum":["idle","running","waiting","waiting_for_user_answer","paused","failed"]},"description":"Exactly these, the values each row's status carries. Precise form of active; passing both errors."},
  "source": {"type":"string","description":"Comma-separated 'chat', 'trigger', 'coding-agent' (legacy 'claude_code' accepted). Omit for all."},
  "my_children": {"type":"boolean","description":"Restrict to this thread's DIRECT children, not grandchildren; resolved from the calling thread, so no id. How you recover a child's thread_id."},
  "has_draft": {"type":"boolean","description":"Holds an unsent draft; false inverts."},
  "change_state": {"type":"string","enum":["none","unproposed","proposed"],"description":"Coding-agent branch work."},
  "limit": {"type":"integer","description":"1-1000, default 100."}
}"#;
pub const THREADS_COUNT_LLM_SCHEMA: &str = r#"{
  "active": {"type":"boolean","description":"UNION of running and waiting_for_user_answer; false inverts. Omit for the total. Nonzero does NOT mean work is in flight: for 'is anything still running?' use status ['running']."},
  "status": {"type":"array","items":{"type":"string","enum":["idle","running","waiting","waiting_for_user_answer","paused","failed"]},"description":"Count exactly these; passing both errors."},
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
        summary: "Thread summaries newest-first: thread_id, title, channel, status, section, parent_thread_id, has_draft, draft_preview, link.",
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
        // `thread_id` and `limit` are already in the union, from `detach_child`
        // and `list`. A schema here would only shadow or repeat them.
        llm_schema: None,
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
        summary: "Find past threads by what was SAID in them (text plus semantic), which \
                  'list' cannot: it filters by status and channel, never by topic. Read one \
                  via `events` 'query' with its thread_id. (requires: q)",
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
        llm_schema: Some(
            r#"{"thread_id":{"type":"string","description":"The child's uuid."}}"#,
        ),
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "archive",
        summary: "Archive YOUR thread ('current', after this turn) or a direct child of yours \
                  (now). The Archive button's refusals apply. (requires: thread_id)",
        method: Method::Post,
        path: "/threads/:thread_id/archive",
        args: &[],
        cli_name: "archive",
        sdk_name: "archive",
        mutating: true,
        llm_alias: Some("archive_thread"),
        llm_schema: Some(
            r#"{"thread_id":{"type":"string","description":"'current', or a direct child's uuid."}}"#,
        ),
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
        // In-process, LLM-only: it records the proposal on the calling thread.
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "apply_triage",
        summary: "Apply what the user approved from this thread's newest triage. (requires: entries)",
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
    tool_summary: "Read threads, cheaper than querying events for what exists, its status and its unsent draft; each row has a link to paste. Stop awaiting a child, archive one, or triage the inbox. 'list' and 'count' share filters. To START a thread use run_thread or run_coding_agent, to REDIRECT one follow_up_child_thread.",
    llm: true,
    // The `lucidos threads list|count` CLI is hand-written (kept, not regenerated)
    // and no SDK consumer needs this. Grouped LLM tool only.
    cli: false,
    sdk: false,
    operations: THREADS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
};

// ---------------------------------------------------------------------------
// memory: long-term memory. Mixed surfaces (declared parity per op). The LLM
// gets CORRECTION: correct and correct_by_id run in-process, so they have no
// HTTP route and are cli/sdk = false. It also gets two READS, `search` and
// `source`. The CLI gets every read.
//
// Memory is injected into context, and injection is the primary path. It cannot
// recover from a bad guess, so the two LLM reads are the backstop for the misses
// that `SUB_QUERY_PROMPT` leaves.
//
// `stats` and `entries` stay CLI-only, deliberately. Paging the whole index and
// reading its statistics are operator reads that never answer a user. They are
// exactly the browsing the summaries above warn off. Correction delegates to
// execute_memory_tool / execute_correct_memory_by_id via the flat alias. See
// engine/tools/memory.rs + api/memory.rs.
// ---------------------------------------------------------------------------

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
        // In-process correction (no HTTP route); LLM-only.
        llm: None,
        cli: Some(false),
        sdk: Some(false),
    },
    Operation {
        action: "correct_by_id",
        summary: "Delete (and optionally replace) ONE memory by its id, the precise path when the [id: <uuid>] is visible. (requires: id)",
        method: Method::Post,
        path: "/memory/correct",
        args: &[],
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
        cli: Some(false),
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
        // CLI-only. A row count tells the agent nothing it can act on, unlike
        // the `source` and `search` ops below, which ARE on the LLM surface.
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
    // A Tree turn reads the summary trees and never this memory, so the tool
    // goes and `recall` takes its slot. The CLI stays, for Classic's own data.
    llm_gate: Gate::MemoryClassic,
};

// ---------------------------------------------------------------------------
// recall: the Tree memory module's recall tools (ADR 0362). The LLM tool is
// offered only once the workspace is on Tree with its trees ready. The routes
// and CLI commands exist everywhere, and answer from whatever is built.
// ---------------------------------------------------------------------------

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

/// The LLM shapes leave out `thread`: a bare id names the calling thread.
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
};

// ---------------------------------------------------------------------------
// thread_queue — the Thread Queue (background admission control). Grouped LLM
// tool (list + update_policy, delegating to the existing flat handlers) PLUS a
// generated CLI (list / run-now / drop). `update_policy` is LLM-only. Its
// handler MERGES the patch with the live policy. The raw
// PUT /thread-queue/policy replaces omitted fields with code defaults instead.
// So a CLI `policy` command would silently reset caps, and none is generated.
//
// run-now/drop are CLI-only (no flat LLM predecessor). See engine/tools/mod.rs +
// api/thread_queue.rs.
// ---------------------------------------------------------------------------

const TQ_ENTRY_ID_ARG: Arg = Arg {
    name: "entry_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Body,
    description: "UUID of the queued entry (from the 'list' action's entries[].id).",
};

// Mirrors the flat update_thread_queue_policy schema (cap_schema + overflow).
// Every field optional — the handler merges the patch with the live policy.
//
// Descriptions stay at what the field NAME does not already say: this schema is
// always-loaded context, billed on every request of every thread, and it sits
// close to its per-tool ceiling. The full semantics of each cap live in
// `system-knowhow/thread-queue.md` § Capacity policy.
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
        // LLM-only: the merge-with-live semantics live in the in-process handler;
        // the raw PUT replaces omitted fields with defaults (would reset caps).
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
        // CLI-only: panel/entry action with no flat LLM predecessor.
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
};

// ---------------------------------------------------------------------------
// mcp — MCP (Model Context Protocol) server management. One grouped LLM tool;
// each action delegates to execute_mcp_management_tool via its flat alias.
// Every verb but setup has its own HTTP route and a generated CLI command;
// setup runs purely in-process. enable/disable act on one thread's enabled
// servers (ADR 0430): the LLM's thread is the caller, the CLI names one.
// sdk = false, since no app manages servers. See engine/tools/mcp.rs and
// api/mcp.rs.
// ---------------------------------------------------------------------------

/// The server id, as a path segment. Every per-server verb takes it.
const MCP_ID_ARG: Arg = Arg {
    name: "id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Path,
    description: "MCP server id, as shown by `list`.",
};

/// The thread whose enabled servers change. The LLM never passes it: its own
/// thread is the one that enables.
const MCP_THREAD_ID_ARG: Arg = Arg {
    name: "thread_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Path,
    description: "Thread uuid whose enabled MCP servers change.",
};

/// `enable` and `disable` take the server id only. The thread is the caller.
const MCP_THREAD_VERB_LLM_SCHEMA: &str =
    r#"{"id":{"type":"string","description":"Server id, as shown in [MCP SERVERS]"}}"#;

const MCP_SETUP_LLM_SCHEMA: &str = r#"{
  "id": {"type":"string","description":"Lowercase with hyphens, e.g. 'blender-mcp'."},
  "name": {"type":"string","description":"Human-readable name."},
  "command": {"type":"string","description":"Command to run the server (e.g. 'npx', 'uvx')."},
  "args": {"type":"array","items":{"type":"string"},"description":"Arguments for the command, e.g. ['blender-mcp']."},
  "env": {"type":"object","additionalProperties":{"type":"string"},"description":"Optional environment variables for the process."}
}"#;

const MCP_OPS: &[Operation] = &[
    Operation {
        action: "setup",
        summary: "Register and connect a server, spawning it and discovering its tools. (requires: id, name, command, args)",
        method: Method::Post,
        path: "/mcp/servers",
        args: &[],
        cli_name: "setup",
        sdk_name: "setup",
        mutating: true,
        llm_alias: Some("setup_mcp_server"),
        llm_schema: Some(MCP_SETUP_LLM_SCHEMA),
        llm: None,
        // The one verb with no CLI command. Registration runs in-process, so
        // there is no HTTP route. Its shape also lives in `llm_schema` rather
        // than `args`, so the generator would emit a flagless command posting
        // an empty body. Declaring the args means building the route first.
        cli: Some(false),
        sdk: None,
    },
    Operation {
        action: "list",
        summary: "List all configured MCP servers with their status (running/stopped) and available tools.",
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
        summary: "Start a stopped server. From a thread, it is enabled there too. (requires: id)",
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
        summary: "Remove an MCP server configuration (stops it first if running). (requires: id)",
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
    // Every verb but `setup` has its own HTTP route, so each generates a CLI
    // command. `setup` is the exception and says why on the op.
    cli: true,
    // No app manages MCP servers: an iframe has no business starting a process
    // for the whole workspace. Declared N/A, which is parity per surface.
    sdk: false,
    operations: MCP_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
};

// ---------------------------------------------------------------------------
// plugins — install/marketplace/update lifecycle. Consolidates the five flat
// tools into one grouped LLM tool; each action delegates to the existing
// execute_plugin_tool via the flat alias. LLM-only: install/uninstall stage a
// confirm-panel handshake (UI plumbing, not a clean CLI), so cli/sdk = false.
// See engine/tools/plugins.
// ---------------------------------------------------------------------------

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
    // install/uninstall are a UI confirm-panel handshake (not a clean CLI); no
    // app/SDK consumer. Declared N/A — the grouped LLM tool is the agent surface.
    cli: false,
    sdk: false,
    operations: PLUGINS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
};

// ---------------------------------------------------------------------------
// webhooks: inbound endpoints the user points a third party at. `llm: false` is
// the load-bearing flag, and it follows the side-effect-grant precedent where
// `create_trigger` deliberately cannot set the grant. An agent must not be able
// to widen its own authority. A webhook opens a publicly reachable door that
// emits a pinned event, so only the user creates one.
//
// `sdk: false` for the same reason, one layer out: an app iframe runs with the
// user's authority, and this is not a capability an app should reach.
// ---------------------------------------------------------------------------

/// The webhook id, as a path segment.
const WEBHOOK_ID_ARG: Arg = Arg {
    name: "id",
    ty: ArgType::Str,
    enum_values: &[],
    required: true,
    loc: ArgIn::Path,
    description: "Webhook UUID, as shown by `list`.",
};

/// Deduping, off unless the hook asks for it.
const WEBHOOK_DEDUPE_ARG: Arg = Arg {
    name: "dedupe",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Recognise a resend instead of emitting twice: {header?, window_secs?}. `header` names the header carrying the sender's delivery id (e.g. 'X-GitHub-Delivery'); with none, the key is a digest of the body. `window_secs` defaults to 3600, is capped at 604800, and 0 switches deduping off. Omit the whole block and every arrival emits, which is what keeps a sender's retries visible on the log.",
};

/// Where the signing secret comes from, when the request brings one.
///
/// Shared between create and update. On update it is a rotation, which the
/// description says, because the two read the same JSON.
const WEBHOOK_SIGNING_SECRET_ARG: Arg = Arg {
    name: "signing_secret",
    ty: ArgType::Json,
    enum_values: &[],
    required: false,
    loc: ArgIn::Body,
    description: "Save the secret the `hmac` block names, instead of pointing at one you saved earlier. `{\"mode\":\"generate\"}` mints 32 bytes and prints them ONCE, which is right for GitHub, where you choose the secret. `{\"mode\":\"provided\",\"value\":\"...\"}` stores a secret the sender issued, which is the only option for Slack and Stripe. The value is stored byte for byte, so a value with surrounding whitespace is refused rather than trimmed. On `create` it refuses to overwrite an existing credential; on `update` it replaces the value, which is how you rotate.",
};

/// Request headers copied into the event payload, under `headers`.
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
    // Deliberately not an agent capability. See the note above this block.
    llm: false,
    cli: true,
    sdk: false,
    operations: WEBHOOKS_OPS,
    llm_aliases: &[],
    llm_gate: Gate::Ungated,
};

// ---------------------------------------------------------------------------
// workspace_prompt_footprint: what the workspace's own content adds to every
// chat turn (ADR 0413). `llm: false` is the point: a tool schema would bill
// every turn of every workspace to measure the prompt. The audit reaches it
// through the CLI it already runs. No app needs it, so no SDK either.
// ---------------------------------------------------------------------------

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
];

/// The full manifest.
pub fn domains() -> &'static [Domain] {
    DOMAINS
}

/// Look up a domain by its grouped LLM tool name, including back-compat aliases
/// (the per-operation `llm_alias` values and any domain-level `llm_aliases`).
pub fn domain_for_tool(tool_name: &str) -> Option<&'static Domain> {
    DOMAINS
        .iter()
        .find(|d| (d.llm && d.tool_name == tool_name) || d.alias_names().contains(&tool_name))
}
