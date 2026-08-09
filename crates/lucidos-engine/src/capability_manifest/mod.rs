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
//! - the CLI command table is generated into `crates/lucidos-cli/src/generated/`
//!   and the SDK capability table into `packages/lucidos-sdk/src/generated/`
//!   (see the `codegen` submodule), each guarded by a staleness test that fails
//!   `cargo test` when the on-disk file falls behind the manifest — the same
//!   pattern as `navigate_targets_codegen` in `llm/tools/misc.rs`.
//!
//! Adding an operation here forces the generated surfaces to follow (staleness
//! test) and forces a handler (the handler's recognised-action set is checked
//! against the manifest by a unit test — see `engine/tools/notifications.rs`).
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
//! No SDK either: an app iframe is not a thread and holds no subscriptions of
//! its own. The capability an app or a script actually wants there is the
//! `triggers` domain, which IS in the manifest: a standing rule that reacts to
//! an event with no thread to resume.
//!
//! This note used to argue that parity was unreachable because `await_event`
//! ended the turn and a CLI invocation had no turn to park. ADR 0049 retired
//! that shape (a subscription holds nothing, and the wake is an ordinary new
//! turn), which is exactly what made the CLI verbs possible.

use crate::llm::provider::ToolDefinition;
use serde_json::{Map, Value};

#[cfg(test)]
mod codegen;

/// Wire type of a capability argument. Maps to the JSON-schema `type` the LLM
/// tool advertises and to the CLI flag parser.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArgType {
    Str,
    Int,
    Bool,
    /// A complex JSON value (object / array / union) that scalar `Str`/`Int`/
    /// `Bool` can't express — e.g. a trigger's `run` object or `on` array. The
    /// CLI takes it as a `--flag '<JSON-STRING>'` that the generated command
    /// parses and rides on the request body; the SDK facade types it as the
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
    /// are filled from `ArgIn::Path` args. For an LLM-only operation that has no
    /// HTTP substrate of its own (e.g. trigger `pause`, which the engine folds
    /// into a PUT), this is the route the verb conceptually maps to; the CLI/SDK
    /// generators never read it because the op is `cli`/`sdk` = false.
    pub path: &'static str,
    /// Wire arguments that ride on the underlying HTTP request — the source of
    /// truth for the CLI flags and SDK params. The grouped LLM tool's schema
    /// comes from `llm_schema` when the LLM shape diverges from these (see
    /// below), or is derived from these args when it doesn't.
    pub args: &'static [Arg],
    /// kebab-case CLI sub-subcommand (e.g. `read-all`).
    pub cli_name: &'static str,
    /// camelCase SDK method name (e.g. `markAllRead`).
    pub sdk_name: &'static str,
    /// Whether the operation mutates state (drives actor stamping + CLI hints).
    pub mutating: bool,
    /// The retired flat LLM tool name this operation supersedes (e.g.
    /// `create_trigger`). Two roles: (1) the grouped tool's handler maps
    /// `action` → this legacy name and delegates to the existing per-verb
    /// handler — no logic rewrite; (2) the legacy name keeps resolving to this
    /// domain via [`domain_for_tool`] so cached prompts/threads still work.
    /// `None` for brand-new operations with no predecessor.
    ///
    /// It doubles as the **handler key** in every domain that dispatches by
    /// flat name, which is most of them. `triggers`, `trigger_groups` and
    /// `preferences` have bespoke handler arms
    /// (`engine/tools/mod.rs`); the generic `grouped_legacy_name` path covers
    /// the rest (`mcp`, `plugins`, `events`, `changes`, `threads`,
    /// `thread_queue`, `memory`), and it also resolves the action via
    /// [`Domain::legacy_tool_for_action`]. Either way a new operation in a
    /// grouped domain needs a name here even with no predecessor to supersede,
    /// or the dispatch rejects it as an unknown action. (This paragraph named
    /// only the three bespoke domains until 2026-08-05, which read as an
    /// exhaustive list and made the alias look like decoration everywhere
    /// else.)
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
// tools omit `device_id` (it's injected from the calling device's context), so
// both ops supply a raw `llm_schema`; the HTTP `args` carry `device_id` for the
// CLI/SDK. See engine/tools/preferences.rs.
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
    description: "e.g. 'theme', 'language', 'timezone', 'chat_model'.",
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
              "key": {"type":"string","description":"e.g. 'theme', 'language', 'timezone', 'chat_model'. The 'get' action lists every settable key."},
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
};

// ---------------------------------------------------------------------------
// triggers — create/list/update/delete + pause/resume. The HTTP body shape
// (cron_expressions array, on array-of-objects, slug, side_effect_grant) drives
// the CLI/SDK; the grouped LLM tool keeps the shorthand shape the chat agent
// already uses (cron string|array, on shorthand, run object) via raw llm_schema,
// and delegates each action to the existing execute_scheduler_tool handler.
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
    description: "Event subscriptions, e.g. [{\"event_type\":\"X\",\"condition\":{...}}].",
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
const TRIGGER_CREATE_LLM_SCHEMA: &str = r#"{
  "name": {"type":"string","description":"Short and descriptive."},
  "run": {"type":"object","description":"{ type: 'intent', intent: '…' } in the user's voice with the procedure left to knowhow, or { type: 'script', path: 'name/run.py' }."},
  "cron": {"description":"6 fields in the USER'S LOCAL TIME (second minute hour day-of-month month day-of-week); '0 0 8 * * *' is 8am daily. Fields AND within one expression, expressions OR across the array. A string, an array, or null.","oneOf":[{"type":"string"},{"type":"array","items":{"type":"string"},"minItems":1},{"type":"null"}]},
  "on": {"description":"Each { event_type: 'X', condition?: {…} }, operators $eq/$ne/$lt/$lte/$gt/$gte/$in. A string, an array, or null.","anyOf":[{"type":"null"},{"type":"string"},{"type":"array","items":{"anyOf":[{"type":"string"},{"type":"object","properties":{"event_type":{"type":"string"},"condition":{"type":"object"}},"required":["event_type"]}]}}]},
  "app_id": {"anyOf":[{"type":"null"},{"type":"string"}],"description":"Owning app directory name; notifications deep-link there. Null for standalone."},
  "go_to_review": {"type":"boolean","description":"Threads this trigger spawns land in REVIEW, not ARCHIVE. Default false."},
  "group_id": {"anyOf":[{"type":"null"},{"type":"string"}],"description":"Trigger-group id, organizational only. Null for ungrouped."},
  "model": {"anyOf":[{"type":"null"},{"type":"string"}],"description":"Chat model id for the intent, e.g. 'claude-sonnet-5'. Null = account default (on update, clears a pin). Set only if asked."},
  "reasoning_effort": {"anyOf":[{"type":"null"},{"type":"string","enum":["none","low","medium","high","xhigh","max"]}],"description":"Thinking budget for the intent. Null = account default (on update, clears a pin)."}
}"#;
// `model` / `reasoning_effort` are deliberately NOT repeated here. Properties
// are unioned across a domain's operations first-wins (see `build_llm_tool`), so
// a second copy under the same name is dropped before the model ever sees it,
// and only the create schema's wording would ship. Update's null-clears
// semantics is stated there instead.
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
            TRIGGER_SLUG_ARG,
            TRIGGER_MODEL_ARG,
            TRIGGER_REASONING_EFFORT_ARG,
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
            TRIGGER_SLUG_ARG,
            TRIGGER_MODEL_ARG,
            TRIGGER_REASONING_EFFORT_ARG,
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
};

// ---------------------------------------------------------------------------
// apps — list/get/update/delete app lifecycle + metadata. Asymmetric across
// surfaces (declared parity, not blanket): app *creation* is the LLM-only
// `create_app` tool (a file write with a large html_content arg — no HTTP route,
// kept standalone per the hot-single-purpose-tool guardrail), and editing app
// *source* is the app-coding-agent's worktree job, so this domain is `llm`-false
// and carries no create/source ops. It closes the real gap: a subprocess/chat
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
  "event_type": {"type":"string","description":"Omitting it queries all, worth avoiding on a busy workspace."},
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
        summary: "Pending and recently-applied changes as { pending, applied, total_pending }. Read .pending[].id before 'apply'.",
        method: Method::Get,
        path: "/changes",
        args: &[],
        cli_name: "list",
        sdk_name: "list",
        mutating: false,
        llm_alias: Some("list_changes"),
        llm_schema: Some("{}"),
        llm: None,
        cli: None,
        sdk: None,
    },
    Operation {
        action: "apply",
        summary: "Merge the coding-agent branch into main, exactly as the Apply button does; returns status, SHAs and restart_required. ONLY when the user asked. (requires: change_id)",
        method: Method::Post,
        path: "/changes/:change_id/apply",
        args: &[],
        cli_name: "apply",
        sdk_name: "apply",
        mutating: true,
        llm_alias: Some("apply_change"),
        llm_schema: Some(r#"{"change_id":{"type":"string","description":"UUID of the pending change to apply. Get it from the 'list' action (.pending[].id)."}}"#),
        llm: None,
        cli: None,
        sdk: None,
    },
];

const CHANGES_DOMAIN: Domain = Domain {
    name: "changes",
    tool_name: "changes",
    tool_summary: "Changes: coding-agent-proposed branches awaiting the Apply button. 'list' is where you find a change's id. Only 'apply' when the user asked.",
    llm: true,
    // `lucidos changes list|apply` is a hand-written CLI; not regenerated. No SDK
    // consumer. Grouped LLM tool only.
    cli: false,
    sdk: false,
    operations: CHANGES_OPS,
    llm_aliases: &[],
};

// ---------------------------------------------------------------------------
// models — chat-model registry (Settings → Models). Migrates the existing
// already-grouped `manage_models` LLM tool INTO the manifest (tool_name kept =
// manage_models, so the LLM name doesn't churn; its schema is now manifest-built
// SSOT, replacing misc::get_manage_models_tool) AND adds a generated `lucidos
// models` CLI over the /models CRUD. execute_tool keeps routing manage_models →
// the unchanged execute_manage_models handler (it reads `action` itself, so no
// grouped-alias delegation). LLM actions (list/add/enable/disable/remove) and
// CLI ops (list/add/update/delete) diverge: enable/disable are LLM-only PUT
// conveniences; `update` is the CLI-only generic PUT. `id` is a Body arg for
// `add` and a Query arg for update/delete (two Args, same name/type). See
// engine/tools/models.rs + api/settings.rs.
// ---------------------------------------------------------------------------

const MODEL_PROVIDER_ENUM: &[&str] = &["vertex", "anthropic", "openai", "openrouter", "local"];

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
    required: true,
    loc: ArgIn::Body,
    description: "Backend that serves the model.",
};
const MODEL_PROVIDER_OPT_ARG: Arg = Arg {
    name: "provider",
    ty: ArgType::Str,
    enum_values: MODEL_PROVIDER_ENUM,
    required: false,
    loc: ArgIn::Body,
    description: "Backend that serves the model.",
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
    description: "Context window in tokens (e.g. 1048576), what the model actually serves. Omitting it guesses from the model id: 1M for an id carrying [1m], 400k for gpt-5*, 200k for everything else including OpenRouter, Gemini and local ids however large they are. The guess errs low on purpose.",
};

const MODELS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "Every model, enabled and disabled, builtin and user.",
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
        summary: "Register a new model in the picker.",
        method: Method::Post,
        path: "/models",
        args: &[
            MODEL_ID_BODY_ARG,
            MODEL_LABEL_ARG,
            MODEL_PROVIDER_ARG,
            MODEL_SORT_ORDER_ARG,
            MODEL_CONTEXT_WINDOW_ARG,
        ],
        cli_name: "add",
        sdk_name: "add",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
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
        summary: "Hide it from the picker; builtins disable, never delete.",
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
        summary: "Edit label, provider, sort_order or enabled.",
        method: Method::Put,
        path: "/models",
        args: &[
            MODEL_ID_QUERY_ARG,
            MODEL_LABEL_ARG,
            MODEL_PROVIDER_OPT_ARG,
            MODEL_SORT_ORDER_ARG,
            MODEL_ENABLED_ARG,
            MODEL_CONTEXT_WINDOW_ARG,
        ],
        cli_name: "update",
        sdk_name: "update",
        mutating: true,
        llm_alias: None,
        llm_schema: None,
        // CLI-only: the LLM uses enable/disable; a generic update isn't exposed.
        llm: Some(false),
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
};

// ---------------------------------------------------------------------------
// repositories — registered external git repos for coding-agent sessions.
// Migrates the already-grouped `manage_repositories` LLM tool into the manifest
// (tool_name kept; schema now manifest-built SSOT, replacing
// misc::get_manage_repositories_tool). Declared LLM-only (cli/sdk = false) so
// the migration adds no new generated surface: the LLM handler reaches
// `RepositoryStore` in-process. HTTP routes for the same verbs DO exist
// (`POST /api/v1/repositories` and `DELETE /api/v1/repositories/:id`, see
// `api::repositories::router`); the `path` values recorded below are the
// conceptual mapping, not something a generator could emit today, since
// `remove` is keyed there by an `:id` path segment rather than by the body
// `name` this tool takes. execute_tool keeps routing manage_repositories to the
// unchanged execute_manage_repositories handler. See engine/tools/mod.rs.
// ---------------------------------------------------------------------------

const REPO_NAME_ARG: Arg = Arg {
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
        cli: None,
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
        cli: None,
        sdk: None,
    },
];

const REPOSITORIES_DOMAIN: Domain = Domain {
    name: "repositories",
    tool_name: "manage_repositories",
    tool_summary: "External git repositories registered for coding-agent sessions, so a coding agent can work on a local repo.",
    llm: true,
    // Declared N/A: the LLM handler runs add/remove in-process against
    // `RepositoryStore`, and this entry is a pure schema-SSOT migration that
    // deliberately ships no generated surface. Not because the routes are
    // missing, they are not: see the block comment above `REPO_NAME_ARG`.
    cli: false,
    sdk: false,
    operations: REPOSITORIES_OPS,
    llm_aliases: &[],
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
    description: "Uppercase letters, digits and underscores, not starting with a digit. An engine-owned name (CRED_*, OAUTH_*, PG*, PATH, LUCIDOS_*) is rejected.",
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
    tool_summary: "Non-secret environment variables injected into every subprocess Lucidos spawns, effective on the next one with no restart. They appear in logs and events, so use request_credential for an API key, token or password.",
    // Full LLM/CLI parity (list/set/delete). The retired standalone
    // set_environment_variable tool stays wired as a back-compat alias to the
    // `set` action (see ENV_VARS_OPS). No SDK consumer (apps don't manage env vars).
    llm: true,
    cli: true,
    sdk: false,
    operations: ENV_VARS_OPS,
    llm_aliases: &[],
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
const THREADS_LIST_LLM_SCHEMA: &str = r#"{
  "active": {"type":"boolean","description":"UNION of running and waiting_for_user_answer; false inverts. For 'is the workspace busy?' use status ['running']: a thread awaiting an answer is blocked on the human, not working."},
  "status": {"type":"array","items":{"type":"string","enum":["idle","running","waiting","waiting_for_user_answer","paused","failed"]},"description":"Exactly these, the values each row's status carries. Precise form of active; passing both errors."},
  "source": {"type":"string","description":"Comma-separated 'chat', 'trigger', 'coding-agent' (legacy 'claude_code' accepted). Omit for all."},
  "my_children": {"type":"boolean","description":"Restrict to this thread's DIRECT children, not grandchildren; resolved from the calling thread, so no id. How you recover a child's thread_id."},
  "limit": {"type":"integer","description":"1-1000, default 100."}
}"#;
const THREADS_COUNT_LLM_SCHEMA: &str = r#"{
  "active": {"type":"boolean","description":"UNION of running and waiting_for_user_answer; false inverts. Omit for the total. Nonzero does NOT mean work is in flight: for 'is anything still running?' use status ['running']."},
  "status": {"type":"array","items":{"type":"string","enum":["idle","running","waiting","waiting_for_user_answer","paused","failed"]},"description":"Count exactly these; passing both errors."},
  "source": {"type":"string","description":"Comma-separated 'chat', 'trigger', 'coding-agent'. Omit for all."},
  "my_children": {"type":"boolean","description":"Restrict to this thread's DIRECT children; resolved from the calling thread, so no id."}
}"#;

const THREADS_OPS: &[Operation] = &[
    Operation {
        action: "list",
        summary: "Thread summaries newest-first: thread_id, title, channel, status, last_activity, parent_thread_id, trigger_id.",
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
];

const THREADS_DOMAIN: Domain = Domain {
    name: "threads",
    tool_name: "threads",
    tool_summary: "Introspect threads, far cheaper than querying events for what exists and its status. Both actions take the same optional filters. To START a thread use run_thread or run_coding_agent, to REDIRECT one follow_up_child_thread.",
    llm: true,
    // The `lucidos threads list|count` CLI is hand-written (kept, not regenerated)
    // and no SDK consumer needs this. Grouped LLM tool only.
    cli: false,
    sdk: false,
    operations: THREADS_OPS,
    llm_aliases: &[],
};

// ---------------------------------------------------------------------------
// memory — long-term memory. Mixed surfaces (declared parity per op): the LLM
// gets a grouped `memory` tool for CORRECTION (correct / correct_by_id, both
// in-process — no HTTP route, so cli/sdk = false on those ops), while the CLI
// gets the READ endpoints (stats / entries / source, GET-only — no LLM tool
// exists for reading memory, which is injected into context instead). Correction
// delegates to the existing execute_memory_tool / execute_correct_memory_by_id
// via the flat alias. See engine/tools/memory.rs + api/memory.rs.
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
const MEM_SOURCE_ID_ARG: Arg = Arg {
    name: "source_id",
    ty: ArgType::Str,
    enum_values: &[],
    required: false,
    loc: ArgIn::Query,
    description: "Event UUID (required when source_type is 'event').",
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
        // CLI-only read: no LLM tool reads memory (it's injected into context).
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
        summary: "One memory's originating event or artifact, plus the entries derived from it.",
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
        llm_alias: None,
        llm_schema: None,
        llm: Some(false),
        cli: None,
        sdk: Some(false),
    },
];

const MEMORY_DOMAIN: Domain = Domain {
    name: "memory",
    tool_name: "memory",
    tool_summary: "Correct long-term memory. Prefer 'correct_by_id' when the [id: <uuid>] is visible. There is no read action: memory is injected into your context.",
    llm: true,
    cli: true,
    sdk: false,
    operations: MEMORY_OPS,
    llm_aliases: &[],
};

// ---------------------------------------------------------------------------
// thread_queue — the Thread Queue (background admission control). Grouped LLM
// tool (list + update_policy, delegating to the existing flat handlers) PLUS a
// new generated CLI (list / run-now / drop). `update_policy` is LLM-only: its
// handler MERGES the patch with the live policy, whereas the raw
// PUT /thread-queue/policy replaces omitted fields with code defaults — a CLI
// `policy` command would silently reset caps, so it's deliberately not generated.
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
const TQ_POLICY_LLM_SCHEMA: &str = r#"{
  "max_concurrent_total": {"type":"integer","minimum":0,"description":"Every kind, background and user work alike."},
  "max_concurrent_event_trigger": {"type":"integer","minimum":0},
  "max_concurrent_cron": {"type":"integer","minimum":0},
  "max_concurrent_sub_thread": {"type":"integer","minimum":0,"description":"Agent-spawned sub-thread chats."},
  "max_concurrent_coding_agent": {"type":"integer","minimum":0},
  "max_concurrent_per_trigger": {"type":"integer","minimum":0,"description":"Runs of one trigger; 1 preserves strict per-trigger FIFO."},
  "max_queued_per_trigger": {"type":"integer","minimum":1,"description":"Backlog for one trigger before overflow applies."},
  "reserved_background": {"type":"integer","minimum":0,"description":"Slots background work reclaims ahead of user work; 0 is pure user priority."},
  "overflow": {"type":"string","enum":["drop-oldest","pause-trigger"],"description":"On reaching max_queued_per_trigger."}
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
};

// ---------------------------------------------------------------------------
// mcp — MCP (Model Context Protocol) server management. Consolidates the five
// flat tools (setup/list/start/stop/remove server) into one grouped LLM tool;
// each action delegates to the existing execute_mcp_management_tool via the flat
// alias. LLM-only: server setup/start/stop/remove run in-process (only
// GET /mcp/servers has an HTTP route), so cli/sdk = false. See engine/tools/mcp.rs.
// ---------------------------------------------------------------------------

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
        cli: None,
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
        action: "start",
        summary: "Start a stopped MCP server by its id. (requires: id)",
        method: Method::Post,
        path: "/mcp/servers",
        args: &[],
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
        summary: "Stop a running MCP server by its id. (requires: id)",
        method: Method::Post,
        path: "/mcp/servers",
        args: &[],
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
        path: "/mcp/servers",
        args: &[],
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
    // setup/start/stop/remove run in-process; only list has an HTTP route. No
    // app/SDK consumer manages MCP servers — declared N/A (parity per surface).
    cli: false,
    sdk: false,
    operations: MCP_OPS,
    llm_aliases: &[],
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
    },
    PREFERENCES_DOMAIN,
    TRIGGERS_DOMAIN,
    TRIGGER_GROUPS_DOMAIN,
    APPS_DOMAIN,
    EVENTS_DOMAIN,
    CHANGES_DOMAIN,
    THREADS_DOMAIN,
    MEMORY_DOMAIN,
    THREAD_QUEUE_DOMAIN,
    ENV_VARS_DOMAIN,
    MODELS_DOMAIN,
    REPOSITORIES_DOMAIN,
    MCP_DOMAIN,
    PLUGINS_DOMAIN,
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
        // engine emits on every turn, so `.claude/rules/no-em-dashes.md`
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

/// All grouped LLM tools the manifest contributes (domains with `llm = true`).
pub fn llm_tools() -> Vec<ToolDefinition> {
    DOMAINS
        .iter()
        .filter(|d| d.llm)
        .map(build_llm_tool)
        .collect()
}

#[cfg(test)]
mod tests {
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
        // mcp — grouped LLM tool only (in-process management; no CLI/SDK).
        let mcp = domains().iter().find(|d| d.name == "mcp").unwrap();
        assert_eq!(
            mcp.actions(),
            vec!["setup", "list", "start", "stop", "remove"]
        );
        assert!(mcp.llm && !mcp.cli && !mcp.sdk);
        assert_eq!(domain_for_tool("setup_mcp_server").unwrap().name, "mcp");
        assert_eq!(
            mcp.legacy_tool_for_action("remove"),
            Some("remove_mcp_server")
        );

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
        assert_eq!(events.actions(), vec!["emit", "query", "count"]);
        assert!(events.llm && !events.cli && !events.sdk);
        assert_eq!(domain_for_tool("emit_event").unwrap().name, "events");
        assert_eq!(events.legacy_tool_for_action("count"), Some("count_events"));

        // changes — grouped LLM tool only (hand-written CLI stays).
        let changes = domains().iter().find(|d| d.name == "changes").unwrap();
        assert_eq!(changes.actions(), vec!["list", "apply"]);
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
        // LLM exposes the correction actions only (reads are CLI-only).
        assert_eq!(mem.actions(), vec!["correct", "correct_by_id"]);
        // CLI exposes the read endpoints only (correction is in-process/LLM-only).
        let cli_ops: Vec<&str> = mem
            .operations
            .iter()
            .filter(|o| o.on_cli(mem))
            .map(|o| o.cli_name)
            .collect();
        assert_eq!(cli_ops, vec!["stats", "entries", "source"]);
        assert_eq!(domain_for_tool("correct_memory").unwrap().name, "memory");
        assert_eq!(
            mem.legacy_tool_for_action("correct_by_id"),
            Some("correct_memory_by_id")
        );
    }

    #[test]
    fn phase5e_threads_declared() {
        let threads = domains().iter().find(|d| d.name == "threads").unwrap();
        assert_eq!(threads.actions(), vec!["list", "count"]);
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
            vec!["list", "add", "enable", "disable", "remove"]
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
        for p in ["action", "id", "label", "provider", "sort_order"] {
            assert!(
                props.get(p).is_some(),
                "manage_models missing property `{p}`"
            );
        }
        assert_eq!(
            props["provider"]["enum"],
            serde_json::json!(MODEL_PROVIDER_ENUM)
        );
        assert_eq!(domain_for_tool("manage_models").unwrap().name, "models");

        // repositories — LLM-only migration (no CLI: add/remove are in-process).
        let repos = domains().iter().find(|d| d.name == "repositories").unwrap();
        assert_eq!(repos.tool_name, "manage_repositories");
        assert!(repos.llm && !repos.cli && !repos.sdk);
        assert_eq!(repos.actions(), vec!["add", "list", "remove"]);
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
}
